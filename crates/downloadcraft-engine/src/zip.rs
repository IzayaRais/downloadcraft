//! List a remote ZIP's contents before downloading it.
//!
//! A ZIP keeps its table of contents (the central directory) at the end of
//! the file. Fetching only the last few kilobytes with a Range request is
//! enough to show what is inside a multi-gigabyte archive, and one more
//! Range request fetches a single file out of it (IDM's "download part of a
//! ZIP"): only that file's compressed bytes cross the network.

use crate::{Error, RequestFn};

/// One file inside a ZIP archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    /// Uncompressed size.
    pub size: u64,
    pub compressed_size: u64,
    pub is_dir: bool,
    /// 0 = stored, 8 = deflate (the only two this crate unpacks).
    pub method: u16,
    /// Where the entry's local header starts in the archive.
    pub local_offset: u64,
}

impl ZipEntry {
    /// Whether [`extract_remote`] can unpack it.
    pub fn can_extract(&self) -> bool {
        !self.is_dir && (self.method == 0 || self.method == 8)
    }
}

/// Fetch one entry of the remote ZIP at `url` and write it, unpacked, to
/// `dest`. Two Range requests: the local header (its name and extra field
/// can differ from the directory's) and the compressed bytes, which are
/// inflated as they stream in. Returns the bytes written.
pub async fn extract_remote(
    request: &RequestFn,
    url: &str,
    entry: &ZipEntry,
    dest: &std::path::Path,
) -> Result<u64, Error> {
    use futures_util::StreamExt;
    use std::io::Write;
    if !entry.can_extract() {
        return Err(Error::Network(format!("{}: compression method {} is not supported", entry.name, entry.method)));
    }
    let (head, _, _) = fetch_range(request, url, (entry.local_offset, 30)).await?;
    if !head.starts_with(b"PK\x03\x04") {
        return Err(bad("local header not found"));
    }
    let name_len = u16_at(&head, 26).ok_or_else(|| bad("short local header"))?;
    let extra_len = u16_at(&head, 28).ok_or_else(|| bad("short local header"))?;
    let data_at = entry.local_offset + 30 + name_len + extra_len;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("dczip");
    let file = std::fs::File::create(&tmp)?;
    let mut sink: Box<dyn Write + Send> = if entry.method == 8 {
        Box::new(flate2::write::DeflateDecoder::new(std::io::BufWriter::new(file)))
    } else {
        Box::new(std::io::BufWriter::new(file))
    };
    if entry.compressed_size > 0 {
        let end = data_at + entry.compressed_size - 1;
        let resp = request(url)
            .header(reqwest::header::RANGE, format!("bytes={data_at}-{end}"))
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        if resp.status().as_u16() != 206 {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::Http { status: resp.status().as_u16(), url: url.to_string() });
        }
        let mut body = resp.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| Error::Network(e.to_string()))?;
            sink.write_all(&chunk).map_err(|e| bad(&format!("could not unpack: {e}")))?;
        }
    }
    sink.flush()?;
    drop(sink);
    let written = std::fs::metadata(&tmp)?.len();
    if written != entry.size {
        let _ = std::fs::remove_file(&tmp);
        return Err(bad(&format!("{} unpacked to {written} bytes, expected {}", entry.name, entry.size)));
    }
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    std::fs::rename(&tmp, dest)?;
    Ok(written)
}

/// How much of the tail to fetch: the end record plus a comment of up to
/// 64 KiB, which also covers the whole directory of most archives.
const TAIL: u64 = 66 * 1024;

/// Read the central directory of the ZIP at `url` with a few small Range
/// requests. `request` builds the GET (the same hook the downloader uses).
pub async fn list_remote(request: &RequestFn, url: &str) -> Result<Vec<ZipEntry>, Error> {
    // Size first: "the last N bytes" (`bytes=-N`) is not supported by every
    // CDN (GitHub's answers 501), an explicit range always is.
    let (_, _, total) = fetch_range(request, url, (0, 1)).await?;
    let from = total.saturating_sub(TAIL);
    let (tail, tail_start, _) = fetch_range(request, url, (from, total - from)).await?;
    match parse(&tail, tail_start, total)? {
        Parsed::Entries(e) => Ok(e),
        Parsed::NeedDirectory { offset, len } => {
            let (dir, _, _) = fetch_range(request, url, (offset, len)).await?;
            parse_directory(&dir, usize::MAX)
        }
    }
}

/// GET `len` bytes from `off`. Returns the body, the offset of its first
/// byte and the file's total size.
async fn fetch_range(request: &RequestFn, url: &str, (off, len): (u64, u64)) -> Result<(Vec<u8>, u64, u64), Error> {
    let header = format!("bytes={off}-{}", off + len.max(1) - 1);
    let resp = request(url)
        .header(reqwest::header::RANGE, header)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if status != 206 {
        return Err(if status == 200 {
            Error::Network("the server does not support Range requests, so the ZIP cannot be previewed".into())
        } else {
            Error::Http { status, url: url.to_string() }
        });
    }
    let (start, _, total) = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::download::parse_content_range)
        .ok_or_else(|| Error::Network("no Content-Range in the reply".into()))?;
    let total = total.ok_or_else(|| Error::Network("the server did not say the file size".into()))?;
    let body = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
    Ok((body.to_vec(), start, total))
}

enum Parsed {
    Entries(Vec<ZipEntry>),
    NeedDirectory { offset: u64, len: u64 },
}

fn u16_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u64)
}
fn u32_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)
}
fn u64_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn bad(msg: &str) -> Error {
    Error::Network(format!("not a readable ZIP: {msg}"))
}

/// Find the end record in the tail and either read the directory from the
/// same bytes or say which range holds it.
fn parse(tail: &[u8], tail_start: u64, total: u64) -> Result<Parsed, Error> {
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..].starts_with(b"PK\x05\x06"))
        .ok_or_else(|| bad("no end-of-directory record"))?;
    let mut count = u16_at(tail, eocd + 10).ok_or_else(|| bad("short end record"))?;
    let mut size = u32_at(tail, eocd + 12).ok_or_else(|| bad("short end record"))?;
    let mut offset = u32_at(tail, eocd + 16).ok_or_else(|| bad("short end record"))?;
    // ZIP64: the real numbers live in a second record the locator points at.
    if count == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
        let loc = eocd.checked_sub(20).filter(|&l| tail[l..].starts_with(b"PK\x06\x07"));
        let rec_abs = loc.and_then(|l| u64_at(tail, l + 8)).ok_or_else(|| bad("missing ZIP64 locator"))?;
        let rec = rec_abs
            .checked_sub(tail_start)
            .map(|r| r as usize)
            .filter(|&r| tail.get(r..).is_some_and(|s| s.starts_with(b"PK\x06\x06")))
            .ok_or_else(|| bad("ZIP64 record outside the fetched tail"))?;
        count = u64_at(tail, rec + 32).ok_or_else(|| bad("short ZIP64 record"))?;
        size = u64_at(tail, rec + 40).ok_or_else(|| bad("short ZIP64 record"))?;
        offset = u64_at(tail, rec + 48).ok_or_else(|| bad("short ZIP64 record"))?;
    }
    if offset.saturating_add(size) > total {
        return Err(bad("directory points past the end of the file"));
    }
    if offset >= tail_start {
        let at = (offset - tail_start) as usize;
        let dir = tail.get(at..at + size as usize).ok_or_else(|| bad("directory cut short"))?;
        return Ok(Parsed::Entries(parse_directory(dir, count as usize)?));
    }
    Ok(Parsed::NeedDirectory { offset, len: size })
}

fn parse_directory(dir: &[u8], max: usize) -> Result<Vec<ZipEntry>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < max && dir.get(i..).is_some_and(|s| s.starts_with(b"PK\x01\x02")) {
        let short = || bad("directory entry cut short");
        let method = u16_at(dir, i + 10).ok_or_else(short)? as u16;
        let mut local = u32_at(dir, i + 42).ok_or_else(short)?;
        let mut comp = u32_at(dir, i + 20).ok_or_else(short)?;
        let mut size = u32_at(dir, i + 24).ok_or_else(short)?;
        let name_len = u16_at(dir, i + 28).ok_or_else(short)? as usize;
        let extra_len = u16_at(dir, i + 30).ok_or_else(short)? as usize;
        let comment_len = u16_at(dir, i + 32).ok_or_else(short)? as usize;
        let name_at = i + 46;
        let name = dir.get(name_at..name_at + name_len).ok_or_else(short)?;
        let name = String::from_utf8_lossy(name).into_owned();
        // ZIP64 sizes sit in the extra field (id 0x0001), in this order.
        let extra = dir.get(name_at + name_len..name_at + name_len + extra_len).ok_or_else(short)?;
        let mut e = 0;
        while e + 4 <= extra.len() {
            let id = u16_at(extra, e).unwrap_or(0);
            let len = u16_at(extra, e + 2).unwrap_or(0) as usize;
            if id == 1 {
                let mut f = e + 4;
                if size == 0xFFFF_FFFF {
                    size = u64_at(extra, f).unwrap_or(size);
                    f += 8;
                }
                if comp == 0xFFFF_FFFF {
                    comp = u64_at(extra, f).unwrap_or(comp);
                    f += 8;
                }
                if local == 0xFFFF_FFFF {
                    local = u64_at(extra, f).unwrap_or(local);
                }
            }
            e += 4 + len;
        }
        out.push(ZipEntry { is_dir: name.ends_with('/'), name, size, compressed_size: comp, method, local_offset: local });
        i = name_at + name_len + extra_len + comment_len;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stored ZIP with the given files (enough of the format for tests).
    pub(crate) fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let local = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0]);
            dir.extend_from_slice(&[0; 8]);
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&local.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        let n = files.len() as u16;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn lists_from_the_tail() {
        let z = make_zip(&[("a.txt", b"hello"), ("dir/", b""), ("dir/b.bin", &[7u8; 300])]);
        let total = z.len() as u64;
        match parse(&z, 0, total).unwrap() {
            Parsed::Entries(e) => {
                assert_eq!(e.len(), 3);
                assert_eq!(e[0].name, "a.txt");
                assert_eq!(e[0].size, 5);
                assert!(e[1].is_dir);
                assert_eq!(e[2].size, 300);
                assert_eq!(e[0].local_offset, 0);
                assert_eq!(e[1].local_offset, 30 + 5 + 5);
                assert!(e[0].can_extract() && !e[1].can_extract());
            }
            Parsed::NeedDirectory { .. } => panic!("directory was in the tail"),
        }
        // Only the end record fetched: the directory range is requested.
        let start = total - 22;
        match parse(&z[start as usize..], start, total).unwrap() {
            Parsed::NeedDirectory { offset, len } => {
                let dir = &z[offset as usize..(offset + len) as usize];
                assert_eq!(parse_directory(dir, usize::MAX).unwrap().len(), 3);
            }
            Parsed::Entries(_) => panic!("directory was not in the tail"),
        }
        assert!(parse(b"not a zip at all, definitely not", 0, 32).is_err());
    }
}
