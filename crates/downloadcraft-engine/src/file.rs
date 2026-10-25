//! The data file: one file the size of the download, written by every
//! connection at its own offset.
//!
//! Unlike one temporary file per segment (IDM's design, and Download Craft's
//! before this engine), nothing has to be copied together at the end: the
//! finished data file is simply renamed. On Windows the file is marked sparse
//! first, so writing near the end of a large, still empty file does not make
//! NTFS zero-fill everything before it.

use std::fs::{File, OpenOptions};
use std::path::Path;

/// Create (or truncate) the data file and size it to `total` when known.
pub(crate) fn create(path: &Path, total: Option<u64>) -> std::io::Result<()> {
    let f = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
    set_sparse(&f);
    if let Some(t) = total {
        f.set_len(t)?;
    }
    Ok(())
}

/// Grow or shrink the data file to its final size.
pub(crate) fn set_len(path: &Path, len: u64) -> std::io::Result<()> {
    let f = OpenOptions::new().write(true).open(path)?;
    set_sparse(&f);
    f.set_len(len)
}

/// Flush written data to the disk (before the journal claims it is there).
pub(crate) fn sync(path: &Path) -> std::io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_data()
}

pub(crate) fn open_writer(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).open(path)
}

/// Write all of `buf` at `offset` without touching any shared cursor.
pub(crate) fn write_all_at(f: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::write_at(f, buf, offset)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(f, buf, offset)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "wrote 0 bytes"));
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Read the first bytes of a file (format sniffing).
pub(crate) fn read_head(path: &Path, n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut out = vec![0u8; n];
    match File::open(path).and_then(|mut f| f.read(&mut out)) {
        Ok(got) => {
            out.truncate(got);
            out
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(windows)]
fn set_sparse(f: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let mut returned = 0u32;
    // Best effort: FAT/exFAT volumes have no sparse files and simply say no.
    unsafe {
        DeviceIoControl(
            f.as_raw_handle() as _,
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(windows))]
fn set_sparse(_: &File) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_land_at_their_offsets() {
        let dir = std::env::temp_dir().join(format!("dce-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.dcdata");
        create(&p, Some(10)).unwrap();
        let a = open_writer(&p).unwrap();
        let b = open_writer(&p).unwrap();
        write_all_at(&b, b"world", 5).unwrap();
        write_all_at(&a, b"hello", 0).unwrap();
        drop((a, b));
        assert_eq!(std::fs::read(&p).unwrap(), b"helloworld");
        assert_eq!(read_head(&p, 4), b"hell");
        let _ = std::fs::remove_dir_all(dir);
    }
}
