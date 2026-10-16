use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// Shared state between the UI thread and the download task.
pub struct SharedState {
    pub downloaded: AtomicU64,
    pub total: AtomicU64,
    pub has_total: AtomicBool,
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
    /// Engine-detected filename (Content-Disposition / video title).
    pub resolved_name: std::sync::Mutex<Option<String>>,
    /// Whether the server supports range resume.
    pub resumable: AtomicBool,
    /// Per-part (start, end) byte ranges for segmented downloads.
    pub part_total: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Per-part downloaded bytes (same indexing as part_total).
    pub part_done: std::sync::Mutex<Vec<u64>>,
    /// Set when YouTube answers with a bot-check wall (HTTP 429 /sorry).
    pub rate_limited: AtomicBool,
    /// Terminal engine error text (e.g. checksum mismatch, unrecoverable
    /// HTTP error). Set once by the download task before it exits.
    pub error: std::sync::Mutex<Option<String>>,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            downloaded: AtomicU64::new(0),
            total: AtomicU64::new(0),
            has_total: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            resolved_name: std::sync::Mutex::new(None),
            resumable: AtomicBool::new(false),
            part_total: std::sync::Mutex::new(Vec::new()),
            part_done: std::sync::Mutex::new(Vec::new()),
            rate_limited: AtomicBool::new(false),
            error: std::sync::Mutex::new(None),
        }
    }

    /// Record the first terminal error (subsequent ones are ignored).
    pub fn set_error(&self, e: impl Into<String>) {
        if let Ok(mut g) = self.error.lock() {
            if g.is_none() {
                *g = Some(e.into());
            }
        }
    }
}

/// Map an I/O error to a user-facing message, with a clear text for a
/// full disk instead of a raw OS code.
pub(crate) fn io_message(e: &std::io::Error) -> String {
    #[cfg(windows)]
    const DISK_FULL: i32 = 112; // ERROR_DISK_FULL
    #[cfg(not(windows))]
    const DISK_FULL: i32 = 28; // ENOSPC
    if e.raw_os_error() == Some(DISK_FULL) {
        return "disk is full — free space and resume to retry".to_string();
    }
    format!("file error: {e}")
}

/// Per-download + global options for the engine.
#[derive(Debug, Clone)]
pub struct EngineOpts {
    pub speed_limit_bps: u64,
    pub timeout_secs: u64,
    pub max_retries: u32,
    pub user_agent: String,
    pub referer: String,
    pub cookie: String,
    pub username: String,
    pub password: String,
    pub proxy_url: String,
    /// Allow the engine to rename the file from server/video metadata.
    /// CLI one-shot downloads set this false to honor an explicit -o path.
    pub allow_rename: bool,
    /// Optional expected SHA-256 hex digest; verified after transfer.
    /// Empty = skip verification.
    pub expected_sha256: String,
    /// When true, an existing destination file is replaced instead of
    /// auto-renaming to `name (1).ext`.
    pub overwrite_existing: bool,
    /// Preferred YouTube quality from the extension menu ("720p", "1080p60",
    /// ...). Empty = best available. Honored at resolve time by picking the
    /// highest stream at or below it.
    pub quality_pref: String,
    /// Live scheduler override (speed windows): when present, the rate
    /// limiter honors it over `speed_limit_bps` without restarts.
    pub global_limit: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

impl Default for EngineOpts {
    fn default() -> Self {
        Self {
            speed_limit_bps: 0,
            timeout_secs: 60,
            max_retries: 5,
            user_agent: String::new(),
            referer: String::new(),
            cookie: String::new(),
            username: String::new(),
            password: String::new(),
            proxy_url: String::new(),
            allow_rename: true,
            expected_sha256: String::new(),
            overwrite_existing: false,
            quality_pref: String::new(),
            global_limit: None,
        }
    }
}

impl EngineOpts {
    pub fn build_client(&self) -> Result<reqwest::Client> {
        // HTTP/1.1 only: several media servers RST or truncate H2 streams
        // from non-browser TLS stacks, while plain H1 range GETs work.
        // IPv4-only egress: a broken IPv6 path (tunnel/relay) completes TCP
        // but serves bot-wall 403s and mid-stream cuts on media edges, while
        // the identical request over IPv4 returns 206 (verified live:
        // curl -4 206 vs curl -6 connection failure on the same URL that
        // our Happy-Eyeballs client kept failing). Binding the source to
        // IPv4 steers every connection at the verified-good path.
        // P2: pooled keepalive connections (segmented/chunked/HLS/DASH open
        // dozens of short requests per file) + TCP keepalives so throttled
        // CDN idle sockets don't die mid-range. H2/IPv6 stay OFF until a
        // live CDN proof run clears them (see P2 in docs/ENGINE_ROADMAP.md).
        let mut b = reqwest::Client::builder()
            .user_agent(self.user_agent.clone())
            .redirect(reqwest::redirect::Policy::limited(10))
            .local_address(std::net::IpAddr::from([0, 0, 0, 0]))
            .http1_only()
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(16)
            .tcp_keepalive(std::time::Duration::from_secs(60));
        if self.timeout_secs > 0 {
            // Connect timeout ONLY: reqwest's `timeout` limits the whole
            // transfer, which wrongly kills big files on slow links.
            b = b.connect_timeout(std::time::Duration::from_secs(self.timeout_secs));
        }
        if !self.proxy_url.trim().is_empty() {
            b = b.proxy(reqwest::Proxy::all(self.proxy_url.trim())?);
        }
        Ok(b.build()?)
    }

    pub(crate) fn get(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.get(url), Some(url))
    }

    fn post(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.post(url), Some(url))
    }

    fn head(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.head(url), Some(url))
    }

    fn apply(
        &self,
        mut r: reqwest::RequestBuilder,
        url: Option<&str>,
    ) -> reqwest::RequestBuilder {
        r = r
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            // Browser streaming headers: FB/IG and other media CDNs throttle
            // or drop non-browser Range requests. These match what the
            // extension captures from a real navigation.
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "cross-site");
        // Client Hints: strict googlevideo edges (SABR/adaptive serving)
        // 403 Range requests that lack them — the legacy progressive edge
        // doesn't care, which is why small files always worked. Derived
        // from the configured UA so the two can never disagree (a lying
        // hint pair is more suspicious than none at all).
        if let Some(v) = chrome_major(&self.user_agent) {
            r = r
                .header(
                    "sec-ch-ua",
                    format!("\"Chromium\";v=\"{v}\", \"Google Chrome\";v=\"{v}\", \"Not-A.Brand\";v=\"99\""),
                )
                .header("sec-ch-ua-mobile", "?0")
                .header("sec-ch-ua-platform", "\"Windows\"");
        }
        // Default page referer for social CDNs when the caller captured none:
        // bare CDN hosts (fbcdn/instagram) often 403 Range requests with no
        // Referer while serving the same bytes with one.
        let mut referer = self.referer.trim().to_string();
        if referer.is_empty() {
            if let Some(u) = url {
                let low = u.to_ascii_lowercase();
                if low.contains("fbcdn.net") || low.contains("facebook.com") {
                    referer = "https://www.facebook.com/".to_string();
                } else if low.contains("cdninstagram.com") || low.contains("instagram.com") {
                    referer = "https://www.instagram.com/".to_string();
                }
            }
        }
        if !referer.is_empty() {
            r = r.header(reqwest::header::REFERER, referer);
        }
        if !self.cookie.trim().is_empty() {
            r = r.header(reqwest::header::COOKIE, self.cookie.trim().to_string());
        }
        if !self.username.trim().is_empty() {
            r = r.basic_auth(
                self.username.trim(),
                if self.password.is_empty() {
                    None
                } else {
                    Some(self.password.clone())
                },
            );
        }
        r
    }
}

pub(crate) async fn wait_if_paused(shared: &Arc<SharedState>) -> Result<()> {
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        if !shared.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// Serialize jobs targeting the same destination (two concurrent writers
/// would interleave part/work files). Returns None when another job owns
/// the destination right now — callers fail loudly instead of corrupting.
fn claim_dest_job(dest: &std::path::Path) -> Option<crate::model::FsGuard> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dest.to_string_lossy().hash(&mut h);
    let g = crate::model::fs_lock(&format!("job-{:x}", h.finish()));
    g.owned().then_some(g)
}

static WORK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Unique sidecar dir per job so concurrent/retried jobs never share part
/// files (`dcwork-<pid>-<n>`).
fn unique_work_dir(dest: &std::path::Path) -> std::path::PathBuf {
    let n = WORK_COUNTER.fetch_add(1, Ordering::SeqCst);
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "dl".to_string());
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    parent.join(format!("{stem}.dcwork-{}-{n}", std::process::id()))
}

/// Resume binding sidecar (P0): ties `.dcpart` bytes to the exact URL +
/// total (+ connection split) that wrote them. Offset-only resume corrupts
/// files when a re-capture (new signed URL, same filename) or a changed
/// server file reuses the name — the old prefix belongs to different bytes.
/// A missing sidecar (legacy partials, fixture seeds) still resumes by
/// length once, then gets bound; a mismatch discards the stale prefix.
#[derive(serde::Serialize, serde::Deserialize)]
struct ResumeMeta {
    url: String,
    total: Option<u64>,
    conns: usize,
}

fn resume_meta_path(dest: &std::path::Path) -> std::path::PathBuf {
    dest.with_extension("dcmeta")
}

fn read_resume_meta(dest: &std::path::Path) -> Option<ResumeMeta> {
    let raw = std::fs::read_to_string(resume_meta_path(dest)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_resume_meta(dest: &std::path::Path, url: &str, total: Option<u64>, conns: usize) {
    let m = ResumeMeta { url: url.to_string(), total, conns };
    if let Ok(json) = serde_json::to_string(&m) {
        let _ = std::fs::write(resume_meta_path(dest), json);
    }
}

fn meta_matches(meta: &ResumeMeta, url: &str, total: Option<u64>, conns: usize) -> bool {
    if meta.url != url || meta.conns != conns {
        return false;
    }
    match (meta.total, total) {
        (Some(a), Some(b)) => a == b,
        // Unknown on either side: can't prove staleness, keep resuming.
        _ => true,
    }
}

/// Validated resume offset for `dest` (.dcpart length) against the given
/// URL/total/split. Drops stale prefixes (and ALL segmented parts `dcpart{i}`
/// for i in 0..16 — a halved re-split has fewer parts than the failed one)
/// on mismatch and (re)binds the sidecar.
pub(crate) fn resume_offset(
    dest: &std::path::Path,
    part_ext: &str,
    url: &str,
    total: Option<u64>,
    conns: usize,
) -> u64 {
    let part = dest.with_extension(part_ext);
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    match read_resume_meta(dest) {
        Some(meta) if meta_matches(&meta, url, total, conns) => {
            // Adopt a newly learned total into the binding.
            if meta.total.is_none() && total.is_some() {
                write_resume_meta(dest, url, total, conns);
            }
            // Clamp absurd prefixes (total shrank server-side): the 416
            // paths handle the restart; never resume past the end.
            if let Some(t) = total {
                if t > 0 && have >= t {
                    return have;
                }
            }
            have
        }
        Some(_) => {
            // Bound to different bytes: discard, never resume across URLs.
            let _ = std::fs::remove_file(&part);
            for i in 0..16 {
                let _ = std::fs::remove_file(dest.with_extension(format!("dcpart{i}")));
            }
            let _ = std::fs::remove_file(resume_meta_path(dest));
            write_resume_meta(dest, url, total, conns);
            0
        }
        None => {
            // Legacy/unbound partial: resume once by length, then bind it.
            // (One-time hole for pre-sidecar files; self-heals on next run.)
            write_resume_meta(dest, url, total, conns);
            have
        }
    }
}
/// Shared token-bucket rate limiter (clean-room implementation).
/// Unlike a naive per-chunk `sleep(bytes/limit)`, the bucket smooths bursts
/// across all connections: tokens accrue at `limit_bps` and each chunk
/// waits only for the deficit. A single shared instance is cloned into
/// every segment task so the *global* budget is honored.
/// An optional scheduler-provided atomic overrides the cap live (speed
/// windows), taking effect in running downloads without restarts.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    limit_bps: u64,
    global_bps: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    state: Arc<std::sync::Mutex<(f64, std::time::Instant)>>,
}

impl RateLimiter {
    pub fn new(limit_bps: u64) -> Self {
        Self {
            limit_bps,
            global_bps: None,
            state: Arc::new(std::sync::Mutex::new((
                limit_bps as f64,
                std::time::Instant::now(),
            ))),
        }
    }

    pub fn with_global(mut self, global_bps: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>) -> Self {
        self.global_bps = global_bps;
        self
    }

    fn effective_limit(&self) -> u64 {
        match &self.global_bps {
            Some(g) => {
                let v = g.load(Ordering::SeqCst);
                // Scheduler convention: u64::MAX = "no window active".
                if v == u64::MAX {
                    self.limit_bps
                } else {
                    v.min(if self.limit_bps == 0 { v } else { self.limit_bps })
                }
            }
            None => self.limit_bps,
        }
    }

    pub async fn consume(&self, bytes: u64) {
        let limit = self.effective_limit();
        if limit == 0 || bytes == 0 {
            return;
        }
        let wait_secs: f64 = {
            let mut g = mutex_guard(&self.state);
            let (ref mut tokens, ref mut last) = *g;
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(*last).as_secs_f64();
            *last = now;
            *tokens = (*tokens + elapsed * limit as f64).min(limit as f64 * 2.0);
            *tokens -= bytes as f64;
            if *tokens < 0.0 {
                -(*tokens) / limit as f64
            } else {
                0.0
            }
        };
        if wait_secs > 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs)).await;
        }
    }
}

/// Lock a mutex even when poisoned (a panicking holder must not cascade
/// into every UI poll and engine task — take the guard anyway).
pub(crate) fn mutex_guard<'a, T>(m: &'a std::sync::Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Marker for HTTP refusals that must NOT be retried (401/403): the same
/// request will fail the same way. Attached to the error (instead of
/// matching "403" substrings in rendered text, which false-positives on
/// byte counts and wrapped messages).
#[derive(Debug)]
struct FinalHttp(String);

impl std::fmt::Display for FinalHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FinalHttp {}

fn final_err(msg: String) -> anyhow::Error {
    anyhow::Error::new(FinalHttp(msg))
}

/// Map an HTTP failure to an actionable message. A bare `403` on a
/// googlevideo link almost always means a stale/session-locked stream URL
/// (they expire and are IP/session-bound) — retrying the same URL is
/// pointless, so callers fail fast on these instead of burning retries.
fn http_status_err(url: &str, e: reqwest::Error) -> anyhow::Error {
    let status = e.status();
    if status == Some(reqwest::StatusCode::FORBIDDEN) {
        if url.contains("googlevideo.com")
            || url.contains("youtube.com")
            || url.contains("youtu.be")
        {
            return final_err(
                "YouTube refused this link (403) — stream links expire and are locked to the browser session that created them: replay the video, reopen the ⬇ Download this video menu for a fresh quality row, and download that (progressive ≤720p saves as one ready file)".to_string(),
            );
        }
        return final_err(
            "server refused (403 Forbidden) — it likely needs login or cookies: open the page in your browser and capture it with the Download Craft extension".to_string(),
        );
    }
    if status == Some(reqwest::StatusCode::UNAUTHORIZED) {
        return final_err(
            "server needs a login (401) — add the username/password in the Add dialog's advanced section, or capture via the browser extension".to_string(),
        );
    }
    match status {
        Some(s) => anyhow!("HTTP {s}"),
        None => anyhow!("network: {e}"),
    }
}

fn is_final_http_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<FinalHttp>().is_some()
}

/// Saturating counter decrement (plain `fetch_sub` wraps to u64::MAX on
/// underflow — which once painted a stuck-full progress bar).
fn sub_saturating(a: &std::sync::atomic::AtomicU64, n: u64) {
    a.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(n)))
        .ok();
}

fn is_googlevideo_url(url: &str) -> bool {
    url.contains("googlevideo.com")
}

/// Major Chrome version from a UA string ("... Chrome/126.0 ...") for
/// Client Hints. None when the UA isn't Chrome-based (custom UAs silently
/// skip hints rather than lie).
fn chrome_major(ua: &str) -> Option<u32> {
    let p = ua.find("Chrome/")? + "Chrome/".len();
    ua[p..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u32>()
        .ok()
        .filter(|v| (50..=999).contains(v))
}

/// Signed, throttled social CDN links (Facebook/Instagram reels): parallel
/// Range requests get cut, and the URL itself expires mid-download.
fn is_fragile_cdn(url: &str) -> bool {
    let low = url.to_ascii_lowercase();
    low.contains("fbcdn")
        || low.contains("facebook.com")
        || low.contains("cdninstagram.com")
        || low.contains("instagram.com")
        || low.contains("fbsbx.com")
        || low.contains("scontent")
        || low.contains("lookaside.")
}

/// Facebook progressive URLs sometimes carry `bytestart`/`byteend` slice
/// selectors (the player fetches previews with them). Such a URL can only
/// ever return that slice (~1 MB) — downloading it while expecting the full
/// file size loops forever at the slice length ("got 1192564 of 9495549").
/// Strip those selectors so the same signed URL serves the full file; the
/// remaining `oh`/`oe` params keep the auth. No-op for other hosts.
pub fn normalize_media_url(url: &str) -> String {
    let low = url.to_ascii_lowercase();
    let social = low.contains("fbcdn")
        || low.contains("facebook.com")
        || low.contains("scontent")
        || low.contains("fbsbx.com")
        || low.contains("lookaside.")
        || low.contains("cdninstagram.com")
        || low.contains("instagram.com");
    if !social {
        return url.to_string();
    }
    if !low.contains("bytestart") && !low.contains("byteend") {
        return url.to_string();
    }
    let Ok(mut u) = url::Url::parse(url) else {
        return url.to_string();
    };
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let kl = k.to_ascii_lowercase();
            kl != "bytestart" && kl != "byteend"
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    // `query_pairs` borrows `u`, so collect first, then rebuild.
    u.set_query(None);
    if !kept.is_empty() {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &kept {
            q.append_pair(k, v);
        }
        u.set_query(Some(&q.finish()));
    }
    u.to_string()
}

fn is_youtube_page(url: &str) -> bool {
    url.contains("youtube.com") || url.contains("youtu.be")
}

/// Verify a finished file against an expected SHA-256 hex digest.
/// Returns Ok(()) when no digest was requested.
pub async fn verify_checksum(path: &PathBuf, expected_hex: &str) -> Result<()> {
    let t = expected_hex.trim().to_lowercase();
    let t = t.strip_prefix("sha256:").unwrap_or(&t).trim().to_string();
    if t.is_empty() {
        return Ok(());
    }
    if t.len() != 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid SHA-256 checksum (want 64 hex chars)"));
    }
    let p = path.clone();
    let actual = tokio::task::spawn_blocking(move || crate::model::sha256_of_file(&p))
        .await
        .map_err(|e| anyhow!("checksum task: {e}"))??;
    if actual != t {
        return Err(anyhow!(
            "checksum mismatch: expected {t}, got {actual}"
        ));
    }
    Ok(())
}

/// Probe total size + range support with HEAD, falling back to a 0-0 range GET.
/// Returns (total, supports_range, content_type, disposition_filename).
pub async fn probe(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
) -> (Option<u64>, bool, Option<String>, Option<String>) {
    let dbg = std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok();
    // Try HEAD first — but ONLY trust success responses. Error pages
    // (403/404 with `Content-Length: 0`) must fall through to the Range
    // fallback: treating 0 as a real total poisons everything downstream
    // (grand totals of 0, single streams that can never satisfy).
    if let Ok(resp) = opts.head(client, url).send().await {
        if dbg {
            eprintln!("[dbg] probe HEAD status={}", resp.status());
        }
        if resp.status().is_success() {
            let h = resp.headers();
            let total = h
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            let accept = h
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("bytes"))
                .unwrap_or(false);
            let ctype = h
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let disp = disposition_filename(h);
            if total.is_some() || disp.is_some() {
                return (total, accept, ctype, disp);
            }
        }
    }
    // Fallback: GET bytes=0-0, parse Content-Range: bytes 0-0/12345
    if let Ok(resp) = opts
        .get(client, url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
    {
        let status = resp.status();
        if dbg {
            eprintln!("[dbg] probe GET 0-0 status={status}");
        }
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            if let Some(cr) = resp.headers().get(reqwest::header::CONTENT_RANGE) {
                if let Ok(s) = cr.to_str() {
                    if let Some(total) = s.split('/').next_back().and_then(|t| t.parse::<u64>().ok()) {
                        return (Some(total), true, None, None);
                    }
                }
            }
        }
        let total = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let disp = disposition_filename(resp.headers());
        // Error statuses teach nothing: a 403 page's `Content-Length: 0` is
        // not a total (see the HEAD gate above).
        if status.is_success() {
            return (total, false, None, disp);
        }
        return (None, false, None, disp);
    }
    (None, false, None, None)
}

/// One-shot Range check for expiring signed CDN links: a single
/// `GET bytes=0-0` decides the whole strategy. Returns:
/// - `Ok(Some(total))` on 206 (ranges honored; total from Content-Range or
///   the caller-kept seed) → caller uses chunked sequential ranges.
/// - `Ok(None)` on 200/unknown (ranges ignored) → caller uses one plain
///   stream (resuming is impossible there anyway).
/// - `Err` on 401/403/410 or network failure → the link is dead/refused;
///   callers fail fast with the expiry hint instead of burning 100 futile
///   chunks that each return zero bytes (`got 0 of …`).
async fn probe_range_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    seed_total: Option<u64>,
) -> Result<Option<u64>, anyhow::Error> {
    let resp = opts
        .get(client, url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| anyhow!("network: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        if let Some(cr) = resp.headers().get(reqwest::header::CONTENT_RANGE) {
            if let Ok(s) = cr.to_str() {
                if let Some(total) = s.split('/').next_back().and_then(|t| t.parse::<u64>().ok()) {
                    return Ok(Some(total));
                }
            }
        }
        return Ok(seed_total);
    }
    // Consume the one byte so the connection can be reused.
    let _ = resp.bytes().await;
    match status {
        reqwest::StatusCode::UNAUTHORIZED
        | reqwest::StatusCode::FORBIDDEN
        | reqwest::StatusCode::GONE => Err(final_err(
            "link refused (expired or session-locked) — replay the reel and click the download button again for a fresh link".to_string(),
        )),
        _ => Ok(None),
    }
}

/// Extract `filename` / `filename*` from a Content-Disposition header.
/// Splits on `;` outside quotes (a quoted `"a;b.mp4"` must survive), matches
/// parameter names case-insensitively, and prefers RFC 5987 `filename*`.
fn disposition_filename(h: &reqwest::header::HeaderMap) -> Option<String> {
    let v = h
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;
    // Split on semicolons that are NOT inside double quotes.
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut in_q = false;
    // Byte walk is safe here: we only look for ASCII `;` and `"`.
    let bytes = v.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                in_q = !in_q;
                i += 1;
            }
            b';' if !in_q => {
                parts.push(v[start..i].trim());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    parts.push(v[start..].trim());
    let param = |name: &str| -> Option<&str> {
        parts.iter().find_map(|p| {
            let (k, val) = p.split_once('=')?;
            if k.trim().eq_ignore_ascii_case(name) {
                Some(val.trim())
            } else {
                None
            }
        })
    };
    if let Some(rest) = param("filename*") {
        // RFC 5987: charset'lang'percent-encoded (value may be quoted).
        let rest = rest.trim().trim_matches('"');
        if let Some(enc) = rest.split('\'').nth(2) {
            let name = percent_decode_owned(enc);
            if !name.trim().is_empty() {
                return Some(sanitize_filename(&name));
            }
        }
    }
    if let Some(rest) = param("filename") {
        let s = rest.trim().trim_matches('"').trim();
        if !s.is_empty() {
            return Some(sanitize_filename(s));
        }
    }
    None
}

fn percent_decode_owned(s: &str) -> String {
    let mut raw: Vec<u8> = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(hex) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                raw.push(hex);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            raw.push(b' ');
        } else {
            raw.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&raw).into_owned()
}

/// Make a server/title string safe as a Windows filename.
pub fn sanitize_filename(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        out = "download.bin".to_string();
    }
    if out.len() > 120 {
        // keep the extension (char-boundary safe)
        if let Some(dot) = out.rfind('.') {
            let (stem, ext) = out.split_at(dot);
            let keep = 120usize.saturating_sub(ext.len());
            let cut = stem
                .char_indices()
                .take_while(|(i, _)| *i < keep)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            out = format!("{}{}", &stem[..cut], ext);
        } else {
            let cut = out
                .char_indices()
                .take_while(|(i, _)| *i < 120)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            out.truncate(cut);
        }
    }
    out
}

/// A resolved playable stream: the video URL plus an optional companion
/// audio URL (adaptive video-only qualities, merged with ffmpeg on
/// completion — the only way to deliver 1080p+).
pub struct ResolvedMedia {
    pub url: String,
    pub title: String,
    pub pair_url: Option<String>,
}

impl ResolvedMedia {
    fn single(url: String, title: String) -> Self {
        Self { url, title, pair_url: None }
    }
}

/// Try to extract a direct media URL + title from an HTML video page.
/// Handles YouTube watch pages (progressive MP4 streams, else an adaptive
/// video+audio pair for high qualities) plus generic pages via og:video /
/// contentUrl / bare .mp4 links.
pub async fn resolve_media(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    shared: &Arc<SharedState>,
) -> Option<ResolvedMedia> {
    // A failed page fetch must not rule out the player API (it only needs
    // the video id), so default to empty HTML instead of bailing.
    let html = opts
        .get(client, page_url)
        .send()
        .await
        .ok()
        .map(|r| async move { r.text().await.ok() });
    let mut html = match html {
        Some(f) => f.await.unwrap_or_default(),
        None => String::new(),
    };
    if html.len() > 8 * 1024 * 1024 {
        html.clear();
    }
    // YouTube bot-check wall: flag it, but keep trying the player API —
    // the wall covers the watch page, not necessarily the API.
    let walled = html.contains("sorry/index") || html.contains("unusual traffic");
    if walled {
        shared.rate_limited.store(true, Ordering::SeqCst);
    }
    if page_url.contains("youtube.com") || page_url.contains("youtu.be") {
        // Player API first: works off the video id alone, even behind the
        // page wall (verified live: ANDROID client returns direct URLs).
        if let Some(vid) = youtube_id(page_url) {
            if let Some(hit) = resolve_youtube_api(client, opts, page_url, &html, &vid).await
            {
                // API success clears the wall flag: this network CAN do it.
                shared.rate_limited.store(false, Ordering::SeqCst);
                return Some(hit);
            }
        }
        if !walled {
            if let Some(hit) = resolve_youtube_full(client, opts, page_url, &html).await {
                dbg_resolve("page-formats", &hit.url);
                return Some(hit);
            }
        }
        return None;
    }
    if walled {
        // Never scrape bot-wall pages for "media links" (they match junk
        // like the sorry-page URL itself, which then fails loudly later).
        return None;
    }
    let hit = resolve_generic(&html, page_url);
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        match &hit {
            Some((u, _)) => dbg_resolve("generic", u),
            None => eprintln!("[dbg] resolve: none"),
        }
    }
    hit.map(|(u, t)| ResolvedMedia::single(u, t))
}

fn dbg_resolve(via: &str, url: &str) {
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        let host = url::Url::parse(url)
            .map(|u| u.host_str().unwrap_or("?").to_string())
            .unwrap_or_else(|_| "?".to_string());
        eprintln!("[dbg] resolve via={via} host={host} len={}", url.len());
    }
}

/// Extract a YouTube video id from watch/shorts/embed/live/youtu.be URLs.
fn youtube_id(page_url: &str) -> Option<String> {
    let u = url::Url::parse(page_url).ok()?;
    if let Some(id) = u
        .query_pairs()
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.to_string())
    {
        if !id.is_empty() {
            return Some(id);
        }
    }
    let segs: Vec<&str> = u
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    for w in segs.windows(2) {
        if ["shorts", "embed", "live", "v"].contains(&w[0]) && w[1].len() >= 6 {
            return Some(w[1].to_string());
        }
    }
    if segs.len() == 1 && segs[0].len() >= 6 {
        // youtu.be/ID
        return Some(segs[0].to_string());
    }
    None
}

/// youtubei player API: stream URLs even when the watch page embeds none
/// (same endpoint third-party clients use). Verified live against the API:
/// the WEB client is playability-gated on flagged networks while the
/// ANDROID client still returns direct progressive URLs — so ANDROID is
/// tried first, WEB kept as fallback.
async fn resolve_youtube_api(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    html: &str,
    video_id: &str,
) -> Option<ResolvedMedia> {
    fn grab(html: &str, key: &str) -> Option<String> {
        let m = format!("\"{key}\":\"");
        let p = html.find(&m)? + m.len();
        let rest = &html[p..];
        let end = rest.find('"')?;
        let s = rest[..end].to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
    let visitor = grab(html, "visitorData").unwrap_or_default();
    let ver =
        grab(html, "INNERTUBE_CLIENT_VERSION").unwrap_or_else(|| "2.20241201".to_string());
    // ANDROID first (works behind the WEB client's bot wall), then WEB.
    // NOTE: visitorData is only sent when non-empty — an empty value (e.g.
    // from a bot-wall page) makes ANDROID return no streams at all.
    const ANDROID_UA: &str =
        "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip";
    let mut android_ctx = serde_json::json!({
        "clientName": "ANDROID",
        "clientVersion": "20.10.38",
        "hl": "en",
        "androidSdkVersion": 34,
    });
    if !visitor.is_empty() {
        android_ctx["visitorData"] = serde_json::Value::String(visitor.clone());
    }
    if let Some(hit) = player_api(client, opts, page_url, video_id, &android_ctx, Some(ANDROID_UA)).await
    {
        dbg_resolve("player-api-android", &hit.url);
        return Some(hit);
    }
    let mut web_ctx = serde_json::json!({
        "clientName": "WEB",
        "clientVersion": ver,
        "hl": "en",
    });
    if !visitor.is_empty() {
        web_ctx["visitorData"] = serde_json::Value::String(visitor);
    }
    if let Some(hit) = player_api(client, opts, page_url, video_id, &web_ctx, None).await {
        dbg_resolve("player-api", &hit.url);
        return Some(hit);
    }
    dbg_resolve("player-api", "<no-stream>");
    None
}

/// Best adaptive pair (video-only + audio, both direct URLs) from an
/// `adaptiveFormats` array: highest video at/below `cap` (closest above
/// when everything exceeds it) plus highest-bitrate audio.
/// Returns (video_score, video_url, audio_url).
fn pick_adaptive_pair(
    arr: &[serde_json::Value],
    cap: Option<u64>,
) -> Option<(u64, String, String)> {
    let mut bv: Option<(u64, String)> = None;
    let mut ba: Option<(u64, String)> = None;
    for f in arr {
        let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
        let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
        if url.is_empty() {
            continue;
        }
        if mime.starts_with("video/") {
            let score = stream_score(f);
            if better_stream(&bv, score, cap) {
                bv = Some((score, url.to_string()));
            }
        } else if mime.starts_with("audio/") {
            let br = f.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
            if ba.as_ref().map(|(b, _)| br > *b).unwrap_or(true) {
                ba = Some((br, url.to_string()));
            }
        }
    }
    let (vs, vu) = bv?;
    let (_, au) = ba?;
    Some((vs, vu, au))
}

/// One player-API call with the given client identity. Only `OK`
/// playability with a direct (non-ciphered) progressive MP4 URL counts —
/// ciphered entries would need a JS decipher challenge we don't run.
async fn player_api(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    video_id: &str,
    ctx_client: &serde_json::Value,
    ua_override: Option<&str>,
) -> Option<ResolvedMedia> {
    // Standard UA + referer via our options.
    let mut req = opts
        .post(client, "https://www.youtube.com/youtubei/v1/player?key=AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8&prettyPrint=false");
    if let Some(ua) = ua_override {
        req = req.header(reqwest::header::USER_AGENT, ua);
    }
    let body = serde_json::json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "context": { "client": ctx_client },
    });
    let resp = req
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::REFERER, page_url)
        .header(reqwest::header::ORIGIN, "https://www.youtube.com")
        .json(&body)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    if v
        .pointer("/playabilityStatus/status")
        .and_then(|s| s.as_str())
        != Some("OK")
    {
        return None;
    }
    let title = v
        .pointer("/videoDetails/title")
        .and_then(|t| t.as_str())
        .unwrap_or("youtube_video")
        .to_string();
    let cap = quality_cap(&opts.quality_pref);
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = stream_score(f);
            if better_stream(&best, score, cap) {
                best = Some((score, url.to_string()));
            }
        }
    }
    // Adaptive pair (1080p+ live here, never in progressive): use it when
    // the menu asked for quality and it beats progressive, or when there is
    // no progressive stream at all (ciphered-only videos). Default path
    // (no preference) keeps the single progressive file — no ffmpeg needed.
    let pair = v
        .pointer("/streamingData/adaptiveFormats")
        .and_then(|f| f.as_array())
        .and_then(|a| pick_adaptive_pair(a, cap));
    match (best, pair) {
        (Some((ps, _)), Some((vs, vu, au)))
            if cap.is_some() && vs > ps =>
        {
            Some(ResolvedMedia {
                url: with_ratebypass(&vu),
                title: format!("{title}.mp4"),
                pair_url: Some(with_ratebypass(&au)),
            })
        }
        (None, Some((_, vu, au))) => Some(ResolvedMedia {
            url: with_ratebypass(&vu),
            title: format!("{title}.mp4"),
            pair_url: Some(with_ratebypass(&au)),
        }),
        (Some((_, u)), _) => Some(ResolvedMedia::single(
            with_ratebypass(&u),
            format!("{title}.mp4"),
        )),
        (None, None) => None,
    }
}

/// Direct-URL-only page resolve (legacy behavior, kept for fixtures).
/// Production path is [`resolve_youtube_full`] (adds cipher + `n` handling).
#[allow(dead_code)]
fn resolve_youtube(html: &str) -> Option<(String, String)> {
    let v = yt_player_value(html)?;
    let title = v
        .pointer("/videoDetails/title")
        .and_then(|t| t.as_str())
        .unwrap_or("youtube_video")
        .to_string();
    // Prefer the highest-quality progressive MP4 with a direct URL.
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = stream_score(f);
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, url.to_string()));
            }
        }
    }
    best.map(|(_, u)| (with_ratebypass(&u), format!("{title}.mp4")))
}

/// Comparable quality score for a stream entry: the height the *label*
/// promises ("1080p60" → 1080), falling back to dimensions. This matches
/// `quality_cap` exactly (both parse the same labels), so a "1080p" menu
/// pick compares equal to a "1080p60" stream — unlike max(width, height),
/// which scores landscape 1080p as 1920 and would reject it under a 1080
/// cap (downgrading the user to 360p).
fn stream_score(f: &serde_json::Value) -> u64 {
    if let Some(q) = f.get("qualityLabel").and_then(|q| q.as_str()) {
        let digits: String = q.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            if (144..=4320).contains(&n) {
                return n;
            }
        }
    }
    let h = f.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
    if (144..=4320).contains(&h) {
        return h;
    }
    f.get("width")
        .and_then(|w| w.as_u64())
        .unwrap_or(0)
        .max(h)
}

/// Parse a menu quality ("720p", "1080p60") into a height cap. None =
/// no preference (best available).
fn quality_cap(pref: &str) -> Option<u64> {
    let digits: String = pref.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u64 = digits.parse().ok()?;
    if (144..=4320).contains(&n) {
        Some(n)
    } else {
        None
    }
}

/// Whether `score` beats the current best under an optional height cap:
/// highest stream at or below the cap wins; when everything exceeds it,
/// the closest (lowest) wins instead of the biggest.
fn better_stream(best: &Option<(u64, String)>, score: u64, cap: Option<u64>) -> bool {
    let Some((b, _)) = best else {
        return true;
    };
    let Some(c) = cap else {
        return score > *b;
    };
    match (score <= c, *b <= c) {
        (true, false) => true,
        (true, true) => score > *b,
        (false, true) => false,
        (false, false) => score < *b,
    }
}

/// YouTube throttles googlevideo streams unless `ratebypass=yes` is set
/// (same trick every downloader uses).
fn with_ratebypass(u: &str) -> String {
    if u.contains("googlevideo.com") && !u.contains("ratebypass=") {
        format!("{u}&ratebypass=yes")
    } else {
        u.to_string()
    }
}

// ---------------------------------------------------------------------------
// YouTube signature decipher (P2.1) + throttle-param fix (P2.2), clean-room.
//
// Page-embedded and some API formats carry `signatureCipher` (a ciphered `s`
// plus the stream `url`) instead of a direct URL, and googlevideo links carry
// a session `n` throttle parameter. The running browser player deciphers both
// with functions from YouTube's PUBLIC player JS (base.js) — the same source
// tools like yt-dlp read. We fetch that JS, extract the classic
// reverse/splice/swap operation chains, and apply them. No IDM code, no
// copied decipher tables: shapes are detected, never hardcoded per version.
// Anything that doesn't match the classic shape is left alone (safe fallback
// to direct URLs + `ratebypass`).

/// One decipher operation: full reverse, drop-first-N, or swap-with-N.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SigOp {
    Reverse,
    Splice(usize),
    Swap(usize),
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn player_ops_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<SigOp>>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<SigOp>>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Parse ONE `=function(a){a=a.split("");...return ...join("")}` body at
/// byte offset `p` (start of the `=`). Returns (helper_obj, ops).
fn try_decipher_at(js: &str, p: usize) -> Option<(String, Vec<SigOp>)> {
    let after = js.get(p..)?;
    // Body ends at the `}` closing the function: find `join("")` then `}`.
    let j = after.find("join(\"\")")?;
    let close = after[j..].find('}')? + j;
    let body = after.get(..close)?;
    // Calls live between the split and the return.
    let split_end = body.find(".split(\"\");")? + ".split(\"\");".len();
    let ret_at = body.find("return")?;
    if split_end >= ret_at {
        return None;
    }
    let calls = body.get(split_end..ret_at)?;
    // First call fixes the helper object; every call must share it.
    let mut obj: Option<String> = None;
    let mut methods: Vec<(String, Option<usize>)> = Vec::new();
    for stmt in calls.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let paren = stmt.find('(')?;
        let head = stmt[..paren].trim();
        let dot = head.rfind('.')?;
        let (o, m) = (head[..dot].trim(), head[dot + 1..].trim());
        if o.is_empty() || m.is_empty() || !m.chars().all(is_ident_char) {
            return None;
        }
        match &obj {
            Some(known) if known != o => return None,
            _ => obj = Some(o.to_string()),
        }
        let args_raw = stmt[paren + 1..].strip_suffix(')')?;
        let args: Vec<&str> = args_raw.split(',').map(|a| a.trim()).collect();
        if args.is_empty() || args[0] != "a" {
            return None;
        }
        let num = if args.len() >= 2 {
            Some(args[1].parse::<usize>().ok()?)
        } else {
            None
        };
        methods.push((m.to_string(), num));
    }
    let obj = obj?;
    if methods.is_empty() {
        return None;
    }
    // Helper object definition: `OBJ={` (identifier-boundary checked).
    let mut def_at = None;
    let mut search = 0;
    let needle = format!("{obj}={{");
    while let Some(rel) = js[search..].find(needle.as_str()) {
        let abs = search + rel;
        let ok_before = abs
            .checked_sub(1)
            .and_then(|i| js[i..].chars().next())
            .map(|c| !is_ident_char(c))
            .unwrap_or(true);
        if ok_before {
            // Body starts past the full `OBJ={` match (needle includes `=`).
            def_at = Some(abs + needle.len());
            break;
        }
        search = abs + 1;
    }
    let mut d = def_at?;
    // We start INSIDE the object's braces, so the depth starts at 1; the
    // scan ends at the matching close.
    let bytes = js.as_bytes();
    let mut depth = 1usize;
    let mut in_str = false;
    let mut str_q = 0u8;
    let mut esc = false;
    let mut end = None;
    while d < bytes.len() {
        let c = bytes[d];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == str_q {
                in_str = false;
            }
        } else if c == b'"' || c == b'\'' {
            in_str = true;
            str_q = c;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                end = Some(d);
                break;
            }
        }
        d += 1;
    }
    let obj_body = js.get(def_at?..end?)?;
    // Method kinds from definition bodies.
    let mut kinds: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    let mut msearch = 0;
    while msearch < obj_body.len() {
        // Skip inter-method separators so `,MB:` parses as `MB`.
        while msearch < obj_body.len()
            && (obj_body[msearch..].starts_with(',') || obj_body[msearch..].starts_with(' '))
        {
            msearch += 1;
        }
        if msearch >= obj_body.len() {
            break;
        }
        // top-level `name:function(` entries
        let rest = &obj_body[msearch..];
        let colon = match rest.find(':') {
            Some(c) => c,
            None => break,
        };
        let name = rest[..colon].trim();
        if name.is_empty() || !name.chars().all(is_ident_char) {
            msearch += colon + 1;
            continue;
        }
        let after_colon = rest[colon + 1..].trim_start();
        if !after_colon.starts_with("function") {
            msearch += colon + 1;
            continue;
        }
        let brace = match after_colon.find('{') {
            Some(b) => b,
            None => break,
        };
        // balanced body
        let bb = after_colon.as_bytes();
        let mut dd = brace;
        let mut dep = 0usize;
        let mut istr = false;
        let mut iq = 0u8;
        let mut iesc = false;
        let mut mend = None;
        while dd < bb.len() {
            let c = bb[dd];
            if istr {
                if iesc {
                    iesc = false;
                } else if c == b'\\' {
                    iesc = true;
                } else if c == iq {
                    istr = false;
                }
            } else if c == b'"' || c == b'\'' {
                istr = true;
                iq = c;
            } else if c == b'{' {
                dep += 1;
            } else if c == b'}' {
                dep -= 1;
                if dep == 0 {
                    mend = Some(dd);
                    break;
                }
            }
            dd += 1;
        }
        let mbody = after_colon.get(brace..mend? + 1).unwrap_or("");
        let mend_abs = mend?;
        let kind = if mbody.contains(".reverse(") {
            "rev"
        } else if mbody.contains(".splice(") {
            "spl"
        } else if mbody.contains("a[0]") && mbody.contains("%a.length") {
            "swp"
        } else {
            msearch += colon + 1 + mend_abs + 1;
            continue;
        };
        kinds.insert(name.to_string(), kind);
        msearch += colon + 1 + mend_abs + 1;
    }
    // Map calls to ops.
    let mut ops = Vec::new();
    for (m, num) in methods {
        match kinds.get(m.as_str()).copied() {
            Some("rev") => ops.push(SigOp::Reverse),
            Some("spl") => ops.push(SigOp::Splice(num?)),
            Some("swp") => ops.push(SigOp::Swap(num?)),
            _ => return None,
        }
    }
    Some((obj, ops))
}

/// All classic-shape decipher candidates in a player JS bundle.
fn decipher_candidates(js: &str) -> Vec<(String, Vec<SigOp>)> {
    let marker = "=function(a){a=a.split(\"\")";
    let mut out = Vec::new();
    let mut base = 0;
    while let Some(rel) = js[base..].find(marker) {
        let p = base + rel;
        if let Some(hit) = try_decipher_at(js, p) {
            out.push(hit);
        }
        base = p + marker.len();
    }
    out
}

/// The signature-decipher operations (first classic candidate).
fn parse_sig_ops(js: &str) -> Option<Vec<SigOp>> {
    decipher_candidates(js).into_iter().next().map(|(_, ops)| ops)
}

/// Best-effort throttle-param fix: a SECOND classic-shape function (not the
/// signature decipher) is the `n` transform on classic players — apply it.
/// Anything else (single candidate, exotic shapes) returns None and the URL
/// keeps its original `n` + `ratebypass`.
fn try_simple_n(js: &str, sig_ops: &[SigOp], n: &str) -> Option<String> {
    let mut cands = decipher_candidates(js).into_iter();
    let _first = cands.next()?;
    for (_, ops) in cands {
        if &ops != sig_ops {
            return Some(apply_sig_ops(n, &ops));
        }
    }
    None
}

fn apply_sig_ops(s: &str, ops: &[SigOp]) -> String {
    let mut v: Vec<char> = s.chars().collect();
    for op in ops {
        match *op {
            SigOp::Reverse => v.reverse(),
            SigOp::Splice(n) => {
                let cut = n.min(v.len());
                v.drain(..cut);
            }
            SigOp::Swap(n) => {
                if v.len() > 1 {
                    let b = n % v.len();
                    if b != 0 {
                        v.swap(0, b);
                    }
                }
            }
        }
    }
    v.into_iter().collect()
}

/// Assemble a usable stream URL from a `signatureCipher` value:
/// `url=<..>&sp=<sig-param>&s=<ciphered>` → deciphered direct URL.
fn decipher_signature_cipher(cipher: &str, ops: &[SigOp]) -> Option<String> {
    let mut url = None;
    let mut sp = String::from("sig");
    let mut sig = None;
    for kv in cipher.split('&') {
        let (k, v) = kv.split_once('=')?;
        match k {
            "url" => url = Some(percent_decode_owned(v)),
            "sp" => {
                let d = percent_decode_owned(v);
                if !d.trim().is_empty() {
                    sp = d;
                }
            }
            "s" => sig = Some(percent_decode_owned(v)),
            _ => {}
        }
    }
    let (u, s) = (url?, sig?);
    if u.is_empty() || s.is_empty() {
        return None;
    }
    let sep = if u.contains('?') { "&" } else { "?" };
    Some(format!("{u}{sep}{sp}={}", apply_sig_ops(&s, ops)))
}

/// `base.js` URL from watch-page HTML (`"jsUrl":"/s/player/…/base.js"`).
fn youtube_player_js_url(html: &str) -> Option<String> {
    for key in ["\"jsUrl\":\"", "\"PLAYER_JS_URL\":\""] {
        if let Some(p) = html.find(key) {
            let rest = &html[p + key.len()..];
            if let Some(end) = rest.find('"') {
                let u = &rest[..end];
                if u.is_empty() {
                    continue;
                }
                if u.starts_with("http") {
                    return Some(u.to_string());
                }
                if u.starts_with('/') {
                    return Some(format!("https://www.youtube.com{u}"));
                }
            }
        }
    }
    None
}

async fn sig_ops_for(
    client: &reqwest::Client,
    opts: &EngineOpts,
    player_url: &str,
) -> Option<Vec<SigOp>> {
    if let Some(hit) = player_ops_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(player_url).cloned())
    {
        return Some(hit);
    }
    let js = opts
        .get(client, player_url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    if js.len() > 4 * 1024 * 1024 {
        return None;
    }
    let ops = parse_sig_ops(&js)?;
    if let Ok(mut g) = player_ops_cache().lock() {
        g.insert(player_url.to_string(), ops.clone());
    }
    Some(ops)
}

/// Fetch player JS text (uncached; callers needing both text + ops fetch
/// once and reuse — the ops cache makes repeat resolves cheap).
async fn player_js_for(
    client: &reqwest::Client,
    opts: &EngineOpts,
    player_url: &str,
) -> Option<String> {
    let js = opts
        .get(client, player_url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    if js.len() > 4 * 1024 * 1024 || js.len() < 1024 {
        return None;
    }
    Some(js)
}

/// Replace a googlevideo `n` throttle value using a simple-shape `n`
/// function from the same player bundle. Returns the URL unchanged when
/// no confident transform exists (never worse than the original).
fn fix_n_param(url: &str, js_text: &str, sig_ops: &[SigOp]) -> String {
    let nval: String = url
        .split(['?', '&'])
        .filter_map(|kv| kv.strip_prefix("n="))
        .next()
        .unwrap_or("")
        .to_string();
    if nval.is_empty() {
        return url.to_string();
    }
    match try_simple_n(js_text, sig_ops, &nval) {
        Some(fixed) if fixed != nval => {
            url.replacen(&format!("n={nval}"), &format!("n={fixed}"), 1)
        }
        _ => url.to_string(),
    }
}

/// Extract the player-response JSON value from watch-page HTML.
fn yt_player_value(html: &str) -> Option<serde_json::Value> {
    let marker = "ytInitialPlayerResponse = ";
    let start = html.find(marker)? + marker.len();
    let bytes = html.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let obj_start = i;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    serde_json::from_str(&html[obj_start..=i]).ok()
}

/// Full page-path resolve: direct progressive MP4 first, then ciphered
/// formats via player-JS decipher (P2.1), with throttle-param fix (P2.2).
async fn resolve_youtube_full(
    client: &reqwest::Client,
    opts: &EngineOpts,
    _page_url: &str,
    html: &str,
) -> Option<ResolvedMedia> {
    let v = yt_player_value(html)?;
    let title = v
        .pointer("/videoDetails/title")
        .and_then(|t| t.as_str())
        .unwrap_or("youtube_video")
        .to_string();
    // Direct progressive MP4 (highest at/below the menu quality when set).
    let cap = quality_cap(&opts.quality_pref);
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = stream_score(f);
            if better_stream(&best, score, cap) {
                best = Some((score, url.to_string()));
            }
        }
    }
    // Player JS (once) for cipher + n handling.
    let player_url = youtube_player_js_url(html);
    let mut js_text: Option<String> = None;
    let mut ops: Option<Vec<SigOp>> = None;
    if player_url.is_some() {
        let pu = player_url.as_deref().unwrap();
        ops = sig_ops_for(client, opts, pu).await;
        if ops.is_some() {
            js_text = player_js_for(client, opts, pu).await;
        }
    }
    if let Some((_, u)) = &best {
        let u = match (&js_text, &ops) {
            (Some(t), Some(o)) => fix_n_param(u, t, o),
            _ => u.clone(),
        };
        // A requested high quality may still live in adaptive-only: pair it
        // (same rule as the player-API path) instead of silently dropping to
        // a small progressive file.
        if cap.is_some() {
            if let Some(adaptive) = v.pointer("/streamingData/adaptiveFormats").and_then(|f| f.as_array()) {
                if let Some((vs, vu, au)) = pick_adaptive_pair(adaptive, cap) {
                    if vs > best.map(|(s, _)| s).unwrap_or(0) {
                        let vu = match (&js_text, &ops) {
                            (Some(t), Some(o)) => fix_n_param(&vu, t, o),
                            _ => vu,
                        };
                        return Some(ResolvedMedia {
                            url: with_ratebypass(&vu),
                            title: format!("{title}.mp4"),
                            pair_url: Some(with_ratebypass(&au)),
                        });
                    }
                }
            }
        }
        return Some(ResolvedMedia::single(
            with_ratebypass(&u),
            format!("{title}.mp4"),
        ));
    }
    // Ciphered progressive MP4: decipher via player ops.
    let (ops, js_text) = match (ops, js_text) {
        (Some(o), Some(t)) => (o, t),
        _ => return None,
    };
    let mut cbest: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let cipher = f.get("signatureCipher").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if cipher.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let Some(u) = decipher_signature_cipher(cipher, &ops) else {
                continue;
            };
            let score = stream_score(f);
            if better_stream(&cbest, score, cap) {
                cbest = Some((score, u));
            }
        }
    }
    cbest.map(|(_, u)| {
        let u = fix_n_param(&u, &js_text, &ops);
        ResolvedMedia::single(with_ratebypass(&u), format!("{title}.mp4"))
    })
}

/// True for HLS/DASH playlists: the progressive engine cannot consume these
/// as a single file (they need segment-assembly + transmux).
fn is_playlist_url(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    let base = low.split(['?', '#']).next().unwrap_or(&low);
    base.ends_with(".m3u8") || base.ends_with(".mpd") || low.contains("m3u8") && low.contains("playlist")
}

fn has_direct_ext(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    let base = low.split(['?', '#']).next().unwrap_or(&low);
    [".mp4", ".webm", ".mkv", ".mov", ".m4v", ".mp3", ".m4a", ".aac", ".flac", ".ogg", ".wav", ".avi"]
        .iter()
        .any(|e| base.ends_with(e))
}

/// Generic page scan: og:video / contentUrl / twitter stream / bare media links.
/// Covers YouTube-alternatives (Vimeo/FB/TikTok/Dailymotion-style pages expose
/// the same og:video + ld+json VideoObject tags) plus bare file links.
/// HLS (.m3u8) / DASH (.mpd) playlists are detected but NOT returned as
/// downloadable files — the engine does segmented progressive HTTP only.
fn resolve_generic(html: &str, page_url: &str) -> Option<(String, String)> {
    let title = meta_content(html, "og:title")
        .or_else(|| tag_text(html, "title"))
        .unwrap_or_else(|| "video".to_string());
    for prop in [
        "og:video:url",
        "og:video",
        "og:video:secure_url",
        "twitter:player:stream",
        "twitter:player",
    ] {
        if let Some(u) = meta_content(html, prop) {
            if u.starts_with("http") && !is_playlist_url(&u) {
                let e = guess_ext(&u);
                return Some((with_ratebypass(&u), with_ext(&title, &e)));
            }
        }
    }
    // ld+json VideoObject: {"@type":"VideoObject","contentUrl":"https://..."}
    for key in [
        "\"contentUrl\":\"",
        "\"contentURL\":\"",
        "\"embedUrl\":\"",
        "\"embedURL\":\"",
    ] {
        let mut search = html;
        while let Some(p) = search.find(key) {
            let rest = &search[p + key.len()..];
            if let Some(end) = rest.find('"') {
                let u = rest[..end].replace("\\/", "/");
                if u.starts_with("http") && !is_playlist_url(&u) {
                    let e = guess_ext(&u);
                    return Some((with_ratebypass(&u), with_ext(&title, &e)));
                }
            }
            search = &search[p + key.len()..];
        }
    }
    // Vimeo-style progressive config: "url":"https://...mp4..." + quality tag.
    // Only accepted when the URL itself carries a direct media extension.
    for key in ["\"url\":\"https://", "\"url\":\"http://"] {
        let mut search = html;
        let mut best: Option<String> = None;
        for _ in 0..8 {
            let Some(p) = search.find(key) else { break };
            let rest = &search[p + 7..];
            let Some(end) = rest.find('"') else { break };
            let u = rest[..end].replace("\\/", "/");
            if u.starts_with("http") && has_direct_ext(&u) && !is_playlist_url(&u) {
                best = Some(u);
            }
            search = &search[p + key.len()..];
        }
        if let Some(u) = best {
            let e = guess_ext(&u);
            return Some((with_ratebypass(&u), with_ext(&title, &e)));
        }
    }
    // bare https://....mp4/.webm/.mkv/.mp3 links
    for ext in [".mp4", ".webm", ".mkv", ".mov", ".mp3", ".m4a"] {
        let mut search = html;
        while let Some(p) = search.find(ext) {
            let before = &search[..p];
            if let Some(s) = before.rfind("https://").or_else(|| before.rfind("http://")) {
                let mut end = p + ext.len();
                let bytes = search.as_bytes();
                while end < bytes.len()
                    && !matches!(bytes[end], b'"' | b'\'' | b' ' | b'<' | b')')
                {
                    // stop before query strings getting out of hand; keep them actually
                    if end - p > 300 {
                        break;
                    }
                    end += 1;
                }
                // char-boundary safe: hostile pages may embed multibyte text
                let Some(slice) = search.get(s..end.min(search.len())) else {
                    search = &search[p + ext.len()..];
                    continue;
                };
                let u = slice
                    .trim_end_matches(['"', '\'', ')', '<'])
                    .to_string();
                if u.len() < 3000 {
                    let name = u
                        .rsplit('/')
                        .next()
                        .and_then(|seg| seg.split('?').next())
                        .filter(|s| !s.is_empty())
                        .map(percent_decode_owned)
                        .unwrap_or_else(|| with_ext(&title, ext));
                    return Some((u, name));
                }
            }
            search = &search[p + ext.len()..];
        }
    }
    let _ = page_url;
    None
}

fn meta_content(html: &str, prop: &str) -> Option<String> {
    // <meta property="og:video" content="..."> (either attribute order)
    let mut search = html;
    while let Some(p) = search.find(prop) {
        // bound to the enclosing <meta ...> tag so we don't grab a neighbour
        let tag_start = search[..p].rfind("<meta").unwrap_or(0);
        let rest = &search[tag_start..];
        let tag_end = rest.find('>').map(|e| tag_start + e).unwrap_or(search.len());
        let tag = &search[tag_start..tag_end.min(search.len())];
        if let Some(c) = tag.find("content=\"") {
            let after = &tag[c + 9..];
            if let Some(end) = after.find('"') {
                let u = after[..end].replace("&amp;", "&");
                if !u.is_empty() {
                    return Some(u);
                }
            }
        }
        search = &search[p + prop.len()..];
    }
    None
}

fn tag_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let s = html.find(&open)?;
    let body_start = html[s..].find('>')? + s + 1;
    let e = html[body_start..].find(&close)? + body_start;
    let t = html[body_start..e].trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn guess_ext(u: &str) -> String {
    let low = u.to_ascii_lowercase();
    for ext in [".mp4", ".webm", ".mkv", ".mov", ".mp3", ".m4a", ".avi"] {
        if low.contains(ext) {
            return ext.to_string();
        }
    }
    ".mp4".to_string()
}

fn with_ext(title: &str, ext: &str) -> String {
    let t = title.trim();
    if t.to_ascii_lowercase().ends_with(ext) {
        t.to_string()
    } else {
        format!("{t}{ext}")
    }
}

/// Resolve a possibly-relative HLS URI against its playlist URL.
fn hls_abs(base: &str, uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return uri.to_string();
    }
    if let Ok(b) = url::Url::parse(base) {
        if let Ok(u) = b.join(uri) {
            return u.to_string();
        }
    }
    uri.to_string()
}

/// Pick the highest-BANDWIDTH variant from a master playlist.
/// Returns None when this is already a media playlist.
fn hls_pick_variant(text: &str, base: &str) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.starts_with("#EXT-X-STREAM-INF") {
            // Tag form: #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=...
            // (the first kv carries the tag prefix before the colon).
            let bw = t
                .split([':', ',', ' '])
                .find_map(|kv| {
                    let kv = kv.trim();
                    kv.strip_prefix("BANDWIDTH=")
                        .or_else(|| kv.strip_prefix("bandwidth="))
                        .and_then(|v| v.parse::<u64>().ok())
                })
                .unwrap_or(0);
            // Next non-empty, non-comment line is the variant URI.
            let mut uri: Option<String> = None;
            while let Some(n) = lines.next() {
                let n = n.trim();
                if n.is_empty() || n.starts_with('#') {
                    continue;
                }
                uri = Some(n.to_string());
                break;
            }
            if let Some(u) = uri {
                let abs = hls_abs(base, &u);
                if best.as_ref().map(|(b, _)| bw > *b).unwrap_or(true) {
                    best = Some((bw, abs));
                }
            }
        }
    }
    best.map(|(_, u)| u)
}

fn hls_segments(text: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        out.push(hls_abs(base, t));
    }
    out
}

/// Split an EXT-X-KEY attribute list on commas outside quotes.
fn hls_key_attrs(line: &str) -> Vec<(String, String)> {
    let body = line.split_once(':').map(|(_, r)| r).unwrap_or("").trim();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in body.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            ',' if !in_q => {
                if let Some((k, v)) = cur.split_once('=') {
                    out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if let Some((k, v)) = cur.split_once('=') {
        out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
    }
    out
}

fn hls_attr(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

/// One segment's encryption: absolute key URI + optional explicit IV hex.
/// A segment with no entry is plain. Rotation (P3) is just a schedule with
/// more than one distinct URI — supported, not rejected.
#[derive(Debug, Clone)]
struct HlsSegKey {
    uri: String,
    iv_hex: Option<String>,
}

/// Per-segment key schedule from a media playlist: walk lines in order,
/// each `#EXT-X-KEY` sets the key for subsequent segments until the next
/// one (`METHOD=NONE` clears back to plain). URIs are resolved against the
/// playlist URL. Errors on SAMPLE-AES (DRM-ish, never attempted) and
/// unknown methods — but NOT on rotation.
fn hls_key_schedule(text: &str, base: &str) -> Result<Vec<Option<HlsSegKey>>> {
    let mut out: Vec<Option<HlsSegKey>> = Vec::new();
    let mut current: Option<HlsSegKey> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("#EXT-X-KEY") {
            let attrs = hls_key_attrs(t);
            let method = hls_attr(&attrs, "METHOD").unwrap_or_default().to_uppercase();
            if method == "NONE" || method.is_empty() {
                current = None;
                continue;
            }
            if method == "SAMPLE-AES" {
                return Err(anyhow!(
                    "this HLS stream uses SAMPLE-AES encryption — not supported (DRM-adjacent). Use the extension MP4 qualities when the site offers them"
                ));
            }
            if method != "AES-128" {
                return Err(anyhow!("unsupported HLS key method {method}"));
            }
            let uri = hls_attr(&attrs, "URI").unwrap_or_default();
            if uri.is_empty() {
                return Err(anyhow!("HLS AES-128 key has no URI"));
            }
            current = Some(HlsSegKey { uri: hls_abs(base, &uri), iv_hex: hls_attr(&attrs, "IV") });
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        out.push(current.clone());
    }
    Ok(out)
}

/// First `#EXT-X-MAP:URI="…"` init section (fragmented MP4 headers that
/// must lead the assembled stream). `BYTERANGE` sub-ranges are not
/// supported — the whole object is fetched; playlists needing a byte slice
/// of the map are rejected loudly at download time.
fn hls_map_init(text: &str, base: &str) -> Result<Option<String>> {
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with("#EXT-X-MAP:") {
            continue;
        }
        let attrs = hls_key_attrs(t);
        let uri = hls_attr(&attrs, "URI").unwrap_or_default();
        if uri.is_empty() {
            return Err(anyhow!("HLS EXT-X-MAP has no URI — not supported"));
        }
        if hls_attr(&attrs, "BYTERANGE").is_some_and(|b| !b.trim().is_empty()) {
            return Err(anyhow!(
                "this HLS stream needs a byte-range of its init section (EXT-X-MAP BYTERANGE) — not supported yet"
            ));
        }
        return Ok(Some(hls_abs(base, &uri)));
    }
    Ok(None)
}

fn hls_media_sequence(text: &str) -> u64 {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            return rest.trim().parse::<u64>().unwrap_or(0);
        }
    }
    0
}

fn parse_hex_iv(s: &str) -> Option<[u8; 16]> {
    let t = s.trim().strip_prefix("0x").or_else(|| s.trim().strip_prefix("0X")).unwrap_or(s.trim());
    if t.len() != 32 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// AES-128-CBC decrypt (raw blocks, no padding — HLS segments are
/// block-aligned by the packager). Backed by the vendored FIPS-197
/// implementation (`crate::aes128`, zero new dependencies).
fn aes128_cbc_decrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<Vec<u8>> {
    crate::aes128::cbc_decrypt(key, iv, data).map_err(|e| anyhow!("AES: {e}"))
}

/// HLS VOD download (clean-room, no IDM code): fetch playlist, follow the
/// best variant, fetch segments in parallel (4–8), decrypt AES-128 when
/// present (incl. key rotation, one fetch per distinct key URI), prepend
/// the EXT-X-MAP init section when present, resume via a sidecar map,
/// concat in order. Live, oversize, and SAMPLE-AES playlists are rejected
/// with guidance — and DRM (Widevine/FairPlay) is never bypassed.
async fn download_hls(
    playlist_url: &str,
    dest: &PathBuf,
    connections: usize,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
) -> Result<()> {
    const MAX_SEGS: usize = 2000;
    let _job = claim_dest_job(dest).ok_or_else(|| {
        anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
    })?;
    let client = opts.build_client()?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // 1. Master playlist?
    let mut pl_url = playlist_url.to_string();
    let mut text = opts
        .get(&client, &pl_url)
        .send()
        .await
        .map_err(|e| anyhow!("playlist fetch: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("playlist HTTP: {e}"))?
        .text()
        .await
        .map_err(|e| anyhow!("playlist read: {e}"))?;
    if text.contains("#EXT-X-STREAM-INF") {
        pl_url = hls_pick_variant(&text, &pl_url).ok_or_else(|| {
            anyhow!("could not pick a variant from this master playlist — use the extension MP4 qualities")
        })?;
        text = opts
            .get(&client, &pl_url)
            .send()
            .await
            .map_err(|e| anyhow!("variant fetch: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow!("variant HTTP: {e}"))?
            .text()
            .await
            .map_err(|e| anyhow!("variant read: {e}"))?;
    }
    let schedule = hls_key_schedule(&text, &pl_url)?;
    let seq0 = hls_media_sequence(&text);
    let segs = hls_segments(&text, &pl_url);
    if segs.is_empty() {
        return Err(anyhow!("playlist has no media segments — it may be live or expired"));
    }
    if segs.len() > MAX_SEGS {
        return Err(anyhow!(
            "playlist too large ({} segments) — live streams are not supported",
            segs.len()
        ));
    }
    let is_live = !text.contains("#EXT-X-ENDLIST") && text.contains("#EXT-X-MEDIA-SEQUENCE");
    if is_live {
        return Err(anyhow!(
            "this looks like a live stream (no ENDLIST) — live capture is not supported; only finished VOD playlists"
        ));
    }
    // 2. Key bytes for AES-128, including rotations (P3): every distinct
    // key URI is fetched once up front (bounded — a playlist cycling
    // endless keys is live-ish and refused below).
    let mut distinct: Vec<String> = Vec::new();
    for slot in schedule.iter().flatten() {
        if !distinct.contains(&slot.uri) {
            distinct.push(slot.uri.clone());
        }
    }
    const MAX_KEYS: usize = 8;
    if distinct.len() > MAX_KEYS {
        return Err(anyhow!(
            "this HLS stream cycles through {} encryption keys — not supported",
            distinct.len()
        ));
    }
    let mut key_map: std::collections::HashMap<String, [u8; 16]> =
        std::collections::HashMap::new();
    for uri in &distinct {
        let kb = opts
            .get(&client, uri)
            .send()
            .await
            .map_err(|e| anyhow!("key fetch: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow!("key HTTP: {e}"))?
            .bytes()
            .await
            .map_err(|e| anyhow!("key read: {e}"))?;
        if kb.len() != 16 {
            return Err(anyhow!("HLS key is not 16 bytes — refusing"));
        }
        let mut k = [0u8; 16];
        k.copy_from_slice(&kb);
        key_map.insert(uri.clone(), k);
    }
    let key_map = std::sync::Arc::new(key_map);
    let schedule = std::sync::Arc::new(schedule);
    // 2b. Init section (EXT-X-MAP, P3): fetched once up front (fail fast —
    // without track headers the merge would be unplayable) and prepended
    // at merge time. Re-fetched on resume (tiny); counted in progress.
    let init_bytes: Option<Vec<u8>> = match hls_map_init(&text, &pl_url)? {
        Some(u) => {
            let blob = opts
                .get(&client, &u)
                .send()
                .await
                .map_err(|e| anyhow!("init fetch: {e}"))?
                .error_for_status()
                .map_err(|e| anyhow!("init HTTP: {e}"))?
                .bytes()
                .await
                .map_err(|e| anyhow!("init read: {e}"))?;
            if blob.is_empty() {
                return Err(anyhow!("HLS init section is empty — refusing"));
            }
            Some(blob.to_vec())
        }
        None => None,
    };
    // 3. Resume map: {playlist, count, done:[idx]}. Entries only count
    // when their part file is still on disk (cleanups must not brick
    // the merge into "segment missing").
    let map_path = dest.with_extension("hlsmap");
    let mut done: std::collections::HashSet<usize> = std::collections::HashSet::new();
    if let Ok(raw) = std::fs::read_to_string(&map_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let same = v.get("playlist").and_then(|x| x.as_str()) == Some(pl_url.as_str())
                && v.get("count").and_then(|x| x.as_u64()) == Some(segs.len() as u64);
            if same {
                if let Some(arr) = v.get("done").and_then(|x| x.as_array()) {
                    for x in arr {
                        if let Some(i) = x.as_u64() {
                            let i = i as usize;
                            if i < segs.len()
                                && dest.with_extension(format!("hls{i}")).exists()
                            {
                                done.insert(i);
                            }
                        }
                    }
                }
            }
        }
    }
    let save_map = |done: &std::collections::HashSet<usize>| {
        let mut v: Vec<u64> = done.iter().map(|i| *i as u64).collect();
        v.sort_unstable();
        let j = serde_json::json!({ "playlist": pl_url, "count": segs.len(), "done": v });
        std::fs::write(&map_path, serde_json::to_string_pretty(&j).unwrap_or_default()).ok();
    };
    // 4. Byte totals via concurrent ranged HEADs (best-effort).
    let parallel = connections.clamp(4, 8);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(parallel));
    {
        let mut total_known: u64 = 0;
        let mut any_known = false;
        let mut probes = Vec::new();
        for s in &segs {
            let client = client.clone();
            let opts = opts.clone();
            let u = s.clone();
            let sem = sem.clone();
            probes.push(tokio::spawn(async move {
                let _p = sem.acquire_owned().await;
                let r = opts
                    .get(&client, &u)
                    .header(reqwest::header::RANGE, "bytes=0-0")
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?;
                let cr = r.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                cr.split('/').next_back()?.parse::<u64>().ok()
            }));
        }
        for h in probes {
            if let Ok(Some(n)) = h.await {
                total_known = total_known.saturating_add(n);
                any_known = true;
            }
        }
        if any_known {
            shared.total.store(total_known, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
    }
    // Progress: one slot per segment.
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        for i in 0..segs.len() {
            totals.push((i as u64, i as u64));
            dones.push(if done.contains(&i) { 1 } else { 0 });
        }
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    // Seed progress from already-resumed segments so the bar doesn't
    // restart at zero (their bytes are on disk, never re-fetched).
    {
        let mut seeded = 0u64;
        for i in done.iter() {
            seeded = seeded.saturating_add(
                std::fs::metadata(dest.with_extension(format!("hls{i}")))
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
        }
        shared.downloaded.store(seeded, Ordering::SeqCst);
    }
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()),
    );
    let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let max_retries = opts.max_retries.max(1);
    let mut handles = Vec::new();
    for (idx, seg_url) in segs.iter().enumerate() {
        if done.contains(&idx) {
            continue;
        }
        let client = client.clone();
        let opts = opts.clone();
        let url = seg_url.clone();
        let part = dest.with_extension(format!("hls{idx}"));
        let shared = shared.clone();
        let limiter = limiter.clone();
        let abort = abort.clone();
        let sem = sem.clone();
        let seg_key = schedule.get(idx).cloned().flatten();
        let key_map = key_map.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.map_err(|e| anyhow!("semaphore: {e}"))?;
            // Per-segment key from the schedule (P3 rotation): explicit IV
            // when the playlist gives one, else the sequence number.
            let kb: Option<[u8; 16]> = match &seg_key {
                Some(k) => Some(key_map.get(&k.uri).copied().ok_or_else(|| {
                    anyhow!("HLS key for segment {idx} was not fetched")
                })?),
                None => None,
            };
            let iv: Option<[u8; 16]> = match (&kb, seg_key) {
                (Some(_), Some(k)) => match k.iv_hex {
                    Some(ref hex) => Some(parse_hex_iv(hex).ok_or_else(|| anyhow!("bad EXT-X-IV"))?),
                    None => {
                        let seq = (seq0.saturating_add(idx as u64)) as u128;
                        Some(seq.to_be_bytes())
                    }
                },
                _ => None,
            };
            let mut attempt = 0u32;
            loop {
                attempt += 1;
                if abort.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                wait_if_paused(&shared).await?;
                if shared.cancelled.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                // Bytes added below belong to THIS attempt; a mid-stream
                // failure rolls them back so progress never overcounts.
                let mut attempt_bytes: u64 = 0;
                let got = opts.get(&client, &url).send().await;
                let data: Vec<u8> = match got {
                    Ok(resp) => {
                        let resp = match resp.error_for_status() {
                            Ok(r) => r,
                            Err(e) => {
                                let m = http_status_err(&url, e);
                                if is_final_http_error(&m) {
                                    abort.store(true, Ordering::SeqCst);
                                    return Err(m);
                                }
                                if attempt >= max_retries {
                                    abort.store(true, Ordering::SeqCst);
                                    return Err(m);
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                                continue;
                            }
                        };
                        use futures_util::StreamExt;
                        let expect_len = resp.content_length();
                        let mut buf = Vec::new();
                        let mut stream = resp.bytes_stream();
                        let mut bad: Option<anyhow::Error> = None;
                        while let Some(chunk) = stream.next().await {
                            if abort.load(Ordering::SeqCst) {
                                sub_saturating(&shared.downloaded, attempt_bytes);
                                return Err(anyhow!("cancelled"));
                            }
                            match chunk {
                                Ok(bytes) => {
                                    buf.extend_from_slice(&bytes);
                                    attempt_bytes += bytes.len() as u64;
                                    shared.downloaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
                                    limiter.consume(bytes.len() as u64).await;
                                }
                                Err(e) => {
                                    bad = Some(anyhow!("segment network: {e}"));
                                    break;
                                }
                            }
                        }
                        if let Some(e) = bad {
                            sub_saturating(&shared.downloaded, attempt_bytes);
                            if attempt >= max_retries {
                                abort.store(true, Ordering::SeqCst);
                                return Err(e);
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                            continue;
                        }
                        // A clean-but-short socket close yields no error —
                        // enforce the advertised length when the server gave one.
                        if let Some(n) = expect_len {
                            if buf.len() as u64 != n {
                                sub_saturating(&shared.downloaded, attempt_bytes);
                                if attempt >= max_retries {
                                    abort.store(true, Ordering::SeqCst);
                                    return Err(anyhow!(
                                        "segment short: got {} of {n} bytes", buf.len()
                                    ));
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                                continue;
                            }
                        }
                        buf
                    }
                    Err(e) => {
                        if attempt >= max_retries {
                            abort.store(true, Ordering::SeqCst);
                            return Err(anyhow!("segment fetch: {e}"));
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                        continue;
                    }
                };
                // Decrypt (AES-128) then store the segment file. A bad key
                // fails EVERY segment, so stop the siblings immediately.
                let plain = match (&kb, iv) {
                    (Some(k), Some(v)) => match aes128_cbc_decrypt(&data, k, &v) {
                        Ok(p) => p,
                        Err(e) => {
                            abort.store(true, Ordering::SeqCst);
                            return Err(e);
                        }
                    },
                    _ => data,
                };
                tokio::fs::write(&part, &plain).await.map_err(|e| anyhow!(io_message(&e)))?;
                if let Ok(mut g) = shared.part_done.lock() {
                    if let Some(slot) = g.get_mut(idx) {
                        *slot = 1;
                    }
                }
                return Ok::<(), anyhow::Error>(());
            }
        }));
    }
    let mut first_err: Option<anyhow::Error> = None;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(anyhow!("segment task: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    // Rebuild the full done set, merge in order, clean up.
    for i in 0..segs.len() {
        done.insert(i);
    }
    save_map(&done);
    let tmp = dest.with_extension("dcpart");
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .await?;
    {
        use tokio::io::AsyncWriteExt;
        // EXT-X-MAP init section leads the stream (P3); counted in
        // progress so the bar stays honest.
        if let Some(init) = &init_bytes {
            out.write_all(init).await.map_err(|e| anyhow!(io_message(&e)))?;
            shared.downloaded.fetch_add(init.len() as u64, Ordering::SeqCst);
            shared.total.fetch_add(init.len() as u64, Ordering::SeqCst);
            limiter.consume(init.len() as u64).await;
        }
        for i in 0..segs.len() {
            let part = dest.with_extension(format!("hls{i}"));
            let mut f = tokio::fs::File::open(&part)
                .await
                .map_err(|_| anyhow!("segment {i} missing after download — retry the download"))?;
            tokio::io::copy(&mut f, &mut out).await?;
            let _ = tokio::fs::remove_file(&part).await;
        }
        out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    }
    drop(out);
    let _ = std::fs::remove_file(&map_path);
    let final_len = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// Entry point used by the UI. Downloads `url` to `dest` with N connections.
/// If `url` is an HTML page (e.g. YouTube), a direct media stream is
/// auto-detected first, like the video capture in classic download managers.
/// Plain (non-encrypted) HLS VOD playlists (.m3u8) are assembled
/// segment-by-segment; DASH (.mpd) likewise; `ftp://` URLs go to the
/// plain-FTP backend.
pub async fn download(
    mut url: String,
    mut dest: PathBuf,
    connections: usize,
    mut opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    // Facebook slice URLs (bytestart/byteend) can only ever return that
    // preview slice: normalize to the full-file URL before any probe, and
    // drop the stale slice prefix (it starts at ~byte 967, not 0, so it
    // can never be resumed into a full file).
    let normalized = normalize_media_url(&url);
    if normalized != url {
        url = normalized;
        let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
        let _ = tokio::fs::remove_file(dest.with_extension("dcmeta")).await;
        for i in 0..16 {
            let _ = tokio::fs::remove_file(dest.with_extension(format!("dcpart{i}"))).await;
        }
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    // Route playlists before the progressive path so we never save a
    // playlist file as if it were the video.
    if url.to_ascii_lowercase().starts_with("ftp://") || url.to_ascii_lowercase().starts_with("ftps://") {
        // Plain-FTP backend (P3): own connection, same resume contract.
        let _job = claim_dest_job(&dest).ok_or_else(|| {
            anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
        })?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
            let resumable_here = dest.with_extension("dcpart").exists();
            if !resumable_here {
                dest = crate::model::conflict_free_dest(&dest);
                if let Some(name) = dest
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                {
                    mutex_guard(&shared.resolved_name).replace(name);
                }
            }
        } else if opts.overwrite_existing && dest.exists() {
            let _ = tokio::fs::remove_file(&dest).await;
        }
        let limiter = std::sync::Arc::new(
            RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()),
        );
        return crate::ftp::download_ftp(&url, &dest, &opts, shared, limiter).await;
    }
    if is_playlist_url(&url) {
        if url.to_ascii_lowercase().split(['?', '#']).next().unwrap_or("").ends_with(".mpd")
            || url.contains(".mpd")
        {
            return download_dash(&url, dest, connections, opts, shared).await;
        }
        return download_hls(&url, &dest, connections, &opts, &shared).await;
    }
    // Same-destination jobs would interleave part files: serialize them.
    let _job = claim_dest_job(&dest).ok_or_else(|| {
        anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
    })?;
    let client = opts.build_client()?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Direct media delivery hosts (NOT watch/reel pages): when the total is
    // already known (browser-measured size or a previous resume), verify
    // liveness + range support with ONE cheap Range probe instead of the
    // full HEAD + Range probe. On expiring signed links every extra request
    // burns freshness — but zero verification is worse: a dead/expired URL
    // would burn ~100 futile chunks that each return zero bytes
    // (`got 0 of …`). A 206 keeps the seeded total and uses chunked ranges;
    // anything else falls through to the plain probe path below.
    let direct_delivery = {
        let low = url.to_ascii_lowercase();
        low.contains("fbcdn")
            || low.contains("scontent")
            || low.contains("lookaside.")
            || low.contains("fbsbx.com")
            || low.contains("cdninstagram.com")
    };
    let seeded_total = if shared.has_total.load(Ordering::SeqCst) {
        Some(shared.total.load(Ordering::SeqCst))
    } else {
        None
    };
    let mut range_verified: Option<Option<u64>> = None; // Some(x) = probe ran
    if direct_delivery && seeded_total.is_some() {
        match probe_range_once(&client, &opts, &url, seeded_total).await {
            Ok(verified) => range_verified = Some(verified),
            Err(e) => return Err(e), // expired/refused: fail fast, loudly
        }
    }
    let (mut total_opt, mut supports_range, ctype, disp) = match range_verified {
        Some(verified) => (verified.or(seeded_total), verified.is_some(), None, None),
        None => probe(&client, &opts, &url).await,
    };
    // Pre-seeded totals imply resume support until proven otherwise; the
    // first response corrects this (single_stream updates it live).
    shared
        .resumable
        .store(supports_range, Ordering::SeqCst);
    // Documented name priority (P1): server Content-Disposition >
    // resolved media title (page path below) > this probe's disposition >
    // URL fallback (queue/extension name). Here: honor the disposition…
    let probe_is_html = ctype
        .as_deref()
        .map(|c| c.contains("text/html"))
        .unwrap_or(false);
    if opts.allow_rename {
        if let Some(n) = disp {
            dest.set_file_name(&n);
            mutex_guard(&shared.resolved_name).replace(n);
        } else if !probe_is_html {
            // …or at least fix the extension from the MIME type when the
            // URL yielded no usable name (`download_123.bin` or bare
            // `song` + video/mp4 → `song.mp4`).
            if let Some(cur) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
                let fixed = if crate::model::is_fallback_filename(&cur) {
                    ctype
                        .as_deref()
                        .and_then(crate::model::ext_for_mime)
                        .map(|ext| cur.trim_end_matches(".bin").to_string() + ext)
                } else if !cur.contains('.') {
                    ctype
                        .as_deref()
                        .and_then(crate::model::ext_for_mime)
                        .map(|ext| cur.clone() + ext)
                } else {
                    None
                };
                if let Some(fixed) = fixed {
                    dest.set_file_name(&fixed);
                    mutex_guard(&shared.resolved_name).replace(fixed);
                }
            }
        }
    }
    let is_html = ctype
        .as_deref()
        .map(|c| c.contains("text/html"))
        .unwrap_or(false);
    // On YouTube hosts always resolve first: the probe alone is
    // meaningless there (walls, page variants), the page/API decides.
    let is_yt = url.contains("youtube.com") || url.contains("youtu.be");
    if is_html || is_yt {
        let page_url = url.clone();
        match resolve_media(&client, &opts, &url, &shared).await {
            Some(hit) => {
                url = hit.url;
                // media servers expect the page as referer
                if opts.referer.trim().is_empty() {
                    opts.referer = page_url.clone();
                }
                if !hit.title.trim().is_empty() {
                    let name = sanitize_filename(&hit.title);
                    if opts.allow_rename {
                        dest.set_file_name(&name);
                        mutex_guard(&shared.resolved_name).replace(name);
                    }
                }
                // Adaptive video+audio pair (1080p+ menu choice): merge via
                // ffmpeg on completion instead of a small progressive file.
                // Drop our dest claim first: download_adaptive takes its own.
                if let Some(pair) = hit.pair_url.filter(|p| p.starts_with("http")) {
                    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                        eprintln!("[dbg] pair video={}", url);
                        eprintln!("[dbg] pair audio={}", pair);
                    }
                    drop(_job);
                    // YouTube adaptive legs can die mid-file at ~20%:
                    // SABR-gated qualities serve only a ~2 MB preview outside
                    // the browser, and stream URLs expire/session-lock fast
                    // (403). Retry once with a fresh resolve — downgrading to
                    // 720p progressive for gated qualities — instead of
                    // failing at 19-20% and telling the user to click again.
                    match download_adaptive(
                        url.clone(),
                        Some(pair.clone()),
                        dest.clone(),
                        connections,
                        opts.clone(),
                        shared.clone(),
                    )
                    .await
                    {
                        Ok(()) => return Ok(()),
                        Err(e) => {
                            let msg = format!("{e:#}");
                            let sabr = msg.contains("serves only the first");
                            let refused = msg.contains("refused this link (403)")
                                || msg.contains("expired");
                            if !(sabr || refused) {
                                return Err(e);
                            }
                            let mut opts2 = opts.clone();
                            if sabr {
                                opts2.quality_pref = "720p".to_string();
                            }
                            match resolve_media(&client, &opts2, &page_url, &shared).await {
                                Some(hit2) => {
                                    return download_adaptive(
                                        hit2.url,
                                        hit2.pair_url.filter(|p| p.starts_with("http")),
                                        dest,
                                        connections,
                                        opts2,
                                        shared,
                                    )
                                    .await
                                    .map_err(|e2| {
                                        let m2 = format!("{e2:#}");
                                        if sabr && (m2.contains("serves only the first") || m2.contains("refused this link (403)")) {
                                            anyhow!("{m2} (tried 720p fallback automatically — if this persists, open the video in your browser and pick a 720p or below quality row)")
                                        } else {
                                            e2
                                        }
                                    });
                                }
                                None => return Err(e),
                            }
                        }
                    }
                }
                let (t2, r2, ctype2, d2) = probe(&client, &opts, &url).await;
                total_opt = t2;
                supports_range = r2;
                shared.resumable.store(r2, Ordering::SeqCst);
                // A "media" link that answers HTML is a dead end (error page),
                // not a file — fail loudly instead of saving garbage.
                if ctype2
                    .as_deref()
                    .map(|c| c.contains("text/html"))
                    .unwrap_or(false)
                {
                    return Err(anyhow!(
                        "resolved link is a web page, not a file — try the browser extension"
                    ));
                }
                if opts.allow_rename {
                    if let Some(n) = d2 {
                        dest.set_file_name(&n);
                        mutex_guard(&shared.resolved_name).replace(n);
                    }
                }
            }
            None => {
                if shared.rate_limited.load(Ordering::SeqCst) {
                    return Err(anyhow!(
                        "YouTube is rate-limiting this network right now (bot check) — open the video in your browser, click the floating ⬇ Download this video button, and pick a quality there; the app will confirm the download"
                    ));
                }
                return Err(anyhow!(
                    "page has no downloadable video link — open it in your browser and click the floating ⬇ Download this video button, then pick a quality"
                ));
            }
        }
    }
    if let Some(t) = total_opt {
        shared.total.store(t, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }

    // Avoid silently overwriting a finished file: pick `name (1).ext`
    // when the destination already exists and no resume state refers to it.
    // Explicit CLI output paths (allow_rename=false) keep exact semantics.
    // The Add dialog's "overwrite" choice (and redownload flows, which clear
    // state first) replace the file instead.
    if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        let resumable_here = dest.with_extension("dcpart").exists()
            || (0..16).any(|i| dest.with_extension(format!("dcpart{i}")).exists());
        if !resumable_here {
            dest = crate::model::conflict_free_dest(&dest);
            if let Some(name) = dest
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
            {
                mutex_guard(&shared.resolved_name).replace(name);
            }
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }

    let conns = connections.clamp(1, 16);
    // Facebook/Instagram CDNs throttle parallel Range requests (idle-socket
    // drops, short segments, 403/416 on later ranges). Start those hosts on
    // a single sequential stream instead of burning retries on N-way Ranges.
    // (P2: `effective_conns` shrinks further via halving on cut storms.)
    let fragile_cdn = is_fragile_cdn(&url);
    let mut effective_conns = if fragile_cdn { 1 } else { conns };

    // One shared bucket so N segments together honor the global cap.
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()),
    );
    // Transfer attempts (max 3 legs): fresh-link retry for refused
    // googlevideo URLs, then single-connection fallback for hosts that
    // throttle or botch parallel Range requests (FB/IG CDN class).
    let mut retried_fresh = false;
    let mut fell_back_single = fragile_cdn;
    let mut last_segmented = false;
    loop {
        // P-speed: size googlevideo splits to the strict ~2 MiB window so
        // parallel ranges 206 instead of 403ing into the fallback.
        let mut seg_parts = effective_conns;
        if is_googlevideo_url(&url) {
            if let Some(t) = total_opt {
                seg_parts = google_parts(t, effective_conns);
            }
        }
        let use_segmented =
            supports_range && matches!(total_opt, Some(t) if t >= 1024 * 1024) && seg_parts > 1 && !fell_back_single;
        last_segmented = use_segmented;
        let r = if use_segmented {
            segmented(
                &client,
                &opts,
                &url,
                &dest,
                total_opt.unwrap(),
                seg_parts,
                shared.clone(),
                limiter.clone(),
            )
            .await
        } else if (fragile_cdn || is_googlevideo_url(&url)) && supports_range && matches!(total_opt, Some(t) if t > 0) {
            // Small bounded ranges only (see chunked_stream): throttling
            // signed CDNs cut big streams, and strict googlevideo edges
            // 403 open-ended, HEAD, and >~2 MiB ranges outright (verified
            // live) while serving identical 1 MiB windows with 206.
            // (Unverified/ignored ranges go single-stream below.)
            chunked_stream(
                &client,
                &opts,
                &url,
                &dest,
                total_opt.unwrap(),
                shared.clone(),
                limiter.clone(),
            )
            .await
        } else {
            single_stream(&client, &opts, &url, &dest, shared.clone(), limiter.clone()).await
        };
        match r {
            Ok(()) => break,
            Err(e)
                if !retried_fresh
                    && is_final_http_error(&e)
                    && is_googlevideo_url(&url)
                    && is_youtube_page(&opts.referer) =>
            {
                retried_fresh = true;
                match resolve_media(&client, &opts, &opts.referer, &shared).await {
                    Some(hit) if hit.url != url && is_googlevideo_url(&hit.url) => {
                        // Drop partials of the refused stream: resuming them
                        // against a different URL would corrupt the file.
                        // (The resume sidecar is dropped too; the new URL
                        // rebinds it on first write.)
                        let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
                        let _ = tokio::fs::remove_file(dest.with_extension("dcmeta")).await;
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        shared.downloaded.store(0, Ordering::SeqCst);
                        url = hit.url;
                        let (t3, r3, _, _) = probe(&client, &opts, &url).await;
                        total_opt = t3;
                        supports_range = r3;
                        shared.resumable.store(r3, Ordering::SeqCst);
                        if let Some(t) = total_opt {
                            shared.total.store(t, Ordering::SeqCst);
                            shared.has_total.store(true, Ordering::SeqCst);
                        } else {
                            shared.has_total.store(false, Ordering::SeqCst);
                        }
                        continue;
                    }
                    _ => {
                        return Err(anyhow!(
                            "{e:#} (a fresh link was tried and also refused — replay the video in your browser and pick a new quality row)"
                        ));
                    }
                }
            }
            Err(e)
                if !fell_back_single
                    && last_segmented
                    && (is_final_http_error(&e)
                        || segmented_throttled(&e.to_string())) =>
            {
                // Parallel Ranges defeated (see seg_fallback): the resume
                // binding drops stale part sets on every re-split, and the
                // progress bar holds its max meanwhile.
                let chunked_ok = (fragile_cdn || is_googlevideo_url(&url))
                    && supports_range
                    && matches!(total_opt, Some(t) if t > 0);
                match seg_fallback(is_final_http_error(&e), effective_conns, chunked_ok) {
                    SegFallback::Halve(h) => {
                        effective_conns = h;
                        continue;
                    }
                    SegFallback::Chunked => {
                        fell_back_single = true;
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        continue;
                    }
                    SegFallback::Single => {
                        fell_back_single = true;
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        supports_range = false;
                        shared.resumable.store(false, Ordering::SeqCst);
                        continue;
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    // Integrity gate: a mismatched file is deleted and reported as failure
    // rather than left on disk looking complete.
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// Locate `ffmpeg` on PATH (with `.exe` on Windows). `None` = not installed.
pub fn ffmpeg_path() -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe", "ffmpeg"]
    } else {
        &["ffmpeg"]
    };
    let Ok(path_var) = std::env::var("PATH") else {
        return None;
    };
    for dir in std::env::split_paths(&path_var) {
        for n in names {
            let cand = dir.join(n);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Mux downloaded video (+ optional audio) streams into one file without
/// re-encoding (`-c copy`). Used for adaptive video-only + audio pairs and
/// DASH representations — the same job IDM's merger does.
pub async fn ffmpeg_mux(
    video: &std::path::Path,
    audio: Option<&std::path::Path>,
    dest: &PathBuf,
) -> Result<()> {
    let ff = ffmpeg_path().ok_or_else(|| {
        anyhow!(
            "ffmpeg not found — install it (https://ffmpeg.org/download.html, or `winget install ffmpeg`) and put it on PATH to merge video+audio qualities; progressive ≤720p rows download as single files without it"
        )
    })?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut cmd = tokio::process::Command::new(ff);
    #[cfg(windows)]
    {
        // No console flash for the helper process.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.arg("-y").arg("-i").arg(video);
    if let Some(a) = audio {
        cmd.arg("-i").arg(a);
    }
    cmd.arg("-c").arg("copy");
    // Fast-start for MP4 so playback begins before the full index loads.
    if dest.extension().map(|e| e == "mp4").unwrap_or(false) {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg(dest);
    let out = cmd.output().await.map_err(|e| anyhow!("ffmpeg launch: {e}"))?;
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr);
        let tail: String = tail.chars().rev().take(400).collect::<String>().chars().rev().collect();
        return Err(anyhow!("ffmpeg merge failed: {}", tail.trim()));
    }
    Ok(())
}

/// One adaptive leg (P-speed): segmented parallel first when the server
/// honors ranges (sized to the strict window on googlevideo), halving on
/// throttled splits, then sequential chunked 1 MiB windows on
/// cut-happy/throttling hosts, then a plain stream. `label` ("video" /
/// "audio") names the leg in errors, since two concurrent transfers would
/// otherwise hide which one refused.
#[allow(clippy::too_many_arguments)]
async fn adaptive_leg(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    leg_dest: &PathBuf,
    total: Option<u64>,
    range: bool,
    conns: usize,
    sub: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
    label: &str,
) -> Result<()> {
    let strict = is_fragile_cdn(url) || is_googlevideo_url(url);
    let can_chunk = strict && matches!(total, Some(t) if t > 0);
    let mut parts = conns.clamp(1, 16);
    if is_googlevideo_url(url) {
        if let Some(t) = total {
            parts = google_parts(t, parts);
        }
    }
    let can_segment = range && matches!(total, Some(t) if t >= 1024 * 1024) && parts > 1;
    if can_segment {
        // Legs start at zero (fresh work dir), so the counter holds only
        // this attempt's bytes — safe to retry the split downward.
        let mut n = parts;
        loop {
            match segmented(client, opts, url, leg_dest, total.unwrap(), n, sub.clone(), limiter.clone()).await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if !is_final_http_error(&e) {
                        if let Some(h) = halve_conns(n) {
                            n = h;
                            continue;
                        }
                    }
                    if can_chunk {
                        // Stale parts belong to the failed split; the
                        // chunked round rebinds its own offset, and the
                        // counter restarts with it (never double-counts).
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                leg_dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        sub.downloaded.store(0, Ordering::SeqCst);
                        return chunked_stream(
                            client,
                            opts,
                            url,
                            leg_dest,
                            total.unwrap(),
                            sub,
                            limiter,
                        )
                        .await
                        .map_err(|e| anyhow!("{label} leg: {e:#}"));
                    }
                    return Err(anyhow!("{label} leg: {e:#}"));
                }
            }
        }
    }
    if can_chunk {
        return chunked_stream(client, opts, url, leg_dest, total.unwrap(), sub, limiter)
            .await
            .map_err(|e| anyhow!("{label} leg: {e:#}"));
    }
    single_stream(client, opts, url, leg_dest, sub, limiter)
        .await
        .map_err(|e| anyhow!("{label} leg: {e:#}"))
}

/// Download an adaptive pair (video-only + audio) concurrently, then mux to
/// one file. Container follows the codecs: MP4/M4A pairs stay `.mp4`,
/// anything WebM/Opus becomes `.mkv` (same convention download managers use).
/// Without `audio` this is a plain direct download through the same path.
pub async fn download_adaptive(
    video_url: String,
    audio_url: Option<String>,
    mut dest: PathBuf,
    connections: usize,
    opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    let video_url = normalize_media_url(&video_url);
    let audio_url = audio_url.map(|u| normalize_media_url(&u));
    let _job = claim_dest_job(&dest).ok_or_else(|| {
        anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
    })?;
    // Merging needs ffmpeg: fail BEFORE any bytes move, not after two
    // quarter-gigabyte legs finish. Progressive (single-file) qualities
    // never reach this path, so this only gates real merges.
    if audio_url.as_deref().map(|u| u.starts_with("http")).unwrap_or(false)
        && ffmpeg_path().is_none()
    {
        return Err(anyhow!(
            "this quality needs merging (video-only + audio) but ffmpeg was not found — install it (`winget install ffmpeg`), restart the app so it lands on PATH, then resume; 720p and below download as single files without it"
        ));
    }
    let client = opts.build_client()?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Probe both legs (referer/cookies apply — media servers expect them).
    let (v_total, v_range, vtype, v_disp) = probe(&client, &opts, &video_url).await;
    shared.resumable.store(v_range, Ordering::SeqCst);
    if opts.allow_rename {
        if let Some(n) = v_disp {
            dest.set_file_name(&n);
            mutex_guard(&shared.resolved_name).replace(n);
        }
    }
    let (a_total, a_range, atype, _) = match &audio_url {
        Some(a) => {
            let (t, r, c, _) = probe(&client, &opts, a).await;
            (t, r, c, ())
        }
        None => (None, false, None, ()),
    };
    // Container: WebM/Opus legs mux to MKV, MP4/M4A legs stay MP4.
    let webmish = vtype
        .as_deref()
        .map(|c| c.contains("webm") || c.contains("vp9") || c.contains("av01"))
        .unwrap_or(false)
        || atype
            .as_deref()
            .map(|c| c.contains("webm") || c.contains("opus"))
            .unwrap_or(false)
        || video_url.to_ascii_lowercase().contains(".webm")
        || audio_url
            .as_deref()
            .map(|u| u.to_ascii_lowercase().contains(".webm"))
            .unwrap_or(false);
    if audio_url.is_some() && webmish && opts.allow_rename {
        dest.set_extension("mkv");
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    }
    if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        dest = crate::model::conflict_free_dest(&dest);
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    let grand_total = v_total.unwrap_or(0).saturating_add(a_total.unwrap_or(0));
    if grand_total > 0 {
        shared.total.store(grand_total, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, grand_total));
        dones.push(0);
    }
    // Work files live in a unique sidecar dir so concurrent jobs and
    // retries never share part files.
    let work = unique_work_dir(&dest);
    tokio::fs::create_dir_all(&work).await?;
    let v_dest = work.join("v");
    let a_dest = work.join("a");
    let conns = connections.clamp(1, 16);
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()),
    );
    let sv = Arc::new(SharedState::new());
    let sa = Arc::new(SharedState::new());
    let v_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let has_audio = audio_url.is_some();
    let fut_v = {
        let client = client.clone();
        let opts = opts.clone();
        let limiter = limiter.clone();
        let sub = sv.clone();
        let done_flag = v_done.clone();
        let vd = v_dest.clone();
        async move {
            let r = adaptive_leg(
                &client, &opts, &video_url, &vd, v_total, v_range, conns, sub, limiter, "video",
            )
            .await;
            done_flag.store(true, Ordering::SeqCst);
            r
        }
    };
    let fut_a = {
        let client = client.clone();
        let opts = opts.clone();
        let limiter = limiter.clone();
        let sub = sa.clone();
        let done_flag = a_done.clone();
        let ad = a_dest.clone();
        let au = audio_url.clone();
        async move {
            let r = match au {
                Some(a) => {
                    adaptive_leg(&client, &opts, &a, &ad, a_total, a_range, conns, sub, limiter, "audio").await
                }
                None => Ok(()),
            };
            done_flag.store(true, Ordering::SeqCst);
            r
        }
    };
    let agg = async {
        loop {
            let sum = sv.downloaded.load(Ordering::SeqCst)
                .saturating_add(sa.downloaded.load(Ordering::SeqCst));
            shared.downloaded.store(sum, Ordering::SeqCst);
            if v_done.load(Ordering::SeqCst) && a_done.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        let sum = sv.downloaded.load(Ordering::SeqCst)
            .saturating_add(sa.downloaded.load(Ordering::SeqCst));
        shared.downloaded.store(sum, Ordering::SeqCst);
    };
    let (rv, ra, _) = tokio::join!(fut_v, fut_a, agg);
    // Leg failures must also drop the work dir (unique per job, but litter
    // accumulates across retries otherwise). The aggregated byte counter is
    // reset too: those leg bytes were deleted with the work dir, so keeping
    // them would persist phantom progress (bar jumps backwards on retry,
    // e.g. 31% -> 20%).
    if let Err(e) = rv {
        let _ = tokio::fs::remove_dir_all(&work).await;
        shared.downloaded.store(0, Ordering::SeqCst);
        return Err(e);
    }
    if let Err(e) = ra {
        let _ = tokio::fs::remove_dir_all(&work).await;
        shared.downloaded.store(0, Ordering::SeqCst);
        return Err(e);
    }
    // Mux (or plain move when audio-less). The unique work dir is removed on
    // every exit so failed merges never poison the next attempt.
    let finish: Result<()> = if has_audio {
        ffmpeg_mux(&v_dest, Some(&a_dest), &dest).await
    } else {
        tokio::fs::rename(&v_dest, &dest)
            .await
            .map_err(|e| anyhow!(io_message(&e)))
    };
    let _ = tokio::fs::remove_dir_all(&work).await;
    if finish.is_err() {
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    finish?;
    let final_len = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// One DASH Representation (video or audio track option).
#[derive(Debug, Clone, Default)]
struct DashRep {
    id: String,
    bandwidth: u64,
    width: u64,
    height: u64,
    codecs: String,
    mime: String,
    base_url: Option<String>,
    seg_media: Option<String>,
    seg_init: Option<String>,
    start_number: u64,
    /// Segment duration in seconds (duration ÷ timescale), when templated.
    seg_dur_secs: Option<f64>,
    /// Exact segment count from a uniform SegmentTimeline (P3), preferred
    /// over duration math when present.
    timeline_count: Option<usize>,
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    // Find name="value" with exact attribute-name match (avoids
    // `bandwidth` matching inside other names by checking the prefix char).
    let mut search = tag;
    while let Some(p) = search.find(name) {
        let before = search[..p].chars().next_back();
        let ok_before = before.map(|c| c.is_whitespace() || c == '<').unwrap_or(false);
        let after = search[p + name.len()..].chars().next();
        if ok_before && after == Some('=') {
            let rest = search[p + name.len() + 1..].trim_start();
            let q = rest.chars().next()?;
            if q == '"' || q == '\'' {
                let end = rest[1..].find(q)?;
                return Some(rest[1..1 + end].to_string());
            }
        }
        search = &search[p + name.len()..];
    }
    None
}

fn xml_child(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let s = block.find(&open)?;
    let body = block[s..].find('>')? + s + 1;
    let e = block[body..].find(&close)? + body;
    let t = block[body..e].trim().replace("&amp;", "&");
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Seconds from a SegmentTemplate tag's duration/timescale pair.
fn tpl_secs(tag: &str) -> Option<f64> {
    let d: f64 = xml_attr(tag, "duration")?.parse().ok()?;
    let ts: f64 = xml_attr(tag, "timescale")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    if d <= 0.0 || ts <= 0.0 {
        None
    } else {
        Some(d / ts)
    }
}

/// Parse an ISO-8601 MPD duration subset (PT1H2M3.5S) to seconds.
fn parse_pt_duration(s: &str) -> Option<f64> {
    // ISO-8601 subset used by MPDs: PT1H2M3.5S
    let t = s.trim().strip_prefix("PT")?;
    let mut secs = 0.0;
    let mut num = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            let v: f64 = num.parse().ok()?;
            num.clear();
            match c {
                'H' => secs += v * 3600.0,
                'M' => secs += v * 60.0,
                'S' => secs += v,
                _ => return None,
            }
        }
    }
    Some(secs)
}

fn dash_seg_count(mpd_duration: Option<f64>, seg_dur: f64) -> Option<usize> {
    if seg_dur <= 0.0 {
        return None;
    }
    let total = mpd_duration?;
    if total <= 0.0 {
        return None;
    }
    let n = (total / seg_dur).ceil() as usize;
    if n == 0 || n > 2000 {
        None
    } else {
        Some(n)
    }
}

/// Count segments from a `SegmentTimeline` element's `<S>` entries (P3).
/// Only uniform timelines are countable: every entry must share one
/// duration `d` and be contiguous (explicit `t` must match the running
/// time). `r` repeats extend the count; `r="-1"` (repeat to period end)
/// needs a known period duration. `timescale` converts timeline units to
/// seconds. Returns:
/// - `Ok(None)` — no timeline present (caller falls back to
///   duration/timescale math).
/// - `Ok(Some(n))` — exact segment count (capped like `dash_seg_count`).
/// - `Err` — gaps, varying durations, or unbounded repeats: kept loud
///   instead of assembling garbage out of order.
fn timeline_count(block: &str, timescale: f64, period_dur_secs: Option<f64>) -> Result<Option<usize>> {
    // Collect `<S …>` tags only (`<S `, `<S/>`, `<S>` — never
    // `<SegmentTemplate…` / `<SegmentTimeline…` / `<Supplemental…`).
    let mut tags: Vec<&str> = Vec::new();
    let mut search = 0usize;
    while let Some(p) = block[search..].find("<S") {
        let abs = search + p;
        let after = block[abs + 2..].chars().next();
        match after {
            Some(' ') | Some('/') | Some('>') => {
                let end = block[abs..].find('>').map(|e| abs + e).unwrap_or(block.len());
                tags.push(&block[abs..end.min(block.len())]);
                search = abs + 2;
            }
            _ => search = abs + 2,
        }
    }
    if tags.is_empty() {
        return Ok(None);
    }
    let mut unit_d: Option<u64> = None;
    let mut pos: Option<u64> = None; // running time in timeline units
    let mut total: usize = 0;
    for tag in &tags {
        let d: u64 = xml_attr(tag, "d")
            .and_then(|v| v.parse().ok())
            .filter(|d| *d > 0)
            .ok_or_else(|| {
                anyhow!("DASH SegmentTimeline entry without a positive duration is not supported")
            })?;
        if let Some(u) = unit_d {
            if u != d {
                return Err(anyhow!(
                    "this DASH manifest uses a varying-duration SegmentTimeline — not supported yet. Use the browser extension qualities (MP4) when the site offers them"
                ));
            }
        } else {
            unit_d = Some(d);
        }
        if let Some(t) = xml_attr(tag, "t").and_then(|v| v.parse::<u64>().ok()) {
            if let Some(p) = pos {
                if t != p {
                    return Err(anyhow!(
                        "this DASH manifest has gaps in its SegmentTimeline — not supported yet"
                    ));
                }
            }
            pos = Some(t);
        }
        let r_raw = xml_attr(tag, "r").unwrap_or_else(|| "0".to_string());
        let r: i64 = r_raw.parse().unwrap_or(0);
        let reps: u64 = if r < 0 {
            // Repeat to period end (DASH-IF `r="-1"`).
            let period = period_dur_secs.ok_or_else(|| {
                anyhow!("this DASH manifest repeats segments to an unknown period end — not supported yet")
            })?;
            let ts = if timescale > 0.0 { timescale } else { 1.0 };
            let seg_secs = d as f64 / ts;
            if seg_secs <= 0.0 {
                return Err(anyhow!("DASH SegmentTimeline entry without a positive duration is not supported"));
            }
            let elapsed = pos.unwrap_or(0) as f64 / ts;
            let n = ((period - elapsed) / seg_secs).ceil().max(1.0) as u64;
            pos = None; // nothing verifiable follows an open repeat
            n
        } else {
            let n = (r as u64).saturating_add(1);
            pos = Some(pos.unwrap_or(0).saturating_add(d.saturating_mul(n)));
            n
        };
        total = total.saturating_add(reps as usize);
        if total > 2000 {
            return Ok(None); // oversize/live-ish: caller reports it cleanly
        }
    }
    if total == 0 {
        return Ok(None);
    }
    Ok(Some(total))
}

fn dash_expand(tpl: &str, number: u64, rep_id: &str, bw: u64) -> String {
    // $Number$, $Number%0Nd$, $RepresentationID$, $Bandwidth$.
    // Byte-walk would corrupt multibyte template text, so iterate chars.
    let mut out = String::new();
    let chars: Vec<char> = tpl.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '$' {
                j += 1;
            }
            if j < chars.len() {
                let tok: String = chars[i + 1..j].iter().collect();
                if tok == "Number" {
                    out.push_str(&number.to_string());
                } else if let Some(fmt) = tok.strip_prefix("Number%0") {
                    if let Some(w) = fmt.strip_suffix('d').and_then(|x| x.parse::<usize>().ok()) {
                        out.push_str(&format!("{number:0>w$}"));
                    } else {
                        out.push_str(&number.to_string());
                    }
                } else if tok == "RepresentationID" {
                    out.push_str(rep_id);
                } else if tok == "Bandwidth" {
                    out.push_str(&bw.to_string());
                } else if tok == "$" {
                    out.push('$');
                } else {
                    out.push('$');
                    out.push_str(&tok);
                    out.push('$');
                }
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Parse an MPD manifest into (video, audio) representation lists.
/// Handles AdaptationSet mimeType + Representation bandwidth/width/height +
/// BaseURL and SegmentTemplate (number-based). Uniform SegmentTimelines are
/// counted exactly (`timeline_count`); varying/gapped timelines and dynamic
/// (live) manifests are rejected by the caller.
fn parse_mpd(xml: &str) -> Result<(Vec<DashRep>, Vec<DashRep>)> {
    // Period duration for open-ended (`r="-1"`) timelines.
    let mpd_dur = xml
        .find("<MPD")
        .and_then(|p| {
            let q = xml[p..].find('>').map(|x| x + p).unwrap_or(xml.len());
            xml_attr(&xml[p..q.min(xml.len())], "mediaPresentationDuration")
        })
        .and_then(|d| parse_pt_duration(&d));
    let mut videos = Vec::new();
    let mut audios = Vec::new();
    let mut rest = xml;
    let mut found_set = false;
    while let Some(s) = rest.find("<AdaptationSet") {
        found_set = true;
        let body_start = rest[s..].find('>').map(|x| x + s + 1).unwrap_or(rest.len());
        let e = rest[body_start..].find("</AdaptationSet>").map(|x| x + body_start).unwrap_or(rest.len());
        let block = &rest[body_start..e.min(rest.len())];
        let open_tag = &rest[s..body_start.min(rest.len())];
        let mime = xml_attr(open_tag, "mimeType").unwrap_or_default();
        let set_tpl_media;
        let set_tpl_init;
        let set_start;
        let set_dur;
        let set_timescale: f64;
        if let Some(st) = block.find("<SegmentTemplate") {
            let st_end = block[st..].find('>').map(|x| x + st).unwrap_or(block.len());
            let st_tag_full = &block[st..st_end.min(block.len())];
            // SegmentTemplate may be self-closed or wrap an explicit
            // Initialization element; sniff both.
            set_tpl_media = xml_attr(st_tag_full, "media");
            set_tpl_init = xml_attr(st_tag_full, "initialization").or_else(|| {
                let r = &block[st..];
                r.find("<Initialization").and_then(|p| {
                    let q = r[p..].find('>').map(|x| x + p).unwrap_or(r.len());
                    xml_attr(&r[p..q.min(r.len())], "sourceURL")
                })
            });
            set_start = block[st..]
                .find("startNumber")
                .and_then(|_| xml_attr(st_tag_full, "startNumber"))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1);
            set_dur = tpl_secs(st_tag_full);
            set_timescale = xml_attr(st_tag_full, "timescale")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|t| *t > 0.0)
                .unwrap_or(1.0);
        } else {
            set_tpl_media = None;
            set_tpl_init = None;
            set_start = 1;
            set_dur = None;
            set_timescale = 1.0;
        }
        // Set-level timeline scan, excluding Representation sub-blocks
        // (their own timelines are evaluated per-rep below).
        let mut set_scan = String::with_capacity(block.len());
        {
            let mut pos = 0usize;
            let mut rsearch = 0usize;
            while let Some(r) = block[rsearch..].find("<Representation") {
                let rs = rsearch + r;
                let r_body = block[rs..].find('>').map(|x| x + rs + 1).unwrap_or(block.len());
                let r_end = block[r_body..]
                    .find("</Representation>")
                    .map(|x| x + r_body + "</Representation>".len())
                    .unwrap_or(block.len());
                set_scan.push_str(&block[pos..rs.min(block.len())]);
                pos = r_end.min(block.len());
                rsearch = pos;
            }
            set_scan.push_str(&block[pos..]);
        }
        // A malformed timeline is a loud error, not a silent miscount —
        // but only when a timeline actually exists here.
        let set_tl = if set_scan.contains("<SegmentTimeline") {
            timeline_count(&set_scan, set_timescale, mpd_dur)?
        } else {
            None
        };
        let mut rrest = block;
        while let Some(r) = rrest.find("<Representation") {
            let r_body = rrest[r..].find('>').map(|x| x + r + 1).unwrap_or(rrest.len());
            let r_end = rrest[r_body..].find("</Representation>").map(|x| x + r_body).unwrap_or(rrest.len());
            let rblock = &rrest[r_body..r_end.min(rrest.len())];
            let rtag = &rrest[r..r_body.min(rrest.len())];
            let rep_tpl_tag = rblock.find("<SegmentTemplate").map(|p| {
                let q = rblock[p..].find('>').map(|x| x + p).unwrap_or(rblock.len());
                rblock[p..q.min(rblock.len())].to_string()
            });
            let rep_start = rep_tpl_tag
                .as_ref()
                .and_then(|t| xml_attr(t, "startNumber"))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(set_start);
            let rep_timescale = rep_tpl_tag
                .as_ref()
                .and_then(|t| xml_attr(t, "timescale"))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|t| *t > 0.0)
                .unwrap_or(set_timescale);
            // Rep-level timeline wins; otherwise the set-level one applies.
            let rep_tl = if rblock.contains("<SegmentTimeline") {
                timeline_count(rblock, rep_timescale, mpd_dur)?
            } else {
                None
            };
            let rep = DashRep {
                id: xml_attr(rtag, "id").unwrap_or_default(),
                bandwidth: xml_attr(rtag, "bandwidth").and_then(|v| v.parse().ok()).unwrap_or(0),
                width: xml_attr(rtag, "width").and_then(|v| v.parse().ok()).unwrap_or(0),
                height: xml_attr(rtag, "height").and_then(|v| v.parse().ok()).unwrap_or(0),
                codecs: xml_attr(rtag, "codecs").unwrap_or_default(),
                mime: xml_attr(rtag, "mimeType").unwrap_or_else(|| mime.clone()),
                base_url: xml_child(rblock, "BaseURL").or_else(|| xml_child(block, "BaseURL")),
                seg_media: rep_tpl_tag
                    .as_ref()
                    .and_then(|t| xml_attr(t, "media"))
                    .or_else(|| set_tpl_media.clone()),
                seg_init: rep_tpl_tag
                    .as_ref()
                    .and_then(|t| xml_attr(t, "initialization"))
                    .or_else(|| set_tpl_init.clone()),
                start_number: rep_start,
                seg_dur_secs: rep_tpl_tag
                    .as_ref()
                    .and_then(|t| tpl_secs(t))
                    .or(set_dur),
                timeline_count: rep_tl.or(set_tl),
            };
            if rep.mime.starts_with("video/") {
                videos.push(rep);
            } else if rep.mime.starts_with("audio/") {
                audios.push(rep);
            }
            rrest = &rrest[r + "<Representation".len()..];
        }
        rest = &rest[s + "<AdaptationSet".len()..];
    }
    if !found_set {
        return Err(anyhow!("no DASH adaptation sets found in this manifest"));
    }
    if videos.is_empty() && audios.is_empty() {
        return Err(anyhow!("DASH manifest has no video/audio tracks"));
    }
    Ok((videos, audios))
}

/// One DASH blob fetch with retries (init or media segment). Standalone so
/// media segments can be fetched from spawned tasks (P2 parallelism).
async fn dash_fetch_blob(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: String,
    shared: &Arc<SharedState>,
    max_retries: u32,
) -> Result<Vec<u8>> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        wait_if_paused(shared).await?;
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        match opts.get(client, &url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(r) => {
                    let expect = r.content_length();
                    let body = r.bytes().await.map_err(|e| anyhow!("segment network: {e}"))?;
                    // Same clean-but-short guard as HLS segments.
                    if let Some(n) = expect {
                        if body.len() as u64 != n {
                            if attempt >= max_retries {
                                return Err(anyhow!(
                                    "segment short: got {} of {n} bytes", body.len()
                                ));
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                            continue;
                        }
                    }
                    return Ok(body.to_vec());
                }
                Err(e) => {
                    let m = http_status_err(&url, e);
                    if is_final_http_error(&m) || attempt >= max_retries {
                        return Err(m);
                    }
                }
            },
            Err(e) => {
                if attempt >= max_retries {
                    return Err(anyhow!("segment fetch: {e}"));
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
    }
}

/// Download one DASH representation to a file: init segment + N media
/// segments concatenated (fragmented MP4 chains this way for VOD). Media
/// segments fetch in bounded parallel (P2, like HLS) into indexed part
/// files and concatenate in order; the init segment stays sequential
/// (headers must lead the stream).
#[allow(clippy::too_many_arguments)]
async fn dash_rep_download(
    client: &reqwest::Client,
    opts: &EngineOpts,
    mpd_url: &str,
    rep: &DashRep,
    rep_id: &str,
    count: Option<usize>,
    dest: &PathBuf,
    shared: &Arc<SharedState>,
    limiter: &std::sync::Arc<RateLimiter>,
    parallel: usize,
) -> Result<()> {
    let max_retries = opts.max_retries.max(1);
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .await?;
    use tokio::io::AsyncWriteExt;
    if rep.seg_media.is_some() {
        // Templated representation: initialization segment (track headers)
        // first; the numbered media segments follow below.
        if let Some(init_tpl) = rep.seg_init.clone() {
            let u = if init_tpl.starts_with("http") {
                init_tpl
            } else {
                hls_abs(mpd_url, &init_tpl)
            };
            let blob = dash_fetch_blob(client, opts, u, shared, max_retries).await?;
            out.write_all(&blob).await.map_err(|e| anyhow!(io_message(&e)))?;
            shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
            limiter.consume(blob.len() as u64).await;
        }
    }
    if let Some(media_tpl) = rep.seg_media.clone() {
        let n = count.ok_or_else(|| {
            anyhow!("DASH live/dynamic manifests without a known segment count are not supported")
        })?;
        // Bounded parallel fetch (P2): each segment lands in its indexed
        // part file; concatenation below stays strictly ordered.
        let slots = parallel.clamp(2, 8);
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(slots));
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let num = rep.start_number.saturating_add(i as u64);
            let seg = dash_expand(&media_tpl, num, rep_id, rep.bandwidth);
            let u = if seg.starts_with("http") { seg } else { hls_abs(mpd_url, &seg) };
            let part = dest.with_extension(format!("dashpart{i}"));
            let client = client.clone();
            let opts = opts.clone();
            let shared = shared.clone();
            let limiter = limiter.clone();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.map_err(|e| anyhow!("semaphore: {e}"))?;
                if shared.cancelled.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                let blob = dash_fetch_blob(&client, &opts, u, &shared, max_retries).await?;
                tokio::fs::write(&part, &blob).await.map_err(|e| anyhow!(io_message(&e)))?;
                shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
                limiter.consume(blob.len() as u64).await;
                Ok::<(), anyhow::Error>(())
            }));
        }
        let mut first_err: Option<anyhow::Error> = None;
        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(anyhow!("segment task: {e}"));
                    }
                }
            }
        }
        if let Some(e) = first_err {
            for i in 0..n {
                let _ = tokio::fs::remove_file(dest.with_extension(format!("dashpart{i}"))).await;
            }
            return Err(e);
        }
        for i in 0..n {
            let part = dest.with_extension(format!("dashpart{i}"));
            let mut f = tokio::fs::File::open(&part).await.map_err(|_| {
                anyhow!("segment {i} missing after download — retry the download")
            })?;
            tokio::io::copy(&mut f, &mut out).await?;
            let _ = tokio::fs::remove_file(&part).await;
        }
    } else if let Some(b) = rep.base_url.clone() {
        // Single-file representation.
        let u = if b.starts_with("http") { b } else { hls_abs(mpd_url, &b) };
        let blob = dash_fetch_blob(client, opts, u, shared, max_retries).await?;
        out.write_all(&blob).await.map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
        limiter.consume(blob.len() as u64).await;
    } else {
        return Err(anyhow!("DASH representation has neither BaseURL nor SegmentTemplate"));
    }
    out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
}

/// DASH VOD download (clean-room): best video + best audio representations,
/// muxed with ffmpeg (or delivered video-only when ffmpeg is absent and no
/// audio track was needed). DRM (`ContentProtection`) manifests are refused.
pub async fn download_dash(
    manifest_url: &str,
    mut dest: PathBuf,
    connections: usize,
    opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    // Segment fetch parallelism (P2): bounded like HLS legs.
    let parallel = connections.clamp(2, 8);
    let client = opts.build_client()?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let xml = opts
        .get(&client, manifest_url)
        .send()
        .await
        .map_err(|e| anyhow!("manifest fetch: {e}"))?
        .error_for_status()
        .map_err(|e| http_status_err(manifest_url, e))?
        .text()
        .await
        .map_err(|e| anyhow!("manifest read: {e}"))?;
    if xml.contains("<ContentProtection") {
        return Err(anyhow!(
            "this DASH stream is DRM-protected (ContentProtection) — protected streams are never downloaded"
        ));
    }
    if xml.contains("type=\"dynamic\"") || xml.contains("type='dynamic'") {
        return Err(anyhow!(
            "this is a live/dynamic DASH manifest — live capture is not supported; only finished VOD"
        ));
    }
    let mpd_dur = xml
        .find("<MPD")
        .and_then(|p| {
            let q = xml[p..].find('>').map(|x| x + p).unwrap_or(xml.len());
            xml_attr(&xml[p..q.min(xml.len())], "mediaPresentationDuration")
        })
        .and_then(|d| parse_pt_duration(&d));
    let (videos, audios) = parse_mpd(&xml)?;
    // Best video = most pixels (fallback: bandwidth); best audio = bandwidth.
    let v = videos
        .iter()
        .max_by_key(|r| ((r.width as u64).saturating_mul(r.height as u64), r.bandwidth))
        .ok_or_else(|| anyhow!("DASH manifest has no video track"))?
        .clone();
    let a = audios.iter().max_by_key(|r| r.bandwidth).cloned();
    // Container follows codecs (mp4 unless VP9/AV1/Opus → mkv), like pairs.
    let webmish = v.codecs.to_ascii_lowercase().contains("vp09")
        || v.codecs.to_ascii_lowercase().contains("av01")
        || a.as_ref()
            .map(|r| {
                r.codecs.to_ascii_lowercase().contains("opus")
                    || r.mime.contains("webm")
            })
            .unwrap_or(false);
    if a.is_some() && webmish && opts.allow_rename {
        dest.set_extension("mkv");
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    }
    if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        dest = crate::model::conflict_free_dest(&dest);
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, 0));
        dones.push(0);
    }
    let work = unique_work_dir(&dest);
    tokio::fs::create_dir_all(&work).await?;
    let v_dest = work.join("v");
    let a_dest = work.join("a");
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()),
    );
    let count_v = v
        .timeline_count
        .or_else(|| v.seg_dur_secs.and_then(|d| dash_seg_count(mpd_dur, d)));
    let rep_id_v = if v.id.is_empty() { v.bandwidth.to_string() } else { v.id.clone() };
    let res: Result<()> = async {
        dash_rep_download(&client, &opts, manifest_url, &v, &rep_id_v, count_v, &v_dest, &shared, &limiter, parallel).await?;
        if let Some(rep) = a {
            let count_a = rep
                .timeline_count
                .or_else(|| rep.seg_dur_secs.and_then(|d| dash_seg_count(mpd_dur, d)));
            let rep_id_a = if rep.id.is_empty() { rep.bandwidth.to_string() } else { rep.id.clone() };
            dash_rep_download(&client, &opts, manifest_url, &rep, &rep_id_a, count_a, &a_dest, &shared, &limiter, parallel).await?;
            ffmpeg_mux(&v_dest, Some(&a_dest), &dest).await
        } else {
            // Video-only manifest: deliver the assembled track as-is.
            tokio::fs::rename(&v_dest, &dest)
                .await
                .map_err(|e| anyhow!(io_message(&e)))
        }
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    res?;
    let final_len = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// Strict googlevideo-style edges 403 ranges above ~2 MiB while serving
/// small bounded ranges with 206 (verified live against the strict
/// fixture and googlevideo SABR edges).
const STRICT_PART_MAX: u64 = 2 * 1024 * 1024;

/// Segmented split size for googlevideo URLs (P-speed): no more parts than
/// needed to keep ranges near the strict ~2 MiB window — fewer, fuller
/// ranges beat a storm of micro-ranges. Never exceeds what was asked
/// (a single connection stays single); larger files still split fully
/// and rely on the chunked fallback below when an edge balks.
fn google_parts(total: u64, conns: usize) -> usize {
    let c = conns.clamp(1, 16);
    if c <= 1 || total < 1024 * 1024 {
        return c;
    }
    let fit = ((total + STRICT_PART_MAX - 1) / STRICT_PART_MAX).clamp(2, 16) as usize;
    fit.min(c)
}

/// Classify a retryable attempt error for the end-of-download summary
/// (cuts vs stalls vs restarts), so the log tells the user what the
/// server actually kept doing instead of just the last error.
fn classify_attempt(msg: &str) -> &'static str {
    if msg.contains("stalled (no data") {
        "stalled"
    } else if msg.contains("bytes") {
        "cut"
    } else {
        "other"
    }
}

/// Next split when parallel ranges get throttled (P2): halve 16→8→4→2,
/// then give up splitting (None) and collapse to one sequential stream.
/// Halving (not abandoning) survives hosts that tolerate a few ranges but
/// cut under a many-way storm.
fn halve_conns(conns: usize) -> Option<usize> {
    if conns > 2 {
        Some((conns / 2).max(2))
    } else {
        None
    }
}

/// Throttling signatures on a segmented failure that halving can survive:
/// range rejections, cut-short parts, and rate limits (429). Hard refusals
/// (401/403/410) are NOT included — parallelism didn't cause those.
fn segmented_throttled(msg: &str) -> bool {
    msg.contains("416")
        || msg.contains("segment short")
        || msg.contains("Range")
        || msg.contains("429")
}

/// Next step after a segmented failure (P-speed): halve the split while
/// throttling may just be a too-wide storm, else one chunked round on
/// cut-happy hosts (1 MiB windows 206 where big ranges 403), else the
/// plain single stream. Pure decision table, unit-tested below.
enum SegFallback {
    Halve(usize),
    Chunked,
    Single,
}

fn seg_fallback(is_final: bool, conns: usize, chunked_ok: bool) -> SegFallback {
    if !is_final {
        if let Some(h) = halve_conns(conns) {
            return SegFallback::Halve(h);
        }
    }
    if chunked_ok {
        SegFallback::Chunked
    } else {
        SegFallback::Single
    }
}

/// Append an attempt summary to a terminal transfer error, e.g.
/// `connection ended early (…) [34 tries: 30 cut, 0 stalled, 4 restarted]`.
/// Visible in full in Queues → Log; the row label shows the head of it.
fn with_attempt_summary(
    e: anyhow::Error,
    attempt: u32,
    cuts: u32,
    stalls: u32,
    restarts: u32,
) -> anyhow::Error {
    anyhow::anyhow!(
        "{e:#} [{attempt} tries: {cuts} cut, {stalls} stalled, {restarts} restarted]"
    )
}

async fn single_stream(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    // Retry the whole stream, resuming from the .dcpart file when the
    // server honors ranges. Throttled CDNs (FB/IG class) cut the socket
    // repeatedly MID-file: attempts that still moved bytes must NOT consume
    // the retry budget, or a big file dies after N cuts (the old bug — every
    // cut burned one retry even with 10 MB of progress). Only consecutive
    // zero-progress attempts fail the download, with an absolute cap so a
    // 1-byte trickle can't loop forever. Mirrors download_range's policy.
    let max_retries = opts.max_retries.max(1);
    let mut stalled = 0u32;
    let mut attempt = 0u32;
    // Attempt telemetry for the final summary (see with_attempt_summary).
    let mut cuts = 0u32;
    let mut stalls = 0u32;
    let mut restarts = 0u32;
    // A 416 (offset rejected — total shrank or server forgot the file) is
    // only worth one clean restart; a second 416 means the URL itself is bad.
    let mut restarted_416 = false;
    loop {
        attempt += 1;
        let len_before = tokio::fs::metadata(dest.with_extension("dcpart"))
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        match single_stream_once(client, opts, url, dest, &shared, &limiter).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst)
                    || e.to_string() == "cancelled"
                {
                    return Err(e);
                }
                // Refusals (403/401/410) AFTER partial progress on a signed
                // link almost always mean expiry (fresh captures work, resume
                // doesn't). Say so plainly instead of a generic refusal —
                // the fix is a fresh capture, not another resume.
                if is_final_http_error(&e) && len_before > 0 && is_fragile_cdn(url) {
                    let got = crate::model::format_bytes(len_before);
                    return Err(final_err(format!(
                        "link expired after {got} — replay the reel and click the download button again for a fresh link, then resume"
                    )));
                }
                // A refusal on FIRST touch can be edge throttling noise (the
                // same URL 206s on immediate retry — verified live), so spend
                // retry budget on it instead of failing instantly. A truly
                // dead link fails every attempt and still reports the refusal
                // with its type intact for the fallback routing above.
                if is_final_http_error(&e) {
                    stalled = stalled.saturating_add(1);
                    if stalled >= max_retries || attempt >= max_retries * 20 {
                        return Err(e);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
                    continue;
                }
                let msg = e.to_string();
                match classify_attempt(&msg) {
                    "cut" => cuts += 1,
                    "stalled" => stalls += 1,
                    _ => {}
                }
                if msg.contains("416") && len_before > 0 && !restarted_416 {
                    // Server rejected our resume offset: drop the prefix and
                    // restart from zero once (never resume a 416 twice).
                    restarted_416 = true;
                    restarts += 1;
                    let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
                    shared.downloaded.store(0, Ordering::SeqCst);
                    if let Ok(mut g) = shared.part_done.lock() {
                        if let Some(slot) = g.first_mut() {
                            *slot = 0;
                        }
                    }
                    stalled = 0;
                    continue;
                }
                // Attempts append, so file growth == progress. A SHRUNK file
                // means the server ignored our Range (200) and we truncated:
                // count it as a restart, not silent progress.
                let len_after = tokio::fs::metadata(dest.with_extension("dcpart"))
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    if len_after < len_before {
                        restarts += 1;
                    }
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 20 {
                    return Err(with_attempt_summary(e, attempt, cuts, stalls, restarts));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
    }
}

async fn single_stream_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    shared: &Arc<SharedState>,
    limiter: &std::sync::Arc<RateLimiter>,
) -> Result<()> {
    // Resume support: continue from the existing .dcpart file, but ONLY
    // when it is bound to this exact URL+total (see ResumeMeta). A stale
    // prefix from another URL is discarded, never resumed into this file.
    let tmp = dest.with_extension("dcpart");
    let known_total = if shared.has_total.load(Ordering::SeqCst) {
        Some(shared.total.load(Ordering::SeqCst))
    } else {
        None
    };
    let mut start_at: u64 = resume_offset(dest, "dcpart", url, known_total, 1);

    // Always request a range (even from 0): some media servers truncate
    // plain 200 responses but serve full content for range requests.
    // Bounded when the total is known: strict edges (googlevideo SABR
    // serving, `gir=yes`) 403 open-ended full-file ranges outright while
    // serving the identical bounded window with 206 (verified live).
    let mut req = opts.get(client, url);
    req = if shared.has_total.load(Ordering::SeqCst) {
        let total = shared.total.load(Ordering::SeqCst);
        if total > start_at {
            req.header(
                reqwest::header::RANGE,
                format!("bytes={start_at}-{}", total - 1),
            )
        } else {
            req.header(reqwest::header::RANGE, format!("bytes={start_at}-"))
        }
    } else {
        req.header(reqwest::header::RANGE, format!("bytes={start_at}-"))
    };
    let resp = match req.send().await?.error_for_status() {
        Ok(r) => r,
        Err(e) => {
            let mapped = http_status_err(url, e);
            if start_at == 0 && !is_final_http_error(&mapped) {
                // server dislikes Range (but isn't refusing us): plain GET
                opts.get(client, url)
                    .send()
                    .await?
                    .error_for_status()
                    .map_err(|e| http_status_err(url, e))?
            } else {
                return Err(mapped);
            }
        }
    };
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        eprintln!(
            "[dbg] media status={} len={:?} range={:?} ctype={:?}",
            resp.status(),
            resp.content_length(),
            resp.headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok()),
            resp.headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
        );
    }

    // If server ignored our Range, restart from 0.
    let is_partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !is_partial && start_at > 0 {
        start_at = 0;
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    // Ground the resume flag in this response (the pre-download probe may
    // have been skipped for expiring CDN links): only a 206 proves ranges.
    // A 200 to a bytes=0- request means ranges are ignored; a 200 after a
    // nonzero offset restart means the same after truncation above.
    if is_partial {
        shared.resumable.store(true, Ordering::SeqCst);
    } else if start_at > 0 || resp.status() == reqwest::StatusCode::OK {
        shared.resumable.store(false, Ordering::SeqCst);
    }
    // Adopt server-advertised total when we didn't have one.
    if !shared.has_total.load(Ordering::SeqCst) {
        if let Some(len) = resp.content_length() {
            shared.total.store(len.saturating_add(start_at), Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
    }
    // Single part view for the details dialog.
    {
        let total = shared.total.load(Ordering::SeqCst);
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, total));
        dones.push(start_at);
    }
    shared.downloaded.store(start_at, Ordering::SeqCst);

    // Open for resume: keep existing bytes (seek/append below), no truncate.
    #[allow(clippy::suspicious_open_options)]
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&tmp)
        .await?;
    if start_at > 0 && is_partial {
        file.seek(std::io::SeekFrom::Start(start_at)).await?;
    } else if start_at > 0 {
        file.set_len(0).await?;
    }

    let per_conn_limit = opts.speed_limit_bps; // single connection gets full budget
    let _ = per_conn_limit;
    // Stall guard: a hung socket (no bytes at all for the timeout window)
    // must fail the attempt so the retry loop resumes — otherwise a
    // throttled CDN drip-freeze hangs the download forever at 0 B/s.
    // Clamped 15–120 s so slow-but-moving trickles are never killed.
    let stall_timeout = (opts.timeout_secs.max(15)).min(120);
    let mut stream = resp.bytes_stream();
    loop {
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(stall_timeout),
            stream.next(),
        )
        .await
        .map_err(|_| {
            anyhow!("stalled (no data for {stall_timeout}s) — resuming")
        })?;
        let Some(chunk) = next else { break };
        wait_if_paused(&shared).await?;
        let bytes = chunk.map_err(|e| anyhow!("network: {e}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.first_mut() {
                *slot = slot.saturating_add(bytes.len() as u64);
            }
        }
        limiter.consume(bytes.len() as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    // A clean-but-early socket close yields a short stream with NO error —
    // the classic Facebook/IG CDN failure ("connection ended early" in the
    // UI). When the total is known, verify the landed bytes and fail so the
    // retry loop resumes from the .dcpart offset instead of finalizing a
    // truncated file as success (which the app would then report as a
    // generic failure with no resume happening).
    if shared.has_total.load(Ordering::SeqCst) {
        let want = shared.total.load(Ordering::SeqCst);
        let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        if want > 0 && got < want {
            return Err(anyhow!(
                "connection ended early (got {got} of {want} bytes) — resuming"
            ));
        }
        // Unknown totals (chunked / lengthless responses) can end cleanly yet
        // short after a dropped socket: re-probe once, and if the server now
        // reports MORE than we received, fail so the retry loop resumes instead
        // of finalizing a truncated file as "Completed".
    } else {
        let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        let (t, _, _, _) = probe(client, opts, url).await;
        if let Some(total) = t {
            shared.total.store(total, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
            if total > got {
                return Err(anyhow!(
                    "stream ended short ({got} of {total} bytes) — resuming"
                ));
            }
        }
    }
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
}

/// Sequential chunked download for throttling signed CDNs (FB/IG reels).
///
/// Failure mode it fixes: some CDN edges cap *bytes per connection*
/// (~1 MB observed on fbcdn — every stream dies at the same offset). A
/// single long stream can then NEVER finish: each resume restarts (or
/// truncates on a 200) and dies at the same ~1 MB mark, burning all 100
/// attempts at a frozen offset. Small bounded ranges (1 MiB) each complete
/// under the cap, and any cut only loses the in-flight chunk — the file
/// offset on disk is the resume point, so progress is strictly additive.
///
/// Requires a known total (browser-measured size or a prior probe);
/// callers fall back to `single_stream` when it is unknown.
const FRAGILE_CHUNK: u64 = 1024 * 1024;

#[allow(clippy::too_many_arguments)]
async fn chunked_stream(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    mut total: u64,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    let tmp = dest.with_extension("dcpart");
    let max_retries = opts.max_retries.max(1);
    let stall_timeout = (opts.timeout_secs.max(15)).min(120);
    // Bind resume state first: a prefix from another URL is discarded here,
    // so every chunk below appends bytes of THIS file only (see ResumeMeta).
    resume_offset(dest, "dcpart", url, Some(total), 1);
    // One restart-from-zero per download (stale offset / shrunken file).
    let mut restarted = false;
    let mut stalled = 0u32;
    let mut attempt = 0u32;
    // Attempt telemetry for the final summary (see with_attempt_summary).
    let mut cuts = 0u32;
    let mut stalls = 0u32;
    let mut restarts = 0u32;
    // Consecutive refusals on a live offset: strict edges throttle bursts
    // with 403s that clear after a breather (verified live: the same range
    // 206s again). Back off hard a few times before declaring expiry —
    // instant expiry misdiagnosed throttling and killed good links.
    let mut refusals = 0u32;
    // Single part view for the dialogs (no lock held across awaits).
    let have_now = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, total));
        dones.push(have_now);
    }
    loop {
        attempt += 1;
        let offset = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        if total > 0 && offset >= total {
            break;
        }
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        wait_if_paused(&shared).await?;
        let end = if total > 0 {
            (offset.saturating_add(FRAGILE_CHUNK).saturating_sub(1)).min(total.saturating_sub(1))
        } else {
            offset.saturating_add(FRAGILE_CHUNK).saturating_sub(1)
        };
        let want = end.saturating_sub(offset).saturating_add(1);
        let len_before = offset;
        match chunked_once(client, opts, url, &tmp, offset, end, want, stall_timeout, &shared, &limiter).await {
            Ok(wrote) => {
                stalled = 0;
                refusals = 0;
                if total > 0 && offset.saturating_add(wrote) >= total {
                    break;
                }
                if wrote == 0 {
                    stalled = stalled.saturating_add(1);
                }
            }
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst) || e.to_string() == "cancelled" {
                    return Err(e);
                }
                if is_final_http_error(&e) {
                    // Refusal on a live offset: throttle burst OR expiry.
                    // Throttle clears after a breather; expiry never does —
                    // so absorb a couple refusals with backoffs first and
                    // only call it expired when it persists (kept short so a
                    // gated YouTube quality fails fast instead of sitting at
                    // 20% with 0 B/s for minutes).
                    if len_before > 0 {
                        if refusals < 2 {
                            refusals += 1;
                            let wait = (2u64 << refusals.min(4)).min(30);
                            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                            continue;
                        }
                        // SABR-gated streams (`gir=yes`) serve only a ~2 MB
                        // preview to plain ranged GETs outside a logged-in
                        // browser session (verified live) — that reads exactly
                        // like expiry ("progress then persistent refusal"),
                        // but re-capturing cannot help. Say so plainly.
                        if url.contains("gir=yes") {
                            let got = crate::model::format_bytes(len_before);
                            return Err(final_err(format!(
                                "YouTube serves only the first ~2 MB of this quality outside the browser (got {got}) — pick 720p or below for a complete direct file"
                            )));
                        }
                        let got = crate::model::format_bytes(len_before);
                        return Err(final_err(format!(
                            "link expired after {got} ({refusals} refusals) — replay the reel and click the download button again for a fresh link, then resume"
                        )));
                    }
                    // First-touch refusal: retry within budget (edge throttle
                    // noise 206s on immediate retry — verified live).
                    stalled = stalled.saturating_add(1);
                    if stalled >= max_retries || attempt >= max_retries * 30 {
                        return Err(e);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
                    continue;
                }
                let msg = e.to_string();
                match classify_attempt(&msg) {
                    "cut" => cuts += 1,
                    "stalled" => stalls += 1,
                    _ => {}
                }
                if msg.contains("416") && !restarted {
                    // Offset rejected (file shrank server-side?): refresh the
                    // total once and restart from zero instead of wedging.
                    restarted = true;
                    restarts += 1;
                    let _ = tokio::fs::remove_file(&tmp).await;
                    shared.downloaded.store(0, Ordering::SeqCst);
                    if let Ok(mut g) = shared.part_done.lock() {
                        if let Some(slot) = g.first_mut() {
                            *slot = 0;
                        }
                    }
                    if let (t, _, _, _) = probe(client, opts, url).await {
                        if let Some(t) = t {
                            total = t;
                            shared.total.store(t, Ordering::SeqCst);
                            shared.has_total.store(true, Ordering::SeqCst);
                            // Rebind resume state to the refreshed total.
                            write_resume_meta(dest, url, Some(t), 1);
                        }
                    }
                    stalled = 0;
                    continue;
                }
                let len_after = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    if len_after < len_before {
                        restarts += 1;
                    }
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 30 {
                    return Err(with_attempt_summary(e, attempt, cuts, stalls, restarts));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
    }
    shared.downloaded.store(
        tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0),
        Ordering::SeqCst,
    );
    let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    if total > 0 && got < total {
        return Err(anyhow!(
            "connection ended early (got {got} of {total} bytes) — resuming"
        ));
    }
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
}

/// One chunk of [`chunked_stream`]: GET `bytes=offset-end`, append exactly
/// that window to `tmp`. Returns bytes appended. A 200 (Range ignored) is
/// sliced down to our window by discarding the prefix — works as far as the
/// connection survives; a short stream is a retryable error, never silent.
#[allow(clippy::too_many_arguments)]
async fn chunked_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    tmp: &PathBuf,
    offset: u64,
    end: u64,
    want: u64,
    stall_timeout: u64,
    shared: &Arc<SharedState>,
    limiter: &std::sync::Arc<RateLimiter>,
) -> Result<u64> {
    let resp = opts
        .get(client, url)
        .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
        .send()
        .await?;
    let resp = resp
        .error_for_status()
        .map_err(|e| http_status_err(url, e))?;
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        eprintln!(
            "[dbg] chunk range={offset}-{end} status={} len={:?}",
            resp.status(),
            resp.content_length(),
        );
    }
    let is_partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !is_partial {
        shared.resumable.store(false, Ordering::SeqCst);
    } else {
        shared.resumable.store(true, Ordering::SeqCst);
    }
    // Kept for the short error below: a zero-byte range with a 200/empty
    // body vs a cut 206 mean very different things (see probe_range_once).
    let resp_status = resp.status();
    let resp_len = resp.content_length();
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tmp)
        .await?;
    // Bytes to skip when slicing a 200 response down to our window.
    let mut to_skip = if is_partial { 0 } else { offset };
    let mut to_take = want;
    let mut wrote = 0u64;
    let mut stream = resp.bytes_stream();
    loop {
        if to_take == 0 {
            break;
        }
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(stall_timeout),
            stream.next(),
        )
        .await
        .map_err(|_| anyhow!("stalled (no data for {stall_timeout}s) — resuming"))?;
        let Some(chunk) = next else { break };
        wait_if_paused(shared).await?;
        let bytes = chunk.map_err(|e| anyhow!("network: {e}"))?;
        let mut data = &bytes[..];
        if to_skip > 0 {
            let cut = (to_skip as usize).min(data.len());
            data = &data[cut..];
            to_skip -= cut as u64;
            if data.is_empty() {
                continue;
            }
        }
        let take = (to_take as usize).min(data.len());
        data = &data[..take];
        if data.is_empty() {
            break;
        }
        file.write_all(data)
            .await
            .map_err(|e| anyhow!(io_message(&e)))?;
        wrote += data.len() as u64;
        to_take -= data.len() as u64;
        shared.downloaded.fetch_add(data.len() as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.first_mut() {
                *slot = slot.saturating_add(data.len() as u64);
            }
        }
        limiter.consume(data.len() as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    if wrote != want {
        // Zero-byte ranges name the cause: an empty/ignored range (200, no
        // body) means resume is impossible on this URL; a cut 206 means
        // throttling (resume works). The Queues log shows the full text.
        if wrote == 0 {
            return Err(anyhow!(
                "empty range {offset}-{end} (status {resp_status}, len {resp_len:?}) — resuming"
            ));
        }
        return Err(anyhow!(
            "connection ended early (got {wrote} of {want} bytes in range {offset}-{end}) — resuming"
        ));
    }
    Ok(wrote)
}

async fn segmented(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    total: u64,
    connections: usize,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    // Fresh segmented download: reset counter (no cross-session resume for v1).
    shared.downloaded.store(0, Ordering::SeqCst);

    // Bind the part set: stale dcpart{i} files from another URL/total/split
    // are discarded here (ranges below derive from this exact split).
    // NOTE: this drops cross-session resume for mismatched splits by design
    // (P0); same-URL+total+conns still resumes via download_range seeding.
    resume_offset(dest, "dcpart", url, Some(total), connections);

    let chunk = total / connections as u64;

    let mut handles = Vec::new();
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        for i in 0..connections {
            let start = i as u64 * chunk;
            let mut end = if i == connections - 1 {
                total - 1
            } else {
                (i as u64 + 1) * chunk - 1
            };
            if end >= total {
                end = total - 1;
            }
            totals.push((start, end));
            dones.push(0);
        }
    }
    for i in 0..connections {
        let (start, end) = mutex_guard(&shared.part_total)[i];
        let part = dest.with_extension(format!("dcpart{}", i));
        let client = client.clone();
        let url = url.to_string();
        let shared = shared.clone();
        let opts = opts.clone();
        let limiter = limiter.clone();
        handles.push(tokio::spawn(async move {
            download_range(
                &client,
                &opts,
                &url,
                &part,
                start,
                end,
                limiter,
                i,
                shared,
            )
            .await
        }));
    }

    let mut failed: Option<anyhow::Error> = None;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if failed.is_none() {
                    failed = Some(e);
                }
            }
            Err(e) => {
                if failed.is_none() {
                    failed = Some(anyhow!("task join: {e}"));
                }
            }
        }
    }
    if let Some(e) = failed {
        // Cancel siblings already done; leave parts for debugging.
        return Err(e);
    }

    // Merge parts in order.
    let tmp = dest.with_extension("dcpart");
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .await?;
    for i in 0..connections {
        let part = dest.with_extension(format!("dcpart{}", i));
        let mut f = tokio::fs::File::open(&part).await?;
        tokio::io::copy(&mut f, &mut out).await?;
    }
    out.flush().await?;
    drop(out);
    for i in 0..connections {
        let part = dest.with_extension(format!("dcpart{}", i));
        let _ = tokio::fs::remove_file(&part).await;
    }
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// One segment of a multi-connection download. Args are bundled loosely;
/// splitting them further would obscure the call sites.
#[allow(clippy::too_many_arguments)]
async fn download_range(
    client: &reqwest::Client,    opts: &EngineOpts,
    url: &str,
    part: &PathBuf,
    start: u64,
    end: u64,
    limiter: std::sync::Arc<RateLimiter>,
    part_idx: usize,
    shared: Arc<SharedState>,
) -> Result<()> {
    let max_retries = opts.max_retries.max(1);
    // Attempts that made ANY progress don't count toward the retry budget
    // (throttled CDNs drip-feed: each 1 MB trickle resets the stall count).
    // Only consecutive zero-progress attempts fail the part, with an
    // absolute cap so a 1-byte trickle can't loop forever.
    let mut stalled = 0u32;
    let mut attempt = 0u32;
    // Seed the slot from any previous partial file (FB-style throttled
    // servers get resumed, never restarted).
    let range_len = end.saturating_sub(start).saturating_add(1);
    let mut have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    if have > range_len {
        // Stale garbage from another URL/shape: drop it, start clean.
        let _ = tokio::fs::remove_file(part).await;
        have = 0;
    }
    if let Ok(mut g) = shared.part_done.lock() {
        if let Some(slot) = g.get_mut(part_idx) {
            if *slot < have {
                shared.downloaded.fetch_add(have - *slot, Ordering::SeqCst);
                *slot = have;
            }
        }
    }
    loop {
        attempt += 1;
        let len_before = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        let res = download_range_once(
            client,
            opts,
            url,
            part,
            start,
            end,
            limiter.clone(),
            part_idx,
            &shared,
        )
        .await;
        match res {
            Ok(()) => return Ok(()),
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst) {
                    return Err(e);
                }
                if e.to_string() == "cancelled" || is_final_http_error(&e) {
                    return Err(e);
                }
                // Attempts append, so file growth == progress.
                let len_after = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 20 {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_range_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    part: &PathBuf,
    start: u64,
    end: u64,
    limiter: std::sync::Arc<RateLimiter>,
    part_idx: usize,
    shared: &Arc<SharedState>,
) -> Result<()> {
    let range_len = end.saturating_sub(start).saturating_add(1);
    // Resume offset: previous attempts (or an earlier throttled run) may
    // have left a prefix. Stale over-long files are dropped, not appended to.
    let orig = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    let have = if orig > range_len {
        let _ = tokio::fs::remove_file(part).await;
        0
    } else {
        orig
    };
    let from = start.saturating_add(have);
    let resp = opts
        .get(client, url)
        .header(
            reqwest::header::RANGE,
            format!("bytes={from}-{end}"),
        )
        .send()
        .await?
        .error_for_status()
        .map_err(|e| http_status_err(url, e))?;

    // 206 continues our prefix (append). A 200 means the server ignored Range
    // and sent the whole file: slice our window out of the stream instead of
    // writing garbage (or failing outright).
    let is_partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !is_partial && have > 0 {
        let _ = tokio::fs::remove_file(part).await;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .await?;
    let mut stream = resp.bytes_stream();
    // Bytes to skip/take when slicing a 200 response down to our window.
    let mut to_skip = if is_partial { 0 } else { start };
    let mut to_take = range_len;
    while let Some(chunk) = stream.next().await {
        wait_if_paused(shared).await?;
        let bytes = chunk.map_err(|e| anyhow!("network: {e}"))?;
        let mut data = &bytes[..];
        if to_skip > 0 {
            let cut = (to_skip as usize).min(data.len());
            data = &data[cut..];
            to_skip -= cut as u64;
            if data.is_empty() {
                continue;
            }
        }
        if !is_partial {
            let take = (to_take as usize).min(data.len());
            data = &data[..take];
            to_take -= take as u64;
        }
        if data.is_empty() {
            break;
        }
        file.write_all(data)
            .await
            .map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(data.len() as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.get_mut(part_idx) {
                *slot = slot.saturating_add(data.len() as u64);
            }
        }
        limiter.consume(data.len() as u64).await;
        if !is_partial && to_take == 0 {
            break;
        }
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    // A clean-but-early socket close yields a short stream with no error —
    // never accept a short part (the retry loop re-fetches the range).
    let wrote = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    let want = end.saturating_sub(start).saturating_add(1);
    if wrote != want {
        return Err(anyhow!(
            "segment short: got {wrote} of {want} bytes — retrying the range"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize_filename("a/b\\c:d.mp4"), "a_b_c_d.mp4");
        assert_eq!(sanitize_filename("   "), "download.bin");
        // multibyte truncation must not panic or split chars
        let long = format!("{}.mp4", "ف".repeat(200));
        let out = sanitize_filename(&long);
        assert!(out.ends_with(".mp4"));
        assert!(out.len() <= 124);
        assert!(String::from_utf8(out.clone().into_bytes()).is_ok());
    }

    #[test]
    fn resume_meta_binding() {
        // Sidecar binds .dcpart bytes to the exact URL+total+split (P0):
        // same URL resumes, different URL discards, legacy unbound
        // partials resume once by length.
        let dir = std::env::temp_dir().join(format!("dc_meta_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("v.mp4");
        let part = dest.with_extension("dcpart");
        let url_a = "https://cdn.example/v.mp4?tok=a";
        let url_b = "https://cdn.example/v.mp4?tok=b";

        // Fresh: no partial, binds and returns 0.
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 0);
        assert!(resume_meta_path(&dest).exists());

        // Same URL: prefix resumes.
        std::fs::write(&part, vec![7u8; 400]).unwrap();
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 400);

        // Total learned later is adopted, still resumes.
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 400);

        // Different total for same URL: stale prefix discarded.
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(2000), 1), 0);
        assert!(!part.exists());

        // Different URL: stale prefix discarded (the FB slice case).
        std::fs::write(&part, vec![7u8; 100]).unwrap();
        write_resume_meta(&dest, url_a, Some(1000), 1);
        assert_eq!(resume_offset(&dest, "dcpart", url_b, Some(1000), 1), 0);
        assert!(!part.exists());

        // Legacy partial without sidecar: resumes once, then binds.
        std::fs::write(&part, vec![7u8; 50]).unwrap();
        let _ = std::fs::remove_file(resume_meta_path(&dest));
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 50);
        assert!(read_resume_meta(&dest).is_some());

        // Split change (1 -> 4 conns): segmented parts dropped.
        let p0 = dest.with_extension("dcpart0");
        std::fs::write(&p0, vec![7u8; 10]).unwrap();
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 4), 0);
        assert!(!p0.exists());

        // meta_matches unit rules.
        let m = ResumeMeta { url: url_a.into(), total: Some(1000), conns: 1 };
        assert!(meta_matches(&m, url_a, Some(1000), 1));
        assert!(!meta_matches(&m, url_b, Some(1000), 1));
        assert!(!meta_matches(&m, url_a, Some(999), 1));
        assert!(!meta_matches(&m, url_a, Some(1000), 4));
        assert!(meta_matches(&m, url_a, None, 1)); // unknown: wildcard

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn halve_schedule() {
        // P2: 16→8→4→2, then give up splitting (single stream).
        assert_eq!(halve_conns(16), Some(8));
        assert_eq!(halve_conns(8), Some(4));
        assert_eq!(halve_conns(4), Some(2));
        assert_eq!(halve_conns(3), Some(2));
        assert_eq!(halve_conns(2), None);
        assert_eq!(halve_conns(1), None);
        // Throttling signatures halve; hard refusals go straight single.
        assert!(segmented_throttled("segment short: got 1 of 100 bytes — retrying the range"));
        assert!(segmented_throttled("HTTP 416 Range Not Satisfiable"));
        assert!(segmented_throttled("HTTP status client error (429 Too Many Requests)"));
        assert!(segmented_throttled("Range Not Satisfiable"));
        assert!(!segmented_throttled("network: connection reset"));
    }

    #[test]
    fn seg_fallback_table() {
        // P-speed decision table after a segmented failure.
        let is = |f: SegFallback| match f {
            SegFallback::Halve(h) => format!("halve:{h}"),
            SegFallback::Chunked => "chunked".to_string(),
            SegFallback::Single => "single".to_string(),
        };
        assert_eq!(is(seg_fallback(false, 16, true)), "halve:8");
        assert_eq!(is(seg_fallback(false, 3, true)), "halve:2");
        assert_eq!(is(seg_fallback(false, 2, true)), "chunked");
        assert_eq!(is(seg_fallback(false, 2, false)), "single");
        assert_eq!(is(seg_fallback(true, 8, true)), "chunked");
        assert_eq!(is(seg_fallback(true, 8, false)), "single");
        assert_eq!(is(seg_fallback(true, 1, false)), "single");
    }

    #[test]
    fn google_split_sizing() {
        // P-speed: fit ranges to the strict window, never exceed asked.
        assert_eq!(google_parts(5 * 1024 * 1024, 8), 3);
        assert_eq!(google_parts(28 * 1024 * 1024, 8), 8);
        assert_eq!(google_parts(5 * 1024 * 1024, 2), 2);
        assert_eq!(google_parts(5 * 1024 * 1024, 1), 1);
        assert_eq!(google_parts(500 * 1024, 8), 8);
        assert_eq!(google_parts(100 * 1024 * 1024, 16), 16);
    }

    #[test]
    fn fb_slice_normalize() {
        // Real failure: scontent slice URL (bytestart/byteend) can only
        // return ~1.1 MB while the full file is ~9.4 MB.
        let slice = "https://scontent-lga3-2.xx.fbcdn.net/o1/v/t2/f2/m366/A.mp4?oh=abc&oe=xyz&bytestart=967&byteend=1193530";
        let full = normalize_media_url(slice);
        assert!(!full.contains("bytestart"), "{full}");
        assert!(!full.contains("byteend"), "{full}");
        assert!(full.contains("oh=abc"), "{full}");
        assert!(full.contains("oe=xyz"), "{full}");
        assert!(is_fragile_cdn(slice));
        assert!(is_fragile_cdn(&full));
        // Non-social hosts untouched.
        let other = "https://cdn.example/v.mp4?bytestart=1&byteend=2";
        assert_eq!(normalize_media_url(other), other);
        // No slice params: identity.
        let plain = "https://scontent.fdac90-1.fna.fbcdn.net/v.mp4?oh=1";
        assert_eq!(normalize_media_url(plain), plain);
    }

    #[test]
    fn dispositions() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; filename=\"my file.zip\""),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("my file.zip"));
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static(
                "attachment; filename*=UTF-8''%D9%81%D8%A7%D8%B1%D8%B3%DB%8C.mp3",
            ),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("فارسی.mp3"));
        // P1: semicolons inside quotes must not split the value.
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; filename=\"a;b.mp4\""),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("a;b.mp4"));
        // P1: parameter names are case-insensitive; quoted filename* works.
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; FileName*=UTF-8''clip%20hd.mp4"),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("clip hd.mp4"));
        // P1: filename* wins over filename; unsafe chars sanitized.
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; filename=\"a/b.mp4\"; filename*=UTF-8''c:d.mp4"),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("c_d.mp4"));
        let empty = reqwest::header::HeaderMap::new();
        assert_eq!(disposition_filename(&empty), None);
    }

    #[test]
    fn youtube_cipher_fixtures() {
        // Classic-shape player JS: helper object + decipher function.
        let js = r#"var XX={aB:function(a){a.reverse()},MB:function(a,b){a.splice(0,b)},kB:function(a,b){var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}};var Y=function(a){a=a.split("");XX.aB(a);XX.MB(a,2);XX.kB(a,1);return a.join("")};"#;
        let ops = parse_sig_ops(js).expect("classic decipher parses");
        assert_eq!(ops, vec![SigOp::Reverse, SigOp::Splice(2), SigOp::Swap(1)]);
        // "abcdef" → reverse "fedcba" → splice 2 "dcba" → swap(1) "cdba".
        assert_eq!(apply_sig_ops("abcdef", &ops), "cdba");
        assert_eq!(apply_sig_ops("", &ops), "");
        assert_eq!(apply_sig_ops("x", &[SigOp::Swap(5)]), "x");
        // signatureCipher assembly.
        let cipher = "url=https%3A%2F%2Fx%2Fv%3Fitag%3D18&sp=sig&s=abcdef";
        assert_eq!(
            decipher_signature_cipher(cipher, &ops).as_deref(),
            Some("https://x/v?itag=18&sig=cdba")
        );
        assert_eq!(decipher_signature_cipher("url=&s=", &ops), None);
        assert_eq!(decipher_signature_cipher("nonsense", &ops), None);
        // Throttle-param fix: a SECOND simple-shape function is the n-code.
        let js2 = js.to_string()
            + r#"var Z=function(a){a=a.split("");XX.MB(a,3);return a.join("")};"#;
        assert_eq!(
            try_simple_n(&js2, &ops, "abcdef").as_deref(),
            Some("def")
        );
        // Single candidate (decipher only) → no confident n transform.
        assert_eq!(try_simple_n(js, &ops, "abcdef"), None);
        // Exotic shapes bail instead of guessing.
        assert_eq!(parse_sig_ops("var x = 1;"), None);
        assert_eq!(
            parse_sig_ops("var Y=function(a){return a;}"),
            None
        );
        // Player JS URL extraction.
        let html = r#"<html><head><script>var cfg = {"jsUrl":"/s/player/abc/base.js"};</script></head></html>"#;
        assert_eq!(
            youtube_player_js_url(html).as_deref(),
            Some("https://www.youtube.com/s/player/abc/base.js")
        );
        assert_eq!(youtube_player_js_url("<html></html>"), None);
        // ratebypass hygiene.
        assert_eq!(
            with_ratebypass("https://r1.googlevideo.com/v?x=1"),
            "https://r1.googlevideo.com/v?x=1&ratebypass=yes"
        );
        assert_eq!(
            with_ratebypass("https://r1.googlevideo.com/v?ratebypass=yes"),
            "https://r1.googlevideo.com/v?ratebypass=yes"
        );
        assert_eq!(
            with_ratebypass("https://cdn.example/v.mp4"),
            "https://cdn.example/v.mp4"
        );
        // n-param rewrite (pure part).
        let fixed = fix_n_param("https://h/v?n=abcdef&x=1", &js2, &ops);
        assert_eq!(fixed, "https://h/v?n=def&x=1");
        assert_eq!(
            fix_n_param("https://h/v?x=1", &js2, &ops),
            "https://h/v?x=1"
        );
    }

    #[test]
    fn youtube_fixture() {
        let html = r#"<html><head></head><body><script>var ytInitialPlayerResponse = {"videoDetails":{"title":"My Vid"},"streamingData":{"formats":[{"mimeType":"video/mp4; codecs=\"avc1\"","width":640,"url":"https://cdn/x/360.mp4"},{"mimeType":"video/mp4; codecs=\"avc1\"","width":1280,"url":"https://cdn/x/720.mp4"},{"mimeType":"video/webm","width":1920,"url":"https://cdn/x/1080.webm"}]}};</script></body></html>"#;
        let hit = resolve_youtube(html).expect("should resolve");
        assert_eq!(hit.0, "https://cdn/x/720.mp4");
        assert_eq!(hit.1, "My Vid.mp4");
    }

    #[test]
    fn generic_fixture() {
        let html = r#"<html><head><meta property="og:title" content="Cool Clip"><meta property="og:video:url" content="https://cdn.example/v.mp4?x=1"></head></html>"#;
        let hit = resolve_generic(html, "https://example.com/p").expect("should resolve");
        assert_eq!(hit.0, "https://cdn.example/v.mp4?x=1");
        assert_eq!(hit.1, "Cool Clip.mp4");
        let bare = resolve_generic(
            "<a href=\"https://cdn.example/files/a.mkv\">dl</a>",
            "https://example.com/p",
        )
        .expect("bare link");
        assert_eq!(bare.0, "https://cdn.example/files/a.mkv");
    }

    #[test]
    fn meta_orders() {
        let html = r#"<meta content="https://c/v.webm" property="og:video">"#;
        assert_eq!(
            meta_content(html, "og:video").as_deref(),
            Some("https://c/v.webm")
        );
        assert_eq!(tag_text("<title>  Hi </title>", "title").as_deref(), Some("Hi"));
        assert_eq!(guess_ext("https://h/f.MKV?1"), ".mkv");
        assert_eq!(with_ext("A", ".mp4"), "A.mp4");
        assert_eq!(with_ext("A.mp4", ".mp4"), "A.mp4");
    }

    #[test]
    fn hostile_pages_never_panic() {
        // marker with no object behind it (truncated page)
        assert_eq!(resolve_youtube("<script>var ytInitialPlayerResponse = "), None);
        assert_eq!(resolve_youtube(""), None);
        // truncated JSON object
        assert_eq!(
            resolve_youtube("ytInitialPlayerResponse = {\"videoDetails\":{\"title\":\"x\""),
            None
        );
        // unbalanced braces inside strings
        assert_eq!(
            resolve_youtube("ytInitialPlayerResponse = {\"a\":\"}{\"}"),
            None
        );
        // sorry-wall page: no streams, must return None (flag set upstream)
        let sorry = "<html><body>unusual traffic <a href=\"/sorry/index\">x</a></body></html>";
        assert_eq!(resolve_youtube(sorry), None);
        assert_eq!(resolve_generic(sorry, "https://www.youtube.com/watch?v=x"), None);
        // multibyte text around a media link must not panic on slicing
        let tricky = "فارسی https://cdn.example/ünïcode-vïd.mp4\" multibyte tail 日本語";
        let hit = resolve_generic(tricky, "https://example.com/p");
        assert!(hit.is_some());
        // youtube ids
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=aqz-KE-bpKQ").as_deref(),
            Some("aqz-KE-bpKQ")
        );
        assert_eq!(
            youtube_id("https://youtu.be/aqz-KE-bpKQ").as_deref(),
            Some("aqz-KE-bpKQ")
        );
        assert_eq!(
            youtube_id("https://www.youtube.com/shorts/aqz-KE-bpKQ").as_deref(),
            Some("aqz-KE-bpKQ")
        );
        assert_eq!(youtube_id("https://www.youtube.com/"), None);
        // empty probe-ish inputs
        assert_eq!(disposition_filename(&reqwest::header::HeaderMap::new()), None);
        assert_eq!(sanitize_filename(""), "download.bin");
    }

    #[test]
    fn dash_helpers() {
        // EXT-X-KEY attribute parsing (quoted commas safe).
        let attrs = hls_key_attrs("#EXT-X-KEY:METHOD=AES-128,URI=\"https://k/x.key\",IV=0x1234");
        assert_eq!(hls_attr(&attrs, "METHOD").as_deref(), Some("AES-128"));
        assert_eq!(hls_attr(&attrs, "uri").as_deref(), Some("https://k/x.key"));
        assert_eq!(hls_attr(&attrs, "iv").as_deref(), Some("0x1234"));
        // Key policy: none/plain ok; SAMPLE-AES rejected; rotation (P3)
        // yields a per-segment schedule instead of an error.
        let plain = hls_key_schedule("#EXTM3U\n#EXTINF:6,\ns.ts\n", "https://h/pl.m3u8").unwrap();
        assert_eq!(plain.len(), 1);
        assert!(plain[0].is_none());
        let aes = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\",IV=0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n#EXTINF:6,\ns.ts\n";
        let sched = hls_key_schedule(aes, "https://h/pl.m3u8").unwrap();
        assert_eq!(sched.len(), 1);
        let slot = sched[0].clone().expect("key");
        assert_eq!(slot.uri, "https://h/k");
        assert_eq!(slot.iv_hex.as_deref(), Some("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
        // Key change mid-playlist: segments before/after differ (rotation).
        let rot = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"a\"\n#EXTINF:6,\ns0.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"b\"\n#EXTINF:6,\ns1.ts\n#EXT-X-ENDLIST\n";
        let rsched = hls_key_schedule(rot, "https://h/pl.m3u8").unwrap();
        assert_eq!(rsched.len(), 2);
        assert_eq!(rsched[0].as_ref().unwrap().uri, "https://h/a");
        assert_eq!(rsched[1].as_ref().unwrap().uri, "https://h/b");
        // METHOD=NONE clears back to plain.
        let none = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"a\"\n#EXTINF:6,\ns0.ts\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:6,\ns1.ts\n";
        let nsched = hls_key_schedule(none, "https://h/pl.m3u8").unwrap();
        assert!(nsched[0].is_some());
        assert!(nsched[1].is_none());
        assert!(hls_key_schedule("#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"", "https://h/x").is_err());
        assert!(hls_key_schedule("#EXT-X-KEY:METHOD=AES-128", "https://h/x").is_err());
        // EXT-X-MAP init sections.
        let map = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6,\ns0.ts\n";
        assert_eq!(
            hls_map_init(map, "https://h/v/pl.m3u8").unwrap().as_deref(),
            Some("https://h/v/init.mp4")
        );
        assert!(hls_map_init("#EXTM3U\n#EXTINF:6,\ns0.ts\n", "https://h/x").unwrap().is_none());
        assert!(hls_map_init("#EXT-X-MAP:URI=\"i.mp4\",BYTERANGE=\"16@0\"", "https://h/x").is_err());
        // IV forms: explicit hex, else sequence-based.
        assert_eq!(parse_hex_iv("0x00000000000000000000000000000001"), Some(1u128.to_be_bytes()));
        assert_eq!(parse_hex_iv("short"), None);
        // ISO durations + counts.
        assert_eq!(parse_pt_duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(parse_pt_duration("PT60S"), Some(60.0));
        assert_eq!(parse_pt_duration("nope"), None);
        assert_eq!(dash_seg_count(Some(60.0), 6.0), Some(10));
        assert_eq!(dash_seg_count(Some(61.0), 6.0), Some(11));
        assert_eq!(dash_seg_count(None, 6.0), None);
        assert_eq!(dash_seg_count(Some(60.0), 0.0), None);
        assert_eq!(dash_expand("seg-$Number$.m4s", 7, "v1", 8000), "seg-7.m4s");
        assert_eq!(dash_expand("seg-$Number%05d$.m4s", 7, "v1", 8000), "seg-00007.m4s");
        assert_eq!(dash_expand("$RepresentationID$/$Bandwidth$/s.m4s", 1, "vid", 8000), "vid/8000/s.m4s");
        // MPD structure parse.
        let mpd = r#"<?xml version="1.0"?><MPD mediaPresentationDuration="PT60S" type="static"><Period><AdaptationSet mimeType="video/mp4"><SegmentTemplate timescale="30" duration="180" startNumber="1" media="v-$Number$.m4s" initialization="v-init.mp4"/><Representation id="v1" bandwidth="5000000" width="1920" height="1080" codecs="avc1.640028"></Representation></AdaptationSet><AdaptationSet mimeType="audio/mp4"><SegmentTemplate timescale="44" duration="100" media="a-$Number$.m4s" initialization="a-init.mp4"/><Representation id="a1" bandwidth="128000" codecs="mp4a.40.2"></Representation></AdaptationSet></Period></MPD>"#;
        let (vv, aa) = parse_mpd(mpd).expect("mpd");
        assert_eq!(vv.len(), 1);
        assert_eq!(aa.len(), 1);
        assert_eq!(vv[0].width, 1920);
        assert_eq!(vv[0].seg_media.as_deref(), Some("v-$Number$.m4s"));
        assert_eq!(vv[0].seg_init.as_deref(), Some("v-init.mp4"));
        assert_eq!(vv[0].seg_dur_secs, Some(6.0));
        assert_eq!(dash_seg_count(Some(60.0), vv[0].seg_dur_secs.unwrap()), Some(10));
        assert!(parse_mpd("<MPD></MPD>").is_err());
        let drm = mpd.replace("<Period>", "<Period><AdaptationSet><ContentProtection/></AdaptationSet>");
        let _ = drm;
    }

    #[test]
    fn dash_timeline() {
        // Uniform repeat timeline: exact count.
        let tl = "<SegmentTimeline><S t=\"0\" d=\"6\" r=\"2\" /></SegmentTimeline>";
        assert_eq!(timeline_count(tl, 1.0, None).unwrap(), Some(3));
        // Multi-entry uniform + contiguous: exact count.
        let tl2 = "<SegmentTimeline><S t=\"0\" d=\"6\" /><S t=\"6\" d=\"6\" r=\"1\" /></SegmentTimeline>";
        assert_eq!(timeline_count(tl2, 1.0, None).unwrap(), Some(3));
        // No timeline: None (duration math fallback).
        assert_eq!(timeline_count("<SegmentTemplate duration=\"6\" />", 1.0, None).unwrap(), None);
        // Gap: loud error, never a miscount.
        let gap = "<SegmentTimeline><S t=\"0\" d=\"6\" /><S t=\"12\" d=\"6\" /></SegmentTimeline>";
        assert!(timeline_count(gap, 1.0, None).is_err());
        // Varying durations: loud error.
        let vary = "<SegmentTimeline><S t=\"0\" d=\"6\" /><S t=\"6\" d=\"4\" /></SegmentTimeline>";
        assert!(timeline_count(vary, 1.0, None).is_err());
        // Open repeat with known period: 60 s / 6 s segments = 10.
        let open = "<SegmentTimeline><S t=\"0\" d=\"6\" r=\"-1\" /></SegmentTimeline>";
        assert_eq!(timeline_count(open, 1.0, Some(60.0)).unwrap(), Some(10));
        // Open repeat without period: loud error.
        assert!(timeline_count(open, 1.0, None).is_err());
        // Oversize timeline: None (caller reports live/oversize cleanly).
        let big = "<SegmentTimeline><S t=\"0\" d=\"1\" r=\"5000\" /></SegmentTimeline>";
        assert_eq!(timeline_count(big, 1.0, None).unwrap(), None);
        // End-to-end: rep carries the timeline count over duration math.
        let mpd = r#"<?xml version="1.0"?><MPD type="static"><Period><AdaptationSet mimeType="video/mp4"><SegmentTemplate timescale="1" startNumber="1" media="s-$Number$.m4s"/><SegmentTimeline><S t="0" d="6" r="2" /></SegmentTimeline><Representation id="v1" bandwidth="1000" width="640" height="360" codecs="avc1.64001e"></Representation></AdaptationSet></Period></MPD>"#;
        let (vv, _) = parse_mpd(mpd).expect("timeline mpd");
        assert_eq!(vv[0].timeline_count, Some(3));
    }

    #[test]
    fn hls_helpers() {
        // playlists recognised, mpd rejected from generic resolve
        assert!(is_playlist_url("https://cdn.example/v/playlist.m3u8?x=1"));
        assert!(is_playlist_url("https://cdn.example/v/manifest.mpd"));
        assert!(!is_playlist_url("https://cdn.example/v/clip.mp4"));
        assert!(has_direct_ext("https://cdn.example/a/b.mp4?token=1"));
        assert!(!has_direct_ext("https://cdn.example/a/playlist.m3u8"));
        // master variant pick: highest bandwidth wins
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\nhi.m3u8\n";
        assert_eq!(
            hls_pick_variant(master, "https://cdn.example/v/master.m3u8").as_deref(),
            Some("https://cdn.example/v/hi.m3u8")
        );
        assert_eq!(hls_pick_variant("#EXTM3U\n#EXT-X-TARGETDURATION:6\n", "https://h/x.m3u8"), None);
        // media segments: comments skipped, relative resolved
        let media = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg0.ts\n#EXTINF:6.0,\nhttps://other.example/s1.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            hls_segments(media, "https://cdn.example/v/pl.m3u8"),
            vec![
                "https://cdn.example/v/seg0.ts".to_string(),
                "https://other.example/s1.ts".to_string()
            ]
        );
        // ld+json embedUrl accepted, playlists skipped
        let ld = r#"<html><head><title>T</title><script type="application/ld+json">{"@type":"VideoObject","embedUrl":"https://cdn.example/e/v.mp4"}</script></head></html>"#;
        let hit = resolve_generic(ld, "https://example.com/p").expect("ld+json");
        assert_eq!(hit.0, "https://cdn.example/e/v.mp4");
        let pl = r#"<html><head><meta property="og:video" content="https://cdn.example/v/pl.m3u8"></head></html>"#;
        assert_eq!(resolve_generic(pl, "https://example.com/p"), None);
    }

    #[test]
    fn aes_roundtrip() {
        let key = [0x0Bu8; 16];
        let iv = 7u128.to_be_bytes();
        let plain: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
        let enc = aes_cbc_encrypt_test(&plain, &key, &iv);
        assert_eq!(enc.len(), plain.len());
        assert_ne!(enc, plain);
        let dec = aes128_cbc_decrypt(&enc, &key, &iv).expect("decrypt");
        assert_eq!(dec, plain);
        assert!(aes128_cbc_decrypt(&enc[..enc.len() - 1], &key, &iv).is_err());
        assert!(aes128_cbc_decrypt(&enc, &[0u8; 16], &iv).is_ok()); // wrong key still yields blocks
    }

    #[tokio::test]
    async fn limiter_and_checksum() {
        // Unlimited limiter never waits.
        let free = RateLimiter::new(0);
        free.consume(1_000_000).await;
        // Scheduler override: global atomic wins over the fixed cap.
        let g = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
        let lim = RateLimiter::new(1000).with_global(Some(g.clone()));
        assert_eq!(lim.effective_limit(), 1000);
        g.store(100, Ordering::SeqCst);
        assert_eq!(lim.effective_limit(), 100);
        g.store(0, Ordering::SeqCst);
        assert_eq!(lim.effective_limit(), 0);
        lim.consume(10).await; // unlimited under override: returns fast
        // Empty checksum request is a no-op.
        let dir = std::env::temp_dir();
        let p = dir.join("download_craft_checksum_test.bin");
        std::fs::write(&p, b"abc").unwrap();
        verify_checksum(&p, "").await.unwrap();
        // Known SHA-256 of "abc".
        verify_checksum(
            &p,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .await
        .unwrap();
        assert!(verify_checksum(&p, &"0".repeat(64)).await.is_err());
        assert!(verify_checksum(&p, "not-hex").await.is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Minimal in-process HTTP/1.1 fixture (localhost only): serves a
    /// deterministic body with Range support, a redirect, a Range-ignoring
    /// endpoint, and a flaky endpoint that fails its first requests.
    /// Exercises the real engine over real sockets — no external network.
    /// Extra: HLS playlists/segments (plain + AES-128) and DASH manifests.
    fn aes_cbc_encrypt_test(plain: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
        crate::aes128::cbc_encrypt(key, iv, plain)
    }

    async fn serve_conn(
        mut s: tokio::net::TcpStream,
        body: std::sync::Arc<Vec<u8>>,
        flaky_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fb_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut raw = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match s.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&chunk[..n]);
                    if raw.len() > 1_000_000
                        || raw.windows(4).any(|w| w == b"\r\n\r\n")
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let head = String::from_utf8_lossy(&raw).into_owned();
        let mut lines = head.lines();
        let rl = lines.next().unwrap_or("").to_string();
        let mut it = rl.split_whitespace();
        let method = it.next().unwrap_or("").to_string();
        let path = it.next().unwrap_or("/").to_string();
        let mut range: Option<(u64, u64)> = None;
        for l in lines {
            if l.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                if k.trim().eq_ignore_ascii_case("range") {
                    let v = v.trim().strip_prefix("bytes=").unwrap_or("").trim();
                    if let Some((a, b)) = v.split_once('-') {
                        if let Ok(start) = a.trim().parse::<u64>() {
                            let end = b.trim().parse::<u64>().unwrap_or(u64::MAX);
                            range = Some((start, end));
                        }
                    }
                }
            }
        }
        let is_head = method.eq_ignore_ascii_case("HEAD");
        // Text responses served immediately (playlists, manifests).
        let text_resp: Option<(&str, String)> = match path.as_str() {
            "/hls/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlo.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000\nhi.m3u8\n".to_string(),
            )),
            "/hls/lo.m3u8" | "/hls/hi.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXTINF:6.0,\ns2.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/hls-enc/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXTINF:6.0,\ns2.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/hls-rot/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI=\"k1\"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"k2\",IV=0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n#EXTINF:6.0,\ns2.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/hls-map/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/dash/vonly.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT18S\" type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            "/dash/both.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT18S\" type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet><AdaptationSet mimeType=\"audio/mp4\"><Representation id=\"a1\" bandwidth=\"128000\" codecs=\"mp4a.40.2\"><BaseURL>a.m4a</BaseURL></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            "/dash/tpl.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT18S\" type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><SegmentTemplate timescale=\"1\" duration=\"6\" startNumber=\"1\" media=\"seg-$Number$.m4s\" initialization=\"init.mp4\"/><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            "/dash/tl.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><SegmentTemplate timescale=\"1\" startNumber=\"1\" media=\"seg-$Number$.m4s\" initialization=\"init.mp4\"/><SegmentTimeline><S t=\"0\" d=\"6\" r=\"2\" /></SegmentTimeline><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            _ => None,
        };
        if let Some((ct, t)) = text_resp {
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                t.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(t.as_bytes()).await;
            }
            let _ = s.flush().await;
            return;
        }
        // Facebook-CDN simulations (clean-but-early closes, like fbcdn):
        // /fbcut honors Range but cuts every connection after 64 KiB while
        // advertising the full window (headers promise, body stops).
        // /fbempty answers ranges with an empty 200 (`got 0 of …`).
        // /fbexpire serves twice, then 403s (signed-URL expiry mid-file).
        // /fb200cut ignores Range (full 200) but cuts every body at 64 KiB.
        const FB_CUT: usize = 64 * 1024;
        let fb_path = path.trim_start_matches('/');
        let fb_mode = ["fbcut", "fbempty", "fbexpire", "fb200cut"]
            .iter()
            .find(|m| fb_path == **m)
            .copied();
        // Strict googlevideo-style edge: HEAD, open-ended, and >2 MiB
        // ranges 403 (verified live); small bounded ranges serve 206.
        if fb_path == "strict" {
            const STRICT_MAX: u64 = 2 * 1024 * 1024;
            let bad = match range {
                None => true, // plain/HEAD without Range
                Some((s, e)) => e == u64::MAX || e.saturating_sub(s).saturating_add(1) > STRICT_MAX,
            };
            if bad {
                let r = "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let len = body.len() as u64;
            let (start, end) = range.unwrap();
            let end = end.min(len.saturating_sub(1));
            if start >= len {
                let r = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let payload = &body[start as usize..=end as usize];
            let r = format!("HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
            let _ = s.write_all(r.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(payload).await;
            }
            let _ = s.flush().await;
            return;
        }
        if let Some(mode) = fb_mode {
            let n = fb_hits.fetch_add(1, Ordering::SeqCst);
            if mode == "fbexpire" && n >= 2 {
                let r = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            if mode == "fbempty" {
                let r = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let len = body.len() as u64;
            if mode == "fb200cut" {
                // Range ignored: whole body promised, cut early.
                let cut = (FB_CUT as u64).min(len);
                let r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                );
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.write_all(&body[..cut as usize]).await;
                let _ = s.flush().await;
                return;
            }
            // fbcut / fbexpire-early: honor the range, then cut the body.
            let (start, end) = range.unwrap_or((0, len.saturating_sub(1)));
            let end = end.min(len.saturating_sub(1));
            if start >= len {
                let r = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let window = &body[start as usize..=end as usize];
            let cut = window.len().min(FB_CUT);
            let r = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                window.len()
            );
            let _ = s.write_all(r.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(&window[..cut]).await;
            }
            let _ = s.flush().await;
            return;
        }
        if path == "/hls-enc/key" {
            let key = [0x0Bu8; 16];
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 16\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(resp.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(&key).await;
            }
            let _ = s.flush().await;
            return;
        }
        // Rotating keys (P3): k1 serves segments 0-1 (sequence IVs), k2
        // serves segment 2 (explicit IV in the playlist).
        if path == "/hls-rot/k1" || path == "/hls-rot/k2" {
            let key = if path.ends_with("k1") { [0x0Au8; 16] } else { [0x0Bu8; 16] };
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 16\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(resp.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(&key).await;
            }
            let _ = s.flush().await;
            return;
        }
        // Binary payload override for segment/media paths (else full body).
        let third = body.len() / 3;
        let seg_bin: Option<Vec<u8>> = (|| {
            if let Some(n) = path
                .strip_prefix("/hls/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 3 {
                    return Some(body[n * third..(n + 1) * third].to_vec());
                }
            }
            if let Some(n) = path
                .strip_prefix("/hls-enc/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 3 {
                    let plain = &body[n * third..(n + 1) * third];
                    return Some(aes_cbc_encrypt_test(plain, &[0x0Bu8; 16], &(n as u128).to_be_bytes()));
                }
            }
            // Rotating-key segments (P3): s0/s1 under k1 (sequence IVs),
            // s2 under k2 (the playlist's explicit IV).
            if let Some(n) = path
                .strip_prefix("/hls-rot/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 3 {
                    let plain = &body[n * third..(n + 1) * third];
                    if n < 2 {
                        return Some(aes_cbc_encrypt_test(plain, &[0x0Au8; 16], &(n as u128).to_be_bytes()));
                    }
                    return Some(aes_cbc_encrypt_test(plain, &[0x0Bu8; 16], &[0xAAu8; 16]));
                }
            }
            // EXT-X-MAP playlist (P3): plaintext segments + fixed init.
            if let Some(n) = path
                .strip_prefix("/hls-map/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 2 {
                    return Some(body[n * third..(n + 1) * third].to_vec());
                }
            }
            if path == "/hls-map/init.mp4" {
                return Some(b"HLSMAPINIT".to_vec());
            }
            if path == "/dash/v.mp4" {
                return Some(body[..body.len() / 2].to_vec());
            }
            if path == "/dash/a.m4a" {
                return Some(body[body.len() / 2..].to_vec());
            }
            // Templated DASH segments: thirds prove ordered assembly, and a
            // fixed init marker proves headers lead the stream.
            if let Some(n) = path
                .strip_prefix("/dash/seg-")
                .and_then(|s| s.strip_suffix(".m4s"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if (1..=3).contains(&n) {
                    let i = n - 1;
                    return Some(body[i * third..(i + 1) * third].to_vec());
                }
            }
            if path == "/dash/init.mp4" {
                return Some(b"DASHINIT".to_vec());
            }
            None
        })();
        let body: &[u8] = seg_bin.as_deref().unwrap_or(&body);
        let len = body.len() as u64;
        // (status, extra headers, payload range)
        let (status, extra, payload): (&str, String, &[u8]) = if path == "/redirect" {
            ("HTTP/1.1 302 Found", "Location: /file\r\n".to_string(), &[])
        } else if path == "/flaky"
            && flaky_hits.fetch_add(1, Ordering::SeqCst) < 3
        {
            // Fail the probe HEAD + probe fallback + first GET: the engine
            // must retry the single stream and still finish.
            ("HTTP/1.1 500 Internal Server Error", String::new(), &[])
        } else if path == "/norange" {
            ("HTTP/1.1 200 OK", "Accept-Ranges: none\r\n".to_string(), &body[..])
        } else if path == "/forbidden" {
            // Permanent refusal: engine must fail fast with guidance.
            ("HTTP/1.1 403 Forbidden", String::new(), &[])
        } else if let Some((start, end)) = range {
            let end = end.min(len.saturating_sub(1));
            if start >= len {
                (
                    "HTTP/1.1 416 Range Not Satisfiable",
                    format!("Content-Range: bytes */{len}\r\n"),
                    &[],
                )
            } else {
                (
                    "HTTP/1.1 206 Partial Content",
                    format!(
                        "Accept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\n"
                    ),
                    &body[start as usize..=end as usize],
                )
            }
        } else {
            (
                "HTTP/1.1 200 OK",
                "Accept-Ranges: bytes\r\n".to_string(),
                &body[..],
            )
        };
        let mut resp = format!(
            "{status}\r\nContent-Type: application/octet-stream\r\n{extra}"
        );
        // Error responses carry no Content-Length so the probe learns nothing.
        if !status.contains(" 500 ") {
            resp.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        resp.push_str("Connection: close\r\n\r\n");
        let _ = s.write_all(resp.as_bytes()).await;
        if !is_head && !payload.is_empty() {
            let _ = s.write_all(payload).await;
        }
        let _ = s.flush().await;
    }

    fn matrix_opts() -> EngineOpts {
        EngineOpts {
            user_agent: "idm-test/1".to_string(),
            timeout_secs: 15,
            max_retries: 4,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn facebook_cdn_matrix() {
        // Simulates the fbcdn failure modes from the field: mid-range cuts,
        // empty ranges, mid-file expiry, and range-ignoring servers — over
        // real sockets, asserting the engine finishes, fails fast, or fails
        // honestly (never a truncated file masquerading as success).
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 300 * 1024; // < 1 MiB chunk: resume aggregation
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let want = body.clone();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));
        let fb_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fb fixture");
        let addr = listener.local_addr().expect("fb fixture addr");
        let fb_h = fb_hits.clone();
        let srv = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((s, _)) => {
                        let b = body.clone();
                        let f = flaky_hits.clone();
                        let h = fb_h.clone();
                        tokio::spawn(async move { serve_conn(s, b, f, h).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_fb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = || EngineOpts {
            user_agent: "idm-test/1".to_string(),
            timeout_secs: 5,
            max_retries: 2, // keep the suite fast; policy is what matters
            ..Default::default()
        };
        let client = opts().build_client().unwrap();

        // 1. Range probe semantics: 206 → total, 200 → none, 403 → dead.
        assert_eq!(
            probe_range_once(&client, &opts(), &format!("{base}/fbcut"), Some(LEN as u64)).await.ok(),
            Some(Some(LEN as u64))
        );
        assert_eq!(
            probe_range_once(&client, &opts(), &format!("{base}/fb200cut"), Some(LEN as u64)).await.ok(),
            Some(None)
        );
        assert!(
            probe_range_once(&client, &opts(), &format!("{base}/forbidden"), Some(LEN as u64))
                .await
                .is_err()
        );

        // 2. Cutter honors ranges but cuts every connection at 64 KiB:
        // chunked sequential ranges must still assemble the exact file.
        let dest = dir.join("fbcut.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbcut"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect("chunked survives cuts");
        assert_eq!(std::fs::read(&dest).unwrap(), want);
        assert_eq!(shared.downloaded.load(Ordering::SeqCst), LEN as u64);

        // 3. Resume into chunked: stale prefix is continued, not rewritten.
        let dest = dir.join("fbresume.bin");
        std::fs::write(dest.with_extension("dcpart"), &want[..100 * 1024]).unwrap();
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbcut"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect("chunked resume");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 4. Empty ranges fail loudly with cause — never a phantom file.
        let dest = dir.join("fbempty.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbempty"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("empty ranges must fail");
        assert!(
            format!("{err:#}").contains("empty range"),
            "must name the cause, got: {err:#}"
        );
        assert!(!dest.exists(), "no truncated file may be finalized");

        // 5. Mid-file expiry: progress then 403 → "expired", not generic.
        // (Reset the fixture counter: earlier scenarios already spent it.)
        fb_hits.store(0, Ordering::SeqCst);
        let dest = dir.join("fbexpire.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbexpire"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("expiry must fail");
        assert!(
            format!("{err:#}").contains("expired"),
            "must say expired, got: {err:#}"
        );

        // 6. Range-ignoring cutter: honest failure, no truncated success.
        let dest = dir.join("fb200cut.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fb200cut"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("range-hostile cutter must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("tries:"),
            "must carry the attempt summary, got: {msg}"
        );
        assert!(!dest.exists(), "no truncated file may be finalized");

        // 7. End-to-end single stream through the cutter (unknown-total path).
        let dest = dir.join("fbsingle.bin");
        download(
            format!("{base}/fbcut"),
            dest.clone(),
            1,
            opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("single survives cuts");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        let _ = std::fs::remove_dir_all(&dir);
        srv.abort();
    }

    #[tokio::test]
    async fn strict_edge_matrix() {
        // googlevideo SABR-style edge (verified live): HEAD, open-ended,
        // and >2 MiB ranges 403; small bounded ranges serve 206. Chunked
        // sequential ranges must complete; big single ranges must fail
        // loudly (this is why strict hosts route to chunked_stream).
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 5 * 1024 * 1024; // forces multiple 1 MiB chunks
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let want = body.clone();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));
        let fb_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind strict fixture");
        let addr = listener.local_addr().expect("strict fixture addr");
        let fb_h = fb_hits.clone();
        let srv = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((s, _)) => {
                        let b = body.clone();
                        let f = flaky_hits.clone();
                        let h = fb_h.clone();
                        tokio::spawn(async move { serve_conn(s, b, f, h).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_strict_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = || EngineOpts {
            user_agent: "idm-test/1".to_string(),
            timeout_secs: 5,
            max_retries: 2,
            ..Default::default()
        };
        let client = opts().build_client().unwrap();

        // 1. Small-range probe verifies liveness + total.
        assert_eq!(
            probe_range_once(&client, &opts(), &format!("{base}/strict"), Some(LEN as u64)).await.ok(),
            Some(Some(LEN as u64))
        );

        // 2. Chunked 1 MiB ranges complete byte-exact through the strict edge.
        let dest = dir.join("strict.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        chunked_stream(
            &client,
            &opts(),
            &format!("{base}/strict"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect("chunked survives strict edge");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 3. A 5 MiB single bounded range 403s immediately (final refusal —
        // by design it keeps its type for fallback routing, no summary).
        let dest = dir.join("strict-single.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = single_stream(
            &client,
            &opts(),
            &format!("{base}/strict"),
            &dest,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("5 MiB single range must 403");
        assert!(
            format!("{err:#}").contains("403"),
            "must name the refusal, got: {err:#}"
        );
        assert!(!dest.exists(), "no truncated file may be finalized");

        let _ = std::fs::remove_dir_all(&dir);
        srv.abort();
    }

    #[test]
    fn quality_selection() {
        assert_eq!(chrome_major("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"), Some(126));
        assert_eq!(chrome_major("download_craft/0.7.0"), None);
        assert_eq!(chrome_major(""), None);
        assert_eq!(quality_cap("1080p"), Some(1080));
        assert_eq!(quality_cap("1080p60"), Some(1080));
        assert_eq!(quality_cap("720p"), Some(720));
        assert_eq!(quality_cap(" 480p "), Some(480));
        assert_eq!(quality_cap(""), None);
        assert_eq!(quality_cap("HD"), None);
        assert_eq!(quality_cap("100p"), None);
        assert_eq!(quality_cap("5000p"), None);
        // No cap: higher always wins.
        assert!(better_stream(&None, 360, None));
        assert!(better_stream(&Some((360, String::new())), 720, None));
        assert!(!better_stream(&Some((720, String::new())), 360, None));
        // Cap 720: highest at/below wins; above-cap fallback goes closest.
        assert!(better_stream(&Some((1080, String::new())), 720, Some(720)));
        assert!(better_stream(&Some((360, String::new())), 720, Some(720)));
        assert!(!better_stream(&Some((720, String::new())), 1080, Some(720)));
        assert!(better_stream(&Some((1440, String::new())), 1080, Some(720)));
        assert!(!better_stream(&Some((1080, String::new())), 1440, Some(720)));
    }

    #[test]
    fn adaptive_pair_selection() {
        let fmts = serde_json::json!([
            {"url": "https://x/v360", "mimeType": "video/mp4", "width": 640, "height": 360},
            {"url": "https://x/v720", "mimeType": "video/mp4", "width": 1280, "height": 720},
            {"url": "https://x/v1080", "mimeType": "video/mp4", "width": 1920, "height": 1080},
            {"url": "https://x/a-low", "mimeType": "audio/mp4", "bitrate": 64000},
            {"url": "https://x/a-high", "mimeType": "audio/mp4", "bitrate": 128000},
            {"mimeType": "video/mp4", "width": 1920, "height": 1080},
        ]);
        let arr = fmts.as_array().unwrap();
        // No cap: best video + best audio.
        let (s, v, a) = pick_adaptive_pair(arr, None).unwrap();
        assert_eq!((s, v.as_str(), a.as_str()), (1080, "https://x/v1080", "https://x/a-high"));
        // Cap 720: exact fit wins, audio still best.
        let (s, v, a) = pick_adaptive_pair(arr, Some(720)).unwrap();
        assert_eq!((s, v.as_str(), a.as_str()), (720, "https://x/v720", "https://x/a-high"));
        // Cap 480: highest at/below wins (360 here).
        let (s, v, _) = pick_adaptive_pair(arr, Some(480)).unwrap();
        assert_eq!((s, v.as_str()), (360, "https://x/v360"));
        // No direct video at all: no pair (ciphered-only needs deciphering).
        let novid = serde_json::json!([
            {"url": "https://x/a", "mimeType": "audio/mp4", "bitrate": 64000},
        ]);
        assert!(pick_adaptive_pair(novid.as_array().unwrap(), None).is_none());
        // No audio at all: no pair (nothing to merge with).
        let noaud = serde_json::json!([
            {"url": "https://x/v", "mimeType": "video/mp4", "width": 640, "height": 360},
        ]);
        assert!(pick_adaptive_pair(noaud.as_array().unwrap(), None).is_none());
    }

    #[tokio::test]
    async fn local_http_matrix() {
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 3 * 1024 * 1024; // >= 1 MiB forces segmented path
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));
        let fb_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let addr = listener.local_addr().expect("fixture addr");
        let srv = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((s, _)) => {
                        let b = body.clone();
                        let f = flaky_hits.clone();
                        let h = fb_hits.clone();
                        tokio::spawn(async move { serve_conn(s, b, f, h).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_matrix_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 0. Error pages never poison totals: a 403 carrying
        // `Content-Length: 0` must probe as unknown (not total 0), or every
        // downstream check treats the file as already complete/empty.
        let pclient = matrix_opts().build_client().unwrap();
        let (t, r, _, _) = probe(&pclient, &matrix_opts(), &format!("{base}/forbidden")).await;
        assert_eq!((t, r), (None, false));
        let (t, r, _, _) = probe(&pclient, &matrix_opts(), &format!("{base}/file")).await;
        assert_eq!(t, Some(LEN as u64));
        assert!(r);

        // 1. Segmented multi-connection download, byte-exact.
        let dest = dir.join("seg.bin");
        let shared = Arc::new(SharedState::new());
        download(
            format!("{base}/file"),
            dest.clone(),
            4,
            matrix_opts(),
            shared.clone(),
        )
        .await
        .expect("segmented");
        let got = std::fs::read(&dest).unwrap();
        let want: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        assert_eq!(got, want);
        assert_eq!(shared.downloaded.load(Ordering::SeqCst), LEN as u64);

        // 2. Resume: stale .dcpart prefix must be continued, not rewritten.
        let dest = dir.join("resume.bin");
        std::fs::write(dest.with_extension("dcpart"), &want[..100 * 1024]).unwrap();
        download(format!("{base}/file"), dest.clone(), 1, matrix_opts(), Arc::new(SharedState::new()))
            .await
            .expect("resume");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 3. Redirect is followed to the real file.
        let dest = dir.join("redir.bin");
        download(
            format!("{base}/redirect"),
            dest.clone(),
            1,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("redirect");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 4. Server ignoring Range with a stale part restarts cleanly.
        let dest = dir.join("norange.bin");
        std::fs::write(dest.with_extension("dcpart"), b"stale-partial").unwrap();
        download(
            format!("{base}/norange"),
            dest.clone(),
            1,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("norange restart");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 5. Flaky endpoint: probe + first GET fail, retry still finishes.
        let dest = dir.join("flaky.bin");
        download(
            format!("{base}/flaky"),
            dest.clone(),
            1,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("flaky retry");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 6. Hard 403: fail fast with an actionable message (no retry storm).
        let dest = dir.join("denied.bin");
        let err = download(
            format!("{base}/forbidden"),
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect_err("403 must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("403"),
            "refusal must name the status, got: {msg}"
        );
        assert!(is_final_http_error(&final_err("HTTP 403 Forbidden".into())));
        assert!(!is_final_http_error(&anyhow!("HTTP 403 Forbidden")));
        assert!(!is_final_http_error(&anyhow!("network: reset by peer")));
        assert!(is_googlevideo_url("https://r1---sn-abc.googlevideo.com/videoplayback?x=1"));
        assert!(!is_googlevideo_url("https://cdn.example/v.mp4"));
        assert!(is_youtube_page("https://www.youtube.com/watch?v=x"));
        assert!(is_youtube_page("https://youtu.be/x"));
        assert!(!is_youtube_page("https://vimeo.com/1"));

        // 7. HLS plain VOD: master → best variant → parallel segments → exact.
        let dest = dir.join("hls.bin");
        download(
            format!("{base}/hls/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls vod");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 8. HLS AES-128: key fetch + per-segment decrypt → exact.
        let dest = dir.join("hlsaes.bin");
        download(
            format!("{base}/hls-enc/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls aes");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 8b. HLS key rotation (P3): s0/s1 under k1, s2 under k2+IV.
        let dest = dir.join("hlsrot.bin");
        download(
            format!("{base}/hls-rot/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls rotation");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 8c. HLS EXT-X-MAP (P3): init section leads the assembled file.
        let dest = dir.join("hlsmap.bin");
        download(
            format!("{base}/hls-map/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls map");
        let third = want.len() / 3;
        let mut want_map = b"HLSMAPINIT".to_vec();
        want_map.extend_from_slice(&want[..2 * third]);
        assert_eq!(std::fs::read(&dest).unwrap(), want_map);

        // 9. DASH video-only manifest → assembled track delivered.
        let dest = dir.join("dashv.bin");
        download(
            format!("{base}/dash/vonly.mpd"),
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("dash video-only");
        assert_eq!(std::fs::read(&dest).unwrap(), want[..want.len() / 2]);

        // 9b. DASH SegmentTemplate (init + 3 media segments): parallel
        // fetch with strictly ordered assembly (init must lead).
        let dest = dir.join("dashtpl.bin");
        download(
            format!("{base}/dash/tpl.mpd"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("dash template");
        let mut want_tpl = b"DASHINIT".to_vec();
        want_tpl.extend_from_slice(&want);
        assert_eq!(std::fs::read(&dest).unwrap(), want_tpl);

        // 9c. DASH SegmentTimeline (`r="2"` → 3 segments, no MPD duration):
        // count comes from the timeline, assembly identical to 9b.
        let dest = dir.join("dashtl.bin");
        download(
            format!("{base}/dash/tl.mpd"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("dash timeline");
        assert_eq!(std::fs::read(&dest).unwrap(), want_tpl);

        // 10. DASH video+audio without ffmpeg → loud guidance, not garbage.
        let dest = dir.join("dashboth.bin");
        let err = download(
            format!("{base}/dash/both.mpd"),
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect_err("dash mux needs ffmpeg");
        assert!(
            format!("{err:#}").contains("ffmpeg"),
            "mux failure must name ffmpeg, got: {err:#}"
        );

        // 11. Adaptive pair, audio-less leg → direct delivery, exact.
        let dest = dir.join("pairv.bin");
        download_adaptive(
            format!("{base}/file"),
            None,
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("adaptive video-only");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        srv.abort();
        std::fs::remove_dir_all(&dir).ok();
    }
}
