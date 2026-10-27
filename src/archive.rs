//! Extract finished archives: ZIP, RAR (including multi-part sets) and 7z.
//!
//! Extraction only ever writes inside the folder it was given:
//!
//! * an entry name that is absolute, has a drive letter or climbs with `..`
//!   is refused (and counted), never written;
//! * a file that is already there is kept, not overwritten;
//! * an archive that would unpack past the size or entry limits stops with a
//!   plain reason, so a "zip bomb" cannot fill the disk;
//! * nothing that comes out is ever run.
//!
//! Password-protected archives are reported as such and left alone.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

/// Refuse to write more than this in total (a real archive this big is rare;
/// a bomb is not).
const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Rar,
    SevenZ,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Zip => "ZIP",
            Kind::Rar => "RAR",
            Kind::SevenZ => "7z",
        }
    }
}

/// Which archive a file name is, and for RAR sets, where it sits in the set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub kind: Kind,
    /// File name of the volume extraction starts from.
    pub first: String,
    /// Shared name of every volume of a multi-part set (lowercase); `None`
    /// for a single file.
    pub set: Option<String>,
}

fn split_part(stem: &str) -> Option<(&str, u32)> {
    let lower = stem.to_ascii_lowercase();
    let i = lower.rfind(".part")?;
    let digits = &stem[i + 5..];
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((&stem[..i], digits.parse().ok()?))
}

/// Read an archive file name. `None` when it is not one this app extracts.
pub fn volume_of(name: &str) -> Option<Volume> {
    let lower = name.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.')?;
    let orig_stem = &name[..stem.len()];
    match ext {
        "zip" => Some(Volume { kind: Kind::Zip, first: name.to_string(), set: None }),
        "7z" => Some(Volume { kind: Kind::SevenZ, first: name.to_string(), set: None }),
        "rar" => match split_part(orig_stem) {
            // name.part1.rar, name.part01.rar, ...: the set starts at part 1
            // written with the same number of digits.
            Some((base, _)) => {
                let width = orig_stem.len() - base.len() - 5;
                Some(Volume {
                    kind: Kind::Rar,
                    first: format!("{base}.part{:0width$}.rar", 1),
                    set: Some(format!("{}.part", base.to_ascii_lowercase())),
                })
            }
            // name.rar, possibly followed by name.r00, name.r01, ...
            None => Some(Volume { kind: Kind::Rar, first: name.to_string(), set: Some(format!("{stem}.r")) }),
        },
        // Old-style later volumes: name.r00 .. name.r99
        e if e.len() == 3 && e.starts_with('r') && e[1..].bytes().all(|b| b.is_ascii_digit()) => Some(Volume {
            kind: Kind::Rar,
            first: format!("{orig_stem}.rar"),
            set: Some(format!("{stem}.r")),
        }),
        _ => None,
    }
}

/// Does file `name` belong to the multi-part set `set` (see [`Volume::set`])?
pub fn in_set(name: &str, set: &str) -> bool {
    volume_of(name).and_then(|v| v.set).is_some_and(|s| s == set)
}

/// Where an archive's files go: next to it, or in a new folder named after
/// it (`name`, `name (2)`, ... when that is taken).
pub fn destination(archive: &Path, into_folder: bool) -> PathBuf {
    let parent = archive.parent().map(Path::to_path_buf).unwrap_or_default();
    if !into_folder {
        return parent;
    }
    let file = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stem = file.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(file.clone());
    if let Some((base, _)) = split_part(&stem) {
        stem = base.to_string();
    }
    let stem = stem.trim().trim_end_matches('.').to_string();
    let stem = if stem.is_empty() { "extracted".to_string() } else { stem };
    let mut dir = parent.join(&stem);
    let mut n = 2;
    while dir.exists() {
        dir = parent.join(format!("{stem} ({n})"));
        n += 1;
    }
    dir
}

/// The path an entry name maps to under `root`, or `None` when the name
/// would land outside it.
pub fn safe_join(root: &Path, name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains(':') || name.contains('\0') {
        return None;
    }
    let mut out = root.to_path_buf();
    let mut parts = 0;
    for piece in name.split('/') {
        match piece {
            "" | "." => continue,
            ".." => return None,
            p => {
                // Each piece must be one plain path component here too.
                let mut c = Path::new(p).components();
                match (c.next(), c.next()) {
                    (Some(Component::Normal(_)), None) => {}
                    _ => return None,
                }
                // Windows drops trailing dots and spaces: "a. " would be "a".
                let p = p.trim_end_matches(['.', ' ']);
                if p.is_empty() {
                    return None;
                }
                out.push(p);
                parts += 1;
            }
        }
    }
    (parts > 0).then_some(out)
}

#[derive(Debug)]
pub enum Error {
    Password,
    Limit(String),
    Broken(String),
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Password => write!(f, "it is password-protected (extract it yourself with the password)"),
            Error::Limit(s) => write!(f, "{s}"),
            Error::Broken(s) => write!(f, "the archive could not be read ({s})"),
            Error::Io(s) => write!(f, "{s}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

#[derive(Debug, Default, Clone)]
pub struct Outcome {
    pub dest: PathBuf,
    pub files: usize,
    pub bytes: u64,
    /// Files already there, kept as they were.
    pub skipped: usize,
    /// Entries whose names tried to leave the folder.
    pub refused: usize,
}

impl Outcome {
    /// Everything in the archive is now on disk as extracted.
    pub fn complete(&self) -> bool {
        self.skipped == 0 && self.refused == 0
    }
}

struct Budget {
    out: Outcome,
    entries: usize,
}

impl Budget {
    fn entry(&mut self) -> Result<(), Error> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(Error::Limit(format!("it holds more than {MAX_ENTRIES} entries")));
        }
        Ok(())
    }

    fn declared(&self, size: u64) -> Result<(), Error> {
        if self.out.bytes.saturating_add(size) > MAX_TOTAL {
            return Err(Error::Limit("it would unpack to more than 64 GB".into()));
        }
        Ok(())
    }

    /// Where an entry goes, or `None` (counted) when it is refused or the
    /// file already exists.
    fn target(&mut self, name: &str) -> Result<Option<PathBuf>, Error> {
        self.entry()?;
        let Some(path) = safe_join(&self.out.dest, name) else {
            self.out.refused += 1;
            return Ok(None);
        };
        if path.exists() {
            self.out.skipped += 1;
            return Ok(None);
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Ok(Some(path))
    }

    /// Copy one entry's bytes, never past the total limit. A partly written
    /// file is removed on failure.
    fn write(&mut self, path: &Path, reader: &mut dyn Read) -> Result<(), Error> {
        let res = (|| -> Result<u64, Error> {
            let mut file = std::fs::File::create(path)?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut n = 0u64;
            loop {
                let k = reader.read(&mut buf).map_err(|e| Error::Broken(e.to_string()))?;
                if k == 0 {
                    break;
                }
                n += k as u64;
                if self.out.bytes + n > MAX_TOTAL {
                    return Err(Error::Limit("it would unpack to more than 64 GB".into()));
                }
                file.write_all(&buf[..k])?;
            }
            Ok(n)
        })();
        match res {
            Ok(n) => {
                self.out.bytes += n;
                self.out.files += 1;
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(path);
                Err(e)
            }
        }
    }
}

/// Extract `archive` into `dest` (created when missing).
pub fn extract(archive: &Path, dest: &Path) -> Result<Outcome, Error> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let vol = volume_of(&name).ok_or_else(|| Error::Broken("not a ZIP, RAR or 7z file".into()))?;
    let start = archive.with_file_name(&vol.first);
    if !start.is_file() {
        return Err(Error::Io(format!("{} is missing", vol.first)));
    }
    std::fs::create_dir_all(dest)?;
    let mut b = Budget { out: Outcome { dest: dest.to_path_buf(), ..Outcome::default() }, entries: 0 };
    match vol.kind {
        Kind::Zip => zip_extract(&start, &mut b)?,
        Kind::SevenZ => sevenz_extract(&start, &mut b)?,
        Kind::Rar => rar_extract(&start, &mut b)?,
    }
    Ok(b.out)
}

fn zip_extract(path: &Path, b: &mut Budget) -> Result<(), Error> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| Error::Broken(e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(zip::result::ZipError::UnsupportedArchive(m)) if m.contains("ncrypt") || m.contains("assword") => {
                return Err(Error::Password)
            }
            Err(e) => return Err(Error::Broken(e.to_string())),
        };
        if entry.encrypted() {
            return Err(Error::Password);
        }
        let name = entry.name().to_string();
        if entry.is_dir() {
            b.entry()?;
            if let Some(dir) = safe_join(&b.out.dest, &name) {
                std::fs::create_dir_all(dir)?;
            } else {
                b.out.refused += 1;
            }
            continue;
        }
        b.declared(entry.size())?;
        if let Some(target) = b.target(&name)? {
            b.write(&target, &mut entry)?;
        }
    }
    Ok(())
}

fn sevenz_extract(path: &Path, b: &mut Budget) -> Result<(), Error> {
    use sevenz_rust2::{ArchiveReader, Error as SzError, Password};
    let map = |e: SzError| match e {
        SzError::PasswordRequired | SzError::MaybeBadPassword(_) => Error::Password,
        e => Error::Broken(e.to_string()),
    };
    let mut reader = ArchiveReader::open(path, Password::empty()).map_err(map)?;
    let total: u64 = reader.archive().files.iter().map(|f| f.size).sum();
    b.declared(total)?;
    let mut failure: Option<Error> = None;
    let res = reader.for_each_entries(|entry, data| {
        let step = (|| -> Result<(), Error> {
            if entry.is_anti_item {
                return Ok(());
            }
            if entry.is_directory {
                b.entry()?;
                match safe_join(&b.out.dest, &entry.name) {
                    Some(dir) => std::fs::create_dir_all(dir)?,
                    None => b.out.refused += 1,
                }
                return Ok(());
            }
            match b.target(&entry.name)? {
                Some(target) => b.write(&target, data),
                // Skipped entries still have to be read through: in a solid
                // archive the next file's bytes come after them.
                None => std::io::copy(data, &mut std::io::sink()).map(|_| ()).map_err(|e| Error::Broken(e.to_string())),
            }
        })();
        match step {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    res.map_err(map)
}

fn rar_extract(path: &Path, b: &mut Budget) -> Result<(), Error> {
    let broken = |e: unrar::error::UnrarError| {
        if matches!(e.code, unrar::error::Code::MissingPassword | unrar::error::Code::BadPassword) {
            Error::Password
        } else {
            Error::Broken(e.to_string())
        }
    };
    let mut arc = unrar::Archive::new(path).open_for_processing().map_err(broken)?;
    if arc.has_encrypted_headers() {
        return Err(Error::Password);
    }
    while let Some(header) = arc.read_header().map_err(broken)? {
        let entry = header.entry();
        if entry.is_encrypted() {
            return Err(Error::Password);
        }
        let name = entry.filename.to_string_lossy().into_owned();
        arc = if entry.is_directory() {
            b.entry()?;
            match safe_join(&b.out.dest, &name) {
                Some(dir) => std::fs::create_dir_all(dir)?,
                None => b.out.refused += 1,
            }
            header.skip().map_err(broken)?
        } else {
            let size = entry.unpacked_size;
            b.declared(size)?;
            match b.target(&name)? {
                Some(target) => {
                    let next = header.extract_to(&target).map_err(|e| {
                        let _ = std::fs::remove_file(&target);
                        broken(e)
                    })?;
                    b.out.bytes += std::fs::metadata(&target).map(|m| m.len()).unwrap_or(size);
                    b.out.files += 1;
                    next
                }
                None => header.skip().map_err(broken)?,
            }
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names() {
        assert_eq!(volume_of("a.zip").unwrap().kind, Kind::Zip);
        assert_eq!(volume_of("A.7Z").unwrap().kind, Kind::SevenZ);
        let one = volume_of("Movie.rar").unwrap();
        assert_eq!((one.first.as_str(), one.set.as_deref()), ("Movie.rar", Some("movie.r")));
        let later = volume_of("Movie.r07").unwrap();
        assert_eq!((later.first.as_str(), later.set.as_deref()), ("Movie.rar", Some("movie.r")));
        let p3 = volume_of("Big.Set.part03.rar").unwrap();
        assert_eq!((p3.first.as_str(), p3.set.as_deref()), ("Big.Set.part01.rar", Some("big.set.part")));
        assert_eq!(volume_of("x.part2.rar").unwrap().first, "x.part1.rar");
        assert!(in_set("big.set.part1.rar", "big.set.part"));
        assert!(!in_set("other.part1.rar", "big.set.part"));
        for n in ["a.tar", "a.zipx", "a", "notes.r", "a.rev"] {
            assert_eq!(volume_of(n), None, "{n}");
        }
    }

    #[test]
    fn entry_names_stay_inside() {
        let root = Path::new("/out");
        assert_eq!(safe_join(root, "a/b.txt"), Some(root.join("a").join("b.txt")));
        assert_eq!(safe_join(root, "a\\b.txt"), Some(root.join("a").join("b.txt")));
        assert_eq!(safe_join(root, "./a.txt"), Some(root.join("a.txt")));
        for bad in ["../x", "a/../../x", "/etc/passwd", "\\\\server\\share\\x", "C:\\x", "C:x", "", "./", "a/ . /b"] {
            assert_eq!(safe_join(root, bad), None, "{bad:?}");
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_archive_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, data) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn zip_into_its_own_folder_and_keeps_existing_files() {
        let d = tmp("zip");
        let z = d.join("Pack.zip");
        make_zip(&z, &[("readme.txt", b"hello"), ("sub/data.bin", &[7u8; 5000]), ("../evil.txt", b"no")]);
        let dest = destination(&z, true);
        assert_eq!(dest, d.join("Pack"));
        let out = extract(&z, &dest).unwrap();
        assert_eq!((out.files, out.refused, out.bytes), (2, 1, 5005));
        assert_eq!(std::fs::read(dest.join("sub").join("data.bin")).unwrap().len(), 5000);
        assert!(!d.join("evil.txt").exists());
        // A second run into the same folder keeps what is there.
        std::fs::write(dest.join("readme.txt"), b"mine").unwrap();
        let again = extract(&z, &dest).unwrap();
        assert_eq!((again.files, again.skipped), (0, 2));
        assert_eq!(std::fs::read(dest.join("readme.txt")).unwrap(), b"mine");
        // The folder name is taken now: the next one gets a number.
        assert_eq!(destination(&z, true), d.join("Pack (2)"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sevenz_extracts() {
        let d = tmp("7z");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src.join("inner").join("b.txt"), vec![b'b'; 70_000]).unwrap();
        let archive = d.join("set.7z");
        let mut w = sevenz_rust2::ArchiveWriter::create(&archive).unwrap();
        w.push_source_path(&src, |_| true).unwrap();
        w.finish().unwrap();
        let out = extract(&archive, &d.join("out")).unwrap();
        assert_eq!(out.files, 2);
        assert_eq!(std::fs::read(d.join("out").join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(d.join("out").join("inner").join("b.txt")).unwrap().len(), 70_000);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("archive").join(name)
    }

    #[test]
    fn rar_extracts_and_reports_passwords() {
        let d = tmp("rar");
        let r = d.join("version.rar");
        std::fs::copy(fixture("version.rar"), &r).unwrap();
        let out = extract(&r, &d.join("o")).unwrap();
        assert_eq!(out.files, 1, "{out:?}");
        assert!(std::fs::read_dir(d.join("o")).unwrap().count() == 1);
        let c = d.join("crypted.rar");
        std::fs::copy(fixture("crypted.rar"), &c).unwrap();
        assert!(matches!(extract(&c, &d.join("c")), Err(Error::Password)));
        std::fs::write(&r, b"not a rar at all").unwrap();
        assert!(matches!(extract(&r, &d.join("o2")), Err(Error::Broken(_))));
        let _ = std::fs::remove_dir_all(&d);
    }
}
