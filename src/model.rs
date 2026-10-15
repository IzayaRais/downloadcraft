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
        return s.to_string();
    }
    // Byte slicing panics on multibyte boundaries (called every frame via
    // Status::label with network-controlled error text) — cut at a char edge.
    let cut = s
        .char_indices()
        .take_while(|(i, _)| *i < n)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}…", &s[..cut])
}

/// Best-effort interprocess file lock (no new deps): `config/<name>.lock
/// created with `create_new`. Stale locks (mtime older than 20 s — no
/// operation legitimately holds one longer) are stolen. All shared-file
/// read-modify-writes (queue, inbox, wake, cmd) take the matching lock so
/// the GUI, dialogs, CLI, and native host stop clobbering each other.
/// If the lock can never be acquired we proceed anyway (single-user box:
///
/// better to risk a race than deadlock the UI).
pub struct FsGuard {
    path: std::path::PathBuf,
    ours: bool,
}

impl FsGuard {
    /// Whether this guard actually holds the lock (a `false` guard means
    /// another process is inside the critical section right now).
    pub fn owned(&self) -> bool {
        self.ours
    }
}

impl Drop for FsGuard {
    fn drop(&mut self) {
        if self.ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn fs_lock(name: &str) -> FsGuard {
    let path = match config_dir() {
        Some(d) => d.join(format!("{name}.lock")),
        None => return FsGuard { path: std::path::PathBuf::new(), ours: false },
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    for _ in 0..100 {
        let mut opts = std::fs::OpenOptions::new();
        match opts.write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                let _ = writeln!(f, "{}", std::process::id());
                return FsGuard { path, ours: true };
            }
            Err(_) => {
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .map(|t| t.elapsed().map(|e| e.as_secs() > 20).unwrap_or(true))
                    .unwrap_or(true);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    FsGuard { path, ours: false }
}

/// Liveness heartbeat for the main window (`main.lock` timestamp, refreshed
/// every few seconds). Standalone dialogs use this instead of counting
/// process names (two dialogs used to look like "main is running").
pub fn touch_heartbeat() {
    if let Some(p) = config_dir().map(|d| d.join("main.lock")) {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, chrono::Local::now().to_rfc3339());
    }
}

pub fn is_main_live() -> bool {
    let Some(p) = config_dir().map(|d| d.join("main.lock")) else {
        return false;
    };
    let Ok(data) = std::fs::read_to_string(&p) else {
        return false;
    };
    chrono::DateTime::parse_from_rfc3339(data.trim())
        .map(|dt| chrono::Local::now().timestamp() - dt.timestamp() < 30)
        .unwrap_or(false)
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
    /// Free-text note from the IDM-style Download File Info dialog.
    /// Empty = none. Never used for engine behavior.
    #[serde(default)]
    pub description: String,
    /// Adaptive pair: companion audio URL for a video-only stream, merged
    /// with ffmpeg on completion (P1.3). Empty = single-file download.
    #[serde(default)]
    pub pair_url: String,
    #[serde(default)]
    pub pair_mime: String,
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
            description: String::new(),
            pair_url: String::new(),
            pair_mime: String::new(),
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

/// Last-resort filename from a URL. Placeholder segments that carry no
/// real name (`videoplayback`, `watch`, `play`, API paths…) and raw
/// hash-looking segments are skipped in favor of an earlier meaningful
/// segment; otherwise a timestamped default.
pub fn filename_from_url(raw: &str) -> String {
    const PLACEHOLDERS: &[&str] = &[
        "videoplayback", "watch", "play", "player", "download", "media",
        "api", "v", "video", "audio", "file", "get", "stream", "content",
    ];
    if let Ok(u) = url::Url::parse(raw) {
        if let Some(segs) = u.path_segments() {
            let all: Vec<&str> = segs.filter(|s| !s.is_empty()).collect();
            for seg in all.iter().rev() {
                let clean = seg.split('?').next().unwrap_or(seg);
                let low = clean.to_ascii_lowercase();
                if clean.is_empty() || PLACEHOLDERS.contains(&low.as_str()) {
                    continue;
                }
                // Raw hash/blob ids (32+ hex chars, no extension) are not names.
                let is_hash = clean.len() >= 32
                    && clean.bytes().all(|b| b.is_ascii_hexdigit())
                    && !clean.contains('.');
                if is_hash {
                    continue;
                }
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

/// One type→folder routing rule (P3.2): comma-separated extensions map to a
/// subfolder under the save directory. First match wins (ordered in UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderRule {
    #[serde(default)]
    pub exts: String,
    #[serde(default)]
    pub folder: String,
}

impl FolderRule {
    pub fn matches(&self, filename: &str) -> bool {
        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext.is_empty() || ext == filename.to_ascii_lowercase() {
            return false;
        }
        self.exts
            .split([',', ';', ' '])
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .any(|e| !e.is_empty() && e == ext)
    }
}

pub fn default_folder_rules() -> Vec<FolderRule> {
    vec![
        FolderRule { exts: "mp4,mkv,webm,mov,avi".into(), folder: "Video".into() },
        FolderRule { exts: "mp3,m4a,flac,ogg,wav".into(), folder: "Music".into() },
        FolderRule { exts: "png,jpg,jpeg,gif,webp,svg".into(), folder: "Image".into() },
        FolderRule { exts: "pdf,doc,docx,txt,epub".into(), folder: "Document".into() },
        FolderRule { exts: "zip,rar,7z,tar,gz".into(), folder: "Compressed".into() },
        FolderRule { exts: "exe,msi".into(), folder: "Apps".into() },
    ]
}

/// Render a rename template: {name} stem, {ext} extension with dot,
/// {date} YYYYMMDD, {host} source host. Empty/blank results fall back to
/// the original stem; the extension is preserved when missing.
pub fn apply_rename_template(tpl: &str, filename: &str, url: &str) -> String {
    let t = tpl.trim();
    if t.is_empty() {
        return filename.to_string();
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(i) if i > 0 => (filename[..i].to_string(), filename[i..].to_string()),
        _ => (filename.to_string(), String::new()),
    };
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    let mut out = t
        .replace("{name}", &stem)
        .replace("{ext}", &ext)
        .replace("{date}", &date)
        .replace("{host}", &host);
    out = out.trim().to_string();
    if out.is_empty() || out == ext {
        return filename.to_string();
    }
    if !ext.is_empty() && !out.to_ascii_lowercase().ends_with(&ext.to_ascii_lowercase()) {
        out.push_str(&ext);
    }
    crate::engine::sanitize_filename(&out)
}

/// Append an item to the shared queue file with a fresh id (used by the
/// standalone dialogs). Holds the queue lock across read-assign-write so
/// concurrent writers can no longer collide on ids. Returns the assigned
/// id, or None when the URL is already queued.
pub fn append_to_queue(item: &mut DownloadItem) -> Option<u64> {
    let _guard = fs_lock("queue");
    let path = config_dir()?.join("queue.json");
    let mut items: Vec<DownloadItem> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if items.iter().any(|d| d.url == item.url) {
        return None;
    }
    let id = items.iter().map(|d| d.id).max().unwrap_or(0) + 1;
    item.id = id;
    item.status = Status::Queued;
    items.push(item.clone());
    serde_json::to_string_pretty(&items)
        .ok()
        .and_then(|j| atomic_write(&path, j.as_bytes()).ok())?;
    Some(id)
}

/// A daily speed window (P3.4): HH:MM–HH:MM at `limit_kbps` (0 = full speed).
/// Overnight ranges (22:00–06:00) wrap midnight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedWindow {
    #[serde(default = "SpeedWindow::d02")]
    pub start: String,
    #[serde(default = "SpeedWindow::d06")]
    pub end: String,
    #[serde(default)]
    pub limit_kbps: u64,
}

impl SpeedWindow {
    fn d02() -> String {
        "02:00".to_string()
    }
    fn d06() -> String {
        "06:00".to_string()
    }
    pub fn start_min(&self) -> Option<u16> {
        parse_hhmm(&self.start)
    }
    pub fn end_min(&self) -> Option<u16> {
        parse_hhmm(&self.end)
    }
    pub fn active_at(&self, now_min: u16) -> bool {
        match (self.start_min(), self.end_min()) {
            (Some(s), Some(e)) => {
                if s <= e {
                    now_min >= s && now_min < e
                } else {
                    now_min >= s || now_min < e
                }
            }
            _ => false,
        }
    }
}

pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// True when `latest` is newer than `current` (`v` prefix + whitespace OK).
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    fn parts(s: &str) -> Vec<u64> {
        s.trim()
            .trim_start_matches(['v', 'V'])
            .split(|c| c == '.' || c == '-' || c == '+')
            .filter_map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u64>()
                    .ok()
            })
            .collect()
    }
    let (a, b) = (parts(current), parts(latest));
    let n = a.len().max(b.len()).max(1);
    for i in 0..n {
        let (x, y) = (*a.get(i).unwrap_or(&0), *b.get(i).unwrap_or(&0));
        if x != y {
            return y > x;
        }
    }
    false
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
        // placeholders and raw hashes fall back instead of becoming the name
        let fb = filename_from_url("https://h/watch?v=x");
        assert!(fb.starts_with("download_") && fb.ends_with(".bin"), "{fb}");
        assert_eq!(filename_from_url("https://h/v/abc123/videoplayback?x=1"), "abc123");
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

    #[test]
    fn folder_rules_and_templates() {
        let rules = default_folder_rules();
        assert!(rules.iter().find(|r| r.folder == "Video").unwrap().matches("a.MP4"));
        assert!(!rules.iter().find(|r| r.folder == "Video").unwrap().matches("a.mp3"));
        assert!(!rules.iter().find(|r| r.folder == "Video").unwrap().matches("noext"));
        assert_eq!(
            apply_rename_template("{name} [{date}]{ext}", "Vid.mp4", "https://h/x"),
            format!("Vid [{}].mp4", chrono::Local::now().format("%Y%m%d"))
        );
        assert_eq!(
            apply_rename_template("{name} from {host}{ext}", "a.mkv", "https://cdn.example/x"),
            "a from cdn.example.mkv"
        );
        assert_eq!(apply_rename_template("prefix_{name}", "a.mp4", "https://h"), "prefix_a.mp4");
        assert_eq!(apply_rename_template("", "a.mp4", "https://h"), "a.mp4");
        assert_eq!(apply_rename_template("   ", "a.mp4", "https://h"), "a.mp4");
    }

    #[test]
    fn windows_and_versions() {
        assert_eq!(parse_hhmm("02:00"), Some(120));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("abc"), None);
        let w = SpeedWindow { start: "22:00".into(), end: "06:00".into(), limit_kbps: 0 };
        assert!(w.active_at(23 * 60));
        assert!(w.active_at(5 * 60));
        assert!(!w.active_at(12 * 60));
        let d = SpeedWindow { start: "02:00".into(), end: "06:00".into(), limit_kbps: 0 };
        assert!(d.active_at(3 * 60));
        assert!(!d.active_at(7 * 60));
        assert!(is_newer_version("0.6.0", "v0.7.0"));
        assert!(!is_newer_version("0.7.0", "0.6.0"));
        assert!(!is_newer_version("0.6.0", "0.6.0"));
        assert!(is_newer_version("0.6", "0.6.1"));
        assert!(!is_newer_version("  v0.6.0 ", "0.6.0"));
    }
}
