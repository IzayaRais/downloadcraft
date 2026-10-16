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
    /// Preferred YouTube quality picked in the extension menu (e.g. "720p",
    /// "1080p60"). Empty = best available. The engine picks the highest
    /// stream at or below it (ciphered pages expose no direct URLs, so the
    /// choice can only be honored server-side at resolve time).
    #[serde(default)]
    pub quality_pref: String,
    /// Media duration in seconds from the extension (best-effort, P4).
    /// Display/metadata only — never affects engine behavior.
    #[serde(default)]
    pub duration_secs: u64,
    /// Per-download retry budget (P5). None = use the global setting.
    /// Some(n) overrides it for this item only (clamped 1–20 at use).
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Per-download stall timeout in seconds (P5). None = global.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Checksum-mismatch auto-repair latch (P5): set once the app spends
    /// its single automatic from-scratch redownload after a mismatch, so
    /// a persistently corrupt source fails loudly instead of looping.
    #[serde(default)]
    pub checksum_repaired: bool,
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
            quality_pref: String::new(),
            duration_secs: 0,
            max_retries: None,
            timeout_secs: None,
            checksum_repaired: false,
        }
    }

    pub fn dest_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.save_dir).join(&self.filename)
    }

    /// Effective retry budget: per-download override or the global setting.
    pub fn eff_retries(&self, global: u32) -> u32 {
        self.max_retries.unwrap_or(global).clamp(1, 20).max(1)
    }

    /// Effective stall timeout: per-download override or the global setting.
    pub fn eff_timeout(&self, global: u64) -> u64 {
        self.timeout_secs.unwrap_or(global).clamp(5, 600).max(5)
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
/// real name (`videoplayback`, `watch`, `play`, `reel`, API paths…),
/// all-digit IDs (`/reel/113054…`), and raw hash-looking segments are
/// skipped in favor of an earlier meaningful segment; otherwise a
/// timestamped default.
pub fn filename_from_url(raw: &str) -> String {
    const PLACEHOLDERS: &[&str] = &[
        "videoplayback", "watch", "play", "player", "download", "media",
        "api", "v", "video", "audio", "file", "get", "stream", "content",
        "reel", "reels", "embed", "post", "posts", "share",
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
                // Numeric IDs (`/reel/113054…`, `/posts/123`) are not names.
                if !clean.is_empty() && clean.bytes().all(|b| b.is_ascii_digit()) {
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

/// Extension for a `Content-Type` (with parameters tolerated): the correct
/// suffix when the URL itself carries no usable name (P1). Unknown types
/// yield None so callers keep the safe `.bin` default.
pub fn ext_for_mime(mime: &str) -> Option<&'static str> {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "video/mp4" => Some(".mp4"),
        "video/webm" => Some(".webm"),
        "video/x-matroska" | "video/mkv" => Some(".mkv"),
        "video/quicktime" => Some(".mov"),
        "video/x-msvideo" => Some(".avi"),
        "audio/mpeg" | "audio/mp3" => Some(".mp3"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some(".m4a"),
        "audio/webm" | "audio/opus" => Some(".webm"),
        "audio/wav" | "audio/x-wav" => Some(".wav"),
        "audio/flac" => Some(".flac"),
        "audio/ogg" => Some(".ogg"),
        "image/png" => Some(".png"),
        "image/jpeg" => Some(".jpg"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        "image/svg+xml" => Some(".svg"),
        "application/pdf" => Some(".pdf"),
        "application/zip" => Some(".zip"),
        "application/x-rar-compressed" | "application/vnd.rar" => Some(".rar"),
        "application/x-7z-compressed" => Some(".7z"),
        "application/gzip" | "application/x-gzip" => Some(".gz"),
        "application/x-msdownload" | "application/octet-stream" => None,
        _ => None,
    }
}

/// True for `download_<ts>.bin` last-resort names (no real name survived).
pub fn is_fallback_filename(name: &str) -> bool {
    let n = name.trim();
    n.starts_with("download_") && n.ends_with(".bin")
}

/// URL fallback name with the extension corrected from a known MIME type:
/// `download_123.bin` + `video/mp4` → `download_123.mp4`, and extensionless
/// `song` + `audio/mpeg` → `song.mp3`. Anything already suffixed is
/// returned untouched.
pub fn filename_from_url_with_mime(raw: &str, mime: Option<&str>) -> String {
    let ext = mime.and_then(ext_for_mime);
    let base = filename_from_url(raw);
    if is_fallback_filename(&base) {
        if let Some(ext) = ext {
            return base.trim_end_matches(".bin").to_string() + ext;
        }
        return base;
    }
    if !base.contains('.') {
        if let Some(ext) = ext {
            return base + ext;
        }
    }
    base
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
    // `ftps://` passes the gate so the engine can refuse it loudly with
    // guidance (instead of a generic "not a URL" message).
    (t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("ftp://")
        || t.starts_with("ftps://"))
        && t.len() < 2048
        && !t.contains(' ')
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

/// Position of a non-completed item writing the same destination file.
/// A fresh capture of the same video (new signed URL, same filename) must
/// refresh that entry instead of queueing a colliding duplicate: two tasks
/// on one destination fail with "already downloading elsewhere", and stale
/// part files from a different URL would corrupt resume.
pub fn find_dest_clash(items: &[DownloadItem], dest: &std::path::Path) -> Option<usize> {
    items.iter().position(|d| {
        !matches!(d.status, Status::Completed) && d.dest_path() == dest
    })
}

/// Drop resume state that belongs to a previous URL for `dest` (sidecar
/// `.dcpart`/segment files + HLS maps). Called when an entry's URL is
/// replaced: offsets from another (possibly sliced) URL must never be
/// resumed into the new file.
pub fn drop_resume_state(dest: &std::path::Path) {
    let _ = std::fs::remove_file(dest.with_extension("dcpart"));
    let _ = std::fs::remove_file(dest.with_extension("dcmeta"));
    for i in 0..16 {
        let _ = std::fs::remove_file(dest.with_extension(format!("dcpart{i}")));
    }
    let _ = std::fs::remove_file(dest.with_extension("hlsmap"));
}

/// Append an item to the shared queue file with a fresh id (used by the
/// standalone dialogs). Holds the queue lock across read-assign-write so
/// concurrent writers can no longer collide on ids. Returns the assigned
/// id, or None when the URL is already queued. A non-completed entry for
/// the SAME destination file (fresh capture, new signed URL) is refreshed
/// in place — new URL/meta, progress reset — and its id is returned, so a
/// re-click never creates a colliding duplicate.
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
    if let Some(pos) = find_dest_clash(&items, &item.dest_path()) {
        let id = items[pos].id;
        let created = items[pos].created_at;
        let fresh_url = item.url.clone();
        let same_url = items[pos].url == fresh_url;
        items[pos].url = fresh_url;
        items[pos].filename = item.filename.clone();
        items[pos].save_dir = item.save_dir.clone();
        items[pos].referer = std::mem::take(&mut item.referer);
        items[pos].cookie = std::mem::take(&mut item.cookie);
        items[pos].username = std::mem::take(&mut item.username);
        items[pos].password = std::mem::take(&mut item.password);
        let checksum_changed = items[pos].checksum_sha256 != item.checksum_sha256;
        items[pos].checksum_sha256 = std::mem::take(&mut item.checksum_sha256);
        items[pos].pair_url = std::mem::take(&mut item.pair_url);
        items[pos].pair_mime = std::mem::take(&mut item.pair_mime);
        items[pos].quality_pref = std::mem::take(&mut item.quality_pref);
        items[pos].duration_secs = item.duration_secs;
        items[pos].max_retries = item.max_retries;
        items[pos].timeout_secs = item.timeout_secs;
        if checksum_changed {
            // New checksum, new latch: the repair retry applies per digest.
            items[pos].checksum_repaired = false;
        }
        items[pos].overwrite_existing = item.overwrite_existing;
        items[pos].total_bytes = item.total_bytes;
        if !same_url {
            // Different bytes (or a slice of them): never resume old parts.
            items[pos].downloaded_bytes = 0;
            let dp = items[pos].dest_path();
            drop_resume_state(&dp);
        }
        items[pos].speed_bps = 0.0;
        items[pos].status = Status::Queued;
        items[pos].created_at = created;
        serde_json::to_string_pretty(&items)
            .ok()
            .and_then(|j| atomic_write(&path, j.as_bytes()).ok())?;
        return Some(id);
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

/// Pick the portable Windows asset from a GitHub `releases/latest` payload
/// (P5): a `.zip` whose name mentions windows (+64-bit when several).
/// Pure helper, fixture-tested below.
pub fn pick_windows_asset(v: &serde_json::Value) -> Option<String> {
    let assets = v.get("assets")?.as_array()?;
    let names: Vec<(&str, &str)> = assets
        .iter()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?;
            let url = a.get("browser_download_url")?.as_str()?;
            Some((name, url))
        })
        .collect();
    let is_zip = |n: &str| n.to_ascii_lowercase().ends_with(".zip");
    let is_win = |n: &str| n.to_ascii_lowercase().contains("windows");
    let is_64 = |n: &str| {
        let l = n.to_ascii_lowercase();
        l.contains("x86_64") || l.contains("x64")
    };
    // Portable build first, then any 64-bit windows zip, then any windows
    // zip, then any zip at all.
    let portable = names
        .iter()
        .find(|(n, _)| is_zip(n) && is_win(n) && n.to_ascii_lowercase().contains("portable"));
    let win64 = names.iter().find(|(n, _)| is_zip(n) && is_win(n) && is_64(n));
    let win = names.iter().find(|(n, _)| is_zip(n) && is_win(n));
    let any = names.iter().find(|(n, _)| is_zip(n));
    portable
        .or(win64)
        .or(win)
        .or(any)
        .map(|(_, u)| u.to_string())
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
        assert!(is_download_url("ftp://example.com/f.zip"));
        assert!(is_download_url("ftps://example.com/f.zip"));
        assert!(!is_download_url("not a url"));
        assert_eq!(filename_from_url("https://h/x/my%20file.zip?a=b"), "my file.zip");
        assert_eq!(filename_from_url("https://h/x/%D9%81%D8%A7%D8%B1%D8%B3%DB%8C.mp3"), "فارسی.mp3");
        // placeholders and raw hashes fall back instead of becoming the name
        let fb = filename_from_url("https://h/watch?v=x");
        assert!(fb.starts_with("download_") && fb.ends_with(".bin"), "{fb}");
        assert_eq!(filename_from_url("https://h/v/abc123/videoplayback?x=1"), "abc123");
        // numeric IDs and reel paths are not names either (P1)
        let reel = filename_from_url("https://www.facebook.com/reel/1130540689426081");
        assert!(reel.starts_with("download_"), "{reel}");
        let posts = filename_from_url("https://www.facebook.com/posts/12345");
        assert!(posts.starts_with("download_"), "{posts}");
        // real file tails still win over placeholder parents
        assert_eq!(
            filename_from_url("https://scontent.xx.fbcdn.net/o1/v/t2/AQabc123.mp4?oh=1&oe=2"),
            "AQabc123.mp4"
        );
    }

    #[test]
    fn mime_naming_matrix() {
        // P1: extension-from-MIME only rescues last-resort names.
        assert_eq!(ext_for_mime("video/mp4"), Some(".mp4"));
        assert_eq!(ext_for_mime("video/mp4; charset=binary"), Some(".mp4"));
        assert_eq!(ext_for_mime("VIDEO/WEBM"), Some(".webm"));
        assert_eq!(ext_for_mime("audio/mp4"), Some(".m4a"));
        assert_eq!(ext_for_mime("audio/mpeg"), Some(".mp3"));
        assert_eq!(ext_for_mime("image/jpeg"), Some(".jpg"));
        assert_eq!(ext_for_mime("application/pdf"), Some(".pdf"));
        assert_eq!(ext_for_mime("application/octet-stream"), None);
        assert_eq!(ext_for_mime("text/html"), None);
        assert_eq!(ext_for_mime(""), None);
        assert!(is_fallback_filename("download_123.bin"));
        assert!(!is_fallback_filename("clip.mp4"));
        // 10-URL acceptance: meaningful names untouched, fallbacks fixed.
        assert_eq!(
            filename_from_url_with_mime("https://cdn.example/a/my%20clip.mp4", Some("video/mp4")),
            "my clip.mp4"
        );
        let song = filename_from_url_with_mime("https://cdn.example/a/song", Some("audio/mpeg"));
        assert_eq!(song, "song.mp3");
        assert_eq!(
            filename_from_url_with_mime("https://h/x/file.unknown", Some("video/webm")),
            "file.unknown"
        );
        let reel = filename_from_url_with_mime(
            "https://www.facebook.com/reel/1130540689426081",
            Some("video/mp4"),
        );
        assert!(reel.starts_with("download_") && reel.ends_with(".mp4"), "{reel}");
        let ytid = filename_from_url_with_mime(
            "https://youtu.be/aqz-KE-bpKQ",
            Some("video/mp4"),
        );
        assert_eq!(ytid, "aqz-KE-bpKQ.mp4");
        let disp = filename_from_url_with_mime("https://h/dl?id=7", Some("application/pdf"));
        assert_eq!(disp, "dl.pdf");
        // unknown mime: usable bare names stay, true fallbacks keep .bin
        let keep = filename_from_url_with_mime("https://h/nameless", Some("text/html"));
        assert_eq!(keep, "nameless");
        let keep2 = filename_from_url_with_mime("https://h/nameless", None);
        assert_eq!(keep2, "nameless");
    }

    #[test]
    fn relative_shapes() {
        let now = chrono::Local::now();
        assert_eq!(relative_time(&now), "just now");
        assert!(relative_time(&(now - chrono::Duration::hours(14))).contains("14 hours ago"));
        assert!(relative_time(&(now - chrono::Duration::days(2))).contains("2 days ago"));
    }

    #[test]
    fn dest_clash_refresh() {
        let dir = std::env::temp_dir().join("dc_clash_test");
        let _ = std::fs::create_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let mut a = DownloadItem::new(1, "https://h/v1.mp4".into(), "v.mp4".into(), dir_s.clone(), 4);
        a.status = Status::Failed("old".into());
        let items = vec![a];
        let dest = std::path::Path::new(&dir_s).join("v.mp4");
        assert_eq!(find_dest_clash(&items, &dest), Some(0));
        let other = std::path::Path::new(&dir_s).join("other.mp4");
        assert_eq!(find_dest_clash(&items, &other), None);
        let mut done = DownloadItem::new(2, "https://h/v2.mp4".into(), "v.mp4".into(), dir_s, 4);
        done.status = Status::Completed;
        assert_eq!(find_dest_clash(&vec![done], &dest), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p5_overrides_and_compat() {
        // P5: per-download overrides fall back to globals, clamped.
        let mut d = DownloadItem::new(1, "https://h/v.mp4".into(), "v.mp4".into(), "C:\\T".into(), 4);
        assert_eq!(d.eff_retries(5), 5);
        assert_eq!(d.eff_timeout(60), 60);
        d.max_retries = Some(99);
        d.timeout_secs = Some(1);
        assert_eq!(d.eff_retries(5), 20);
        assert_eq!(d.eff_timeout(60), 5);
        d.max_retries = Some(0);
        assert_eq!(d.eff_retries(5), 1);
        // Old queue.json entries (no new keys) still parse with safe
        // defaults: no override, no repair spent, no duration.
        let old = r#"{"id":7,"url":"https://h/v.mp4","filename":"v.mp4","save_dir":"C:\\T","total_bytes":100,"downloaded_bytes":10,"status":"Queued","created_at":"2026-10-15T00:00:00+06:00","connections":8}"#;
        let back: DownloadItem = serde_json::from_str(old).expect("old queue parses");
        assert_eq!(back.max_retries, None);
        assert_eq!(back.timeout_secs, None);
        assert!(!back.checksum_repaired);
        assert_eq!(back.duration_secs, 0);
        assert_eq!(back.eff_retries(5), 5);
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

    #[test]
    fn update_asset_pick() {
        // P5: portable windows zip wins; then 64-bit; then any windows
        // zip; then any zip; nothing zip-like gives None.
        let v: serde_json::Value = serde_json::json!({
            "tag_name": "v0.8.0",
            "assets": [
                {"name": "DownloadCraft-0.8.0-linux-x64.tar.gz", "browser_download_url": "https://h/l.tgz"},
                {"name": "DownloadCraft-0.8.0-windows-x64.zip", "browser_download_url": "https://h/w64.zip"},
                {"name": "DownloadCraft-0.8.0-windows-x64-portable.zip", "browser_download_url": "https://h/port.zip"},
                {"name": "notes.txt", "browser_download_url": "https://h/n.txt"}
            ]
        });
        assert_eq!(pick_windows_asset(&v).as_deref(), Some("https://h/port.zip"));
        let v2: serde_json::Value = serde_json::json!({
            "assets": [
                {"name": "app-windows-x64.zip", "browser_download_url": "https://h/a.zip"},
                {"name": "app-linux.zip", "browser_download_url": "https://h/b.zip"}
            ]
        });
        assert_eq!(pick_windows_asset(&v2).as_deref(), Some("https://h/a.zip"));
        let v3: serde_json::Value = serde_json::json!({"assets": []});
        assert_eq!(pick_windows_asset(&v3), None);
        assert_eq!(pick_windows_asset(&serde_json::json!({})), None);
    }
}
