use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Queued => "Queued".to_string(),
            Status::Downloading => "Downloading".to_string(),
            Status::Paused => "Paused".to_string(),
            Status::Completed => "Completed".to_string(),
            Status::Failed(e) => format!("Failed: {}", shorten(e, 40)),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Status::Downloading)
    }
}

fn shorten(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub save_dir: String,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub status: Status,
    #[serde(default)]
    pub speed_bps: f64,
    pub created_at: DateTime<Local>,
    pub connections: usize,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub referer: String,
    #[serde(default)]
    pub cookie: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub resumable: bool,
    /// Optional expected SHA-256 hex digest for integrity verification.
    /// Empty = no verification. Clean-room feature inspired by
    /// multi-protocol managers (no reference code copied).
    #[serde(default)]
    pub checksum_sha256: String,
    /// Replace an existing destination file instead of auto-renaming.
    #[serde(default)]
    pub overwrite_existing: bool,
}

impl DownloadItem {
    pub fn new(id: u64, url: String, filename: String, save_dir: String, connections: usize) -> Self {
        Self {
            id,
            url,
            filename,
            save_dir,
            total_bytes: None,
            downloaded_bytes: 0,
            status: Status::Queued,
            speed_bps: 0.0,
            created_at: Local::now(),
            connections: connections.clamp(1, 16),
            selected: false,
            referer: String::new(),
            cookie: String::new(),
            username: String::new(),
            password: String::new(),
            resumable: false,
            checksum_sha256: String::new(),
            overwrite_existing: false,
        }
    }

    pub fn dest_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.save_dir).join(&self.filename)
    }

    pub fn progress(&self) -> f32 {
        match self.total_bytes {
            Some(t) if t > 0 => (self.downloaded_bytes as f64 / t as f64).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        }
    }

    pub fn category(&self) -> &'static str {
        category_of(&self.filename)
    }
}

pub fn category_of(filename: &str) -> &'static str {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => "Image",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "Music",
        "mp4" | "mkv" | "avi" | "mov" | "webm" => "Video",
        "pdf" | "doc" | "docx" | "txt" | "epub" => "Document",
        "zip" | "rar" | "7z" | "tar" | "gz" => "Compressed",
        "exe" | "msi" => "Apps",
        _ => "Other",
    }
}

pub fn format_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", b, UNITS[u])
    } else {
        format!("{:.2} {}", v, UNITS[u])
    }
}

pub fn format_speed(bps: f64) -> String {
    if bps <= 0.0 {
        return "-".to_string();
    }
    format!("{}/s", format_bytes(bps as u64))
}

pub fn format_eta(item: &DownloadItem) -> String {
    if item.speed_bps <= 1.0 {
        return "-".to_string();
    }
    if let Some(total) = item.total_bytes {
        let left = total.saturating_sub(item.downloaded_bytes) as f64;
        let secs = (left / item.speed_bps) as u64;
        if secs >= 3600 {
            format!("{} h {} m left", secs / 3600, (secs % 3600) / 60)
        } else if secs >= 60 {
            format!("{} m {} s left", secs / 60, secs % 60)
        } else {
            format!("{} s left", secs)
        }
    } else {
        "-".to_string()
    }
}

pub fn filename_from_url(raw: &str) -> String {
    if let Ok(u) = url::Url::parse(raw) {
        if let Some(seg) = u.path_segments().and_then(|mut s| s.next_back()) {
            let clean = seg.split('?').next().unwrap_or(seg);
            if !clean.is_empty() {
                return percent_decode(clean);
            }
        }
    }
    format!("download_{}.bin", chrono::Local::now().timestamp())
}

fn percent_decode(s: &str) -> String {
    let mut raw: Vec<u8> = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                raw.push(hex);
                i += 3;
                continue;
            }
        }
        raw.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&raw).into_owned()
}

pub fn is_download_url(s: &str) -> bool {
    let t = s.trim();
    (t.starts_with("http://") || t.starts_with("https://")) && t.len() < 2048 && !t.contains(' ')
}

/// Public product name shown everywhere in the UI.
pub const APP_NAME: &str = "Download Craft";
/// Config/storage slug: `%APPDATA%/download_craft` (v0.5+).
pub const APP_SLUG: &str = "download_craft";
/// Previous slug, kept only for one-time migration of existing installs.
const LEGACY_SLUG: &str = "idm_rust";

/// App config directory, e.g. `%APPDATA%/download_craft`.
pub fn config_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join(APP_SLUG))
}

/// One-time migration from the legacy `idm_rust` config dir: moves
/// queue/settings/inbox files so existing installs keep their history.
/// Safe to call on every startup (no-op once migrated).
pub fn migrate_config() {
    let base = match dirs::config_dir() {
        Some(p) => p,
        None => return,
    };
    let (old, new) = (base.join(LEGACY_SLUG), base.join(APP_SLUG));
    if !new.exists() && old.exists() {
        if std::fs::create_dir_all(&new).is_ok() {
            if let Ok(entries) = std::fs::read_dir(&old) {
                for e in entries.flatten() {
                    let dest = new.join(e.file_name());
                    if !dest.exists() {
                        let _ = std::fs::rename(e.path(), &dest);
                    }
                }
            }
            // Remove the legacy dir only when fully drained.
            if std::fs::read_dir(&old).map(|mut e| e.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(&old);
            }
        }
    }
}

pub fn relative_time(dt: &chrono::DateTime<chrono::Local>) -> String {
    let secs = (chrono::Local::now() - *dt).num_seconds().max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} minutes ago", secs / 60)
    } else if secs < 86400 {
        format!("{} hours ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86400)
    }
}

/// Normalize a user-supplied SHA-256 hex string: trim, lowercase, strip
/// optional `sha256:` prefix. Returns "" when empty.
pub fn normalize_sha256(s: &str) -> String {
    let mut t = s.trim().to_lowercase();
    if let Some(rest) = t.strip_prefix("sha256:") {
        t = rest.trim().to_string();
    }
    t
}

/// True when `s` looks like a 64-char hex SHA-256 digest.
pub fn is_valid_sha256(s: &str) -> bool {
    let t = normalize_sha256(s);
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Compute the SHA-256 hex digest of a file (streamed, low memory).
pub fn sha256_of_file(path: &std::path::Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    use std::io::Read;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Pick a non-colliding destination: if `dest` exists (and is not a
/// leftover `.dcpart` situation handled by the engine), append ` (1)`,
/// ` (2)`, ... before the extension. Pure clean-room implementation.
pub fn conflict_free_dest(dest: &std::path::Path) -> std::path::PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let ext = dest
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10000 {
        let cand = parent.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    dest.to_path_buf()
}

/// Atomically write `data` to `path` (temp file + rename) so a crash or
/// power loss never leaves a half-written queue/settings file.
pub fn atomic_write(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(category_of("a.mp3"), "Music");
        assert_eq!(category_of("a.MP4"), "Video");
        assert_eq!(category_of("a.png"), "Image");
        assert_eq!(category_of("a.pdf"), "Document");
        assert_eq!(category_of("a.zip"), "Compressed");
        assert_eq!(category_of("a.exe"), "Apps");
        assert_eq!(category_of("a.xyz"), "Other");
        assert_eq!(category_of("noext"), "Other");
    }

    #[test]
    fn byte_formats() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1024 * 1024 * 927 + 1024 * 1024 / 2), "927.50 MB");
        assert_eq!(format_speed(0.0), "-");
        assert_eq!(format_speed(1024.0 * 1024.0 * 4.91), "4.91 MB/s");
    }

    #[test]
    fn url_checks() {
        assert!(is_download_url("https://example.com/f.zip"));
        assert!(!is_download_url("ftp://example.com/f.zip"));
        assert!(!is_download_url("not a url"));
        assert_eq!(filename_from_url("https://h/x/my%20file.zip?a=b"), "my file.zip");
        assert_eq!(filename_from_url("https://h/x/%D9%81%D8%A7%D8%B1%D8%B3%DB%8C.mp3"), "فارسی.mp3");
    }

    #[test]
    fn relative_shapes() {
        let now = chrono::Local::now();
        assert_eq!(relative_time(&now), "just now");
        assert!(relative_time(&(now - chrono::Duration::hours(14))).contains("14 hours ago"));
        assert!(relative_time(&(now - chrono::Duration::days(2))).contains("2 days ago"));
    }

    #[test]
    fn sha256_helpers() {
        assert_eq!(normalize_sha256("  SHA256:ABC "), "abc");
        assert!(is_valid_sha256(&"a".repeat(64)));
        assert!(!is_valid_sha256("xyz"));
        assert!(!is_valid_sha256(""));
        // conflict_free_dest returns input when nothing exists
        let p = std::path::Path::new("no_such_dir_xyz/no_such_file.bin");
        assert_eq!(conflict_free_dest(p), p.to_path_buf());
    }
}
