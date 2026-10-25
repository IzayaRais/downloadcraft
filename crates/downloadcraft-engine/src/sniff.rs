//! Recognise a file from its first bytes.
//!
//! Used to catch the classic download-manager failure: the server answered
//! with an HTML error or login page, and it was saved as `movie.mp4`. Also
//! lets callers warn when a file's content does not match its extension.

/// A short name for the format of `head` (the file's first bytes), or `None`
/// when it is not one of the formats checked here.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| head.len() >= m.len() && &head[..m.len()] == m;
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return Some("mp4");
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("matroska");
    }
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        return Some("zip");
    }
    if starts(b"%PDF") {
        return Some("pdf");
    }
    if starts(b"\x89PNG") {
        return Some("png");
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpeg");
    }
    if starts(b"GIF8") {
        return Some("gif");
    }
    if starts(b"RIFF") && head.len() >= 12 {
        return match &head[8..12] {
            b"WEBP" => Some("webp"),
            b"WAVE" => Some("wav"),
            b"AVI " => Some("avi"),
            _ => None,
        };
    }
    if starts(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 && head[1] & 0x06 != 0) {
        return Some("mp3");
    }
    if starts(b"OggS") {
        return Some("ogg");
    }
    if starts(b"fLaC") {
        return Some("flac");
    }
    if starts(b"7z\xBC\xAF\x27\x1C") {
        return Some("7z");
    }
    if starts(b"Rar!") {
        return Some("rar");
    }
    if starts(&[0x1F, 0x8B]) {
        return Some("gzip");
    }
    if starts(b"MZ") {
        return Some("exe");
    }
    if head.len() >= 189 && head[0] == 0x47 && head[188] == 0x47 {
        return Some("mpegts");
    }
    if looks_like_html(head) {
        return Some("html");
    }
    None
}

/// Whether the bytes look like an HTML page.
pub fn looks_like_html(head: &[u8]) -> bool {
    let text: String = head
        .iter()
        .take(512)
        .skip_while(|b| b.is_ascii_whitespace() || **b == 0xEF || **b == 0xBB || **b == 0xBF)
        .map(|b| b.to_ascii_lowercase() as char)
        .collect();
    text.starts_with("<!doctype html") || text.starts_with("<html") || text.starts_with("<head")
        || (text.starts_with("<!--") && text.contains("<html"))
}

/// Format families a file extension may hold.
fn family(ext: &str) -> Option<&'static [&'static str]> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4a" | "m4v" | "mov" | "3gp" | "heic" | "avif" => &["mp4"],
        "mkv" | "webm" | "mka" => &["matroska"],
        "zip" | "docx" | "xlsx" | "pptx" | "apk" | "jar" | "epub" | "odt" | "ods" | "nupkg" | "vsix"
        | "xpi" | "aab" | "ipa" | "msix" | "appx" => &["zip"],
        "pdf" => &["pdf"],
        "png" => &["png"],
        "jpg" | "jpeg" => &["jpeg"],
        "gif" => &["gif"],
        "webp" => &["webp"],
        "wav" => &["wav"],
        "avi" => &["avi"],
        "mp3" => &["mp3"],
        "ogg" | "oga" | "ogv" | "opus" => &["ogg"],
        "flac" => &["flac"],
        "7z" => &["7z"],
        "rar" => &["rar"],
        "gz" | "tgz" => &["gzip"],
        "exe" | "dll" | "msi" => &["exe", "zip"],
        "ts" | "mts" | "m2ts" => &["mpegts"],
        "html" | "htm" | "xhtml" | "php" | "asp" | "aspx" => &["html"],
        _ => return None,
    })
}

/// True only when the extension and the sniffed format clearly disagree
/// (`.mp4` holding an HTML page or a ZIP). Unknown extensions and unknown
/// contents never conflict.
pub fn conflicts(ext: &str, sniffed: &str) -> bool {
    match family(ext) {
        Some(fam) => !fam.contains(&sniffed),
        None => sniffed == "html" && !ext.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_signatures() {
        assert_eq!(sniff(b"\0\0\0\x20ftypisom\0\0\0\0"), Some("mp4"));
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 1]), Some("matroska"));
        assert_eq!(sniff(b"PK\x03\x04rest"), Some("zip"));
        assert_eq!(sniff(b"%PDF-1.7"), Some("pdf"));
        assert_eq!(sniff(b"ID3\x04"), Some("mp3"));
        assert_eq!(sniff(b"  \n<!DOCTYPE html><html>"), Some("html"));
        assert_eq!(sniff(b"<HTML><body>403</body>"), Some("html"));
        assert_eq!(sniff(b"plain text"), None);
    }

    #[test]
    fn conflicts_are_conservative() {
        assert!(conflicts("mp4", "html"));
        assert!(conflicts("mp4", "zip"));
        assert!(!conflicts("m4a", "mp4"));
        assert!(!conflicts("docx", "zip"));
        assert!(!conflicts("html", "html"));
        assert!(!conflicts("bin", "zip"));
        assert!(conflicts("bin", "html"));
        assert!(!conflicts("", "html"));
    }
}
