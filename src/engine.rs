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
fn io_message(e: &std::io::Error) -> String {
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
        }
    }
}

impl EngineOpts {
    pub fn build_client(&self) -> Result<reqwest::Client> {
        // HTTP/1.1 only: several media servers RST or truncate H2 streams
        // from non-browser TLS stacks, while plain H1 range GETs work.
        let mut b = reqwest::Client::builder()
            .user_agent(self.user_agent.clone())
            .redirect(reqwest::redirect::Policy::limited(10))
            .http1_only();
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

    fn get(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.get(url))
    }

    fn post(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.post(url))
    }

    fn head(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.head(url))
    }

    fn apply(&self, mut r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        r = r
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");
        if !self.referer.trim().is_empty() {
            r = r.header(reqwest::header::REFERER, self.referer.trim().to_string());
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

async fn wait_if_paused(shared: &Arc<SharedState>) -> Result<()> {
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

/// Shared token-bucket rate limiter (clean-room implementation).
/// Unlike a naive per-chunk `sleep(bytes/limit)`, the bucket smooths bursts
/// across all connections: tokens accrue at `limit_bps` and each chunk
/// waits only for the deficit. A single shared instance is cloned into
/// every segment task so the *global* budget is honored.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    limit_bps: u64,
    state: Arc<std::sync::Mutex<(f64, std::time::Instant)>>,
}

impl RateLimiter {
    pub fn new(limit_bps: u64) -> Self {
        Self {
            limit_bps,
            state: Arc::new(std::sync::Mutex::new((
                limit_bps as f64,
                std::time::Instant::now(),
            ))),
        }
    }

    pub async fn consume(&self, bytes: u64) {
        if self.limit_bps == 0 || bytes == 0 {
            return;
        }
        let wait_secs: f64 = {
            let mut g = self.state.lock().unwrap();
            let (ref mut tokens, ref mut last) = *g;
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(*last).as_secs_f64();
            *last = now;
            *tokens = (*tokens + elapsed * self.limit_bps as f64)
                .min(self.limit_bps as f64 * 2.0);
            *tokens -= bytes as f64;
            if *tokens < 0.0 {
                -(*tokens) / self.limit_bps as f64
            } else {
                0.0
            }
        };
        if wait_secs > 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs)).await;
        }
    }
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
            return anyhow!(
                "YouTube refused this link (403) — stream links expire and are locked to the browser session that created them: replay the video, reopen the ⬇ Download this video menu for a fresh quality row, and download that (progressive ≤720p saves as one ready file)"
            );
        }
        return anyhow!(
            "server refused (403 Forbidden) — it likely needs login or cookies: open the page in your browser and capture it with the Download Craft extension"
        );
    }
    if status == Some(reqwest::StatusCode::UNAUTHORIZED) {
        return anyhow!(
            "server needs a login (401) — add the username/password in the Add dialog's advanced section, or capture via the browser extension"
        );
    }
    match status {
        Some(s) => anyhow!("HTTP {s}"),
        None => anyhow!("network: {e}"),
    }
}

fn is_final_http_error(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    s.contains("403") || s.contains("401")
}

fn is_googlevideo_url(url: &str) -> bool {
    url.contains("googlevideo.com")
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
    // Try HEAD first.
    if let Ok(resp) = opts.head(client, url).send().await {
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
    // Fallback: GET bytes=0-0, parse Content-Range: bytes 0-0/12345
    if let Ok(resp) = opts
        .get(client, url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
    {
        let status = resp.status();
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
        return (total, false, None, disp);
    }
    (None, false, None, None)
}

/// Extract `filename` / `filename*` from a Content-Disposition header.
fn disposition_filename(h: &reqwest::header::HeaderMap) -> Option<String> {
    let v = h
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;
    for part in v.split(';') {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix("filename*=") {
            // RFC 5987: charset'lang'percent-encoded
            if let Some(enc) = rest.split('\'').nth(2) {
                let name = percent_decode_owned(enc);
                if !name.trim().is_empty() {
                    return Some(sanitize_filename(&name));
                }
            }
        }
    }
    for part in v.split(';') {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix("filename=") {
            let s = rest.trim().trim_matches('"').trim();
            if !s.is_empty() {
                return Some(sanitize_filename(s));
            }
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

/// Try to extract a direct media URL + title from an HTML video page.
/// Handles YouTube watch pages (progressive MP4 streams) plus generic
/// pages via og:video / contentUrl / bare .mp4 links.
pub async fn resolve_media(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    shared: &Arc<SharedState>,
) -> Option<(String, String)> {
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
            if let Some(hit) = resolve_youtube(&html) {
                dbg_resolve("page-formats", &hit.0);
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
    hit
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
) -> Option<(String, String)> {
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
        dbg_resolve("player-api-android", &hit.0);
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
        dbg_resolve("player-api", &hit.0);
        return Some(hit);
    }
    dbg_resolve("player-api", "<no-stream>");
    None
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
) -> Option<(String, String)> {
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
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = f
                .get("width")
                .and_then(|w| w.as_u64())
                .unwrap_or(0)
                .max(f.get("height").and_then(|h| h.as_u64()).unwrap_or(0));
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, url.to_string()));
            }
        }
    }
    best.map(|(_, u)| (with_ratebypass(&u), format!("{title}.mp4")))
}

fn resolve_youtube(html: &str) -> Option<(String, String)> {
    // ytInitialPlayerResponse = {...};  (balanced-brace scan)
    let marker = "ytInitialPlayerResponse = ";
    let start = html.find(marker)? + marker.len();
    let bytes = html.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        return None; // marker with no object (truncated page)
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
    let v: serde_json::Value = serde_json::from_str(&html[obj_start..=i]).ok()?;
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
            let score = f
                .get("width")
                .and_then(|w| w.as_u64())
                .unwrap_or(0)
                .max(f.get("height").and_then(|h| h.as_u64()).unwrap_or(0));
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, url.to_string()));
            }
        }
    }
    best.map(|(_, u)| (with_ratebypass(&u), format!("{title}.mp4")))
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

/// Plain HLS VOD download (clean-room, no IDM code): fetch playlist, follow
/// the best variant, concat media segments. Encrypted (EXT-X-KEY) and live
/// (no ENDLIST, ever-growing) playlists are rejected with guidance —
/// and DRM (Widevine/FairPlay) is never bypassed.
async fn download_hls(
    playlist_url: &str,
    dest: &PathBuf,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
) -> Result<()> {
    const MAX_SEGS: usize = 2000;
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
    if text.contains("#EXT-X-KEY") {
        return Err(anyhow!(
            "this HLS stream is encrypted (EXT-X-KEY) — encrypted/DRM streams are not downloaded. Use the extension MP4 qualities when the site offers them"
        ));
    }
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
    // Progress: single-part view (segment count unknown in bytes yet).
    {
        let mut totals = shared.part_total.lock().unwrap();
        let mut dones = shared.part_done.lock().unwrap();
        totals.clear();
        dones.clear();
        totals.push((0, segs.len() as u64));
        dones.push(0);
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    let limiter = std::sync::Arc::new(RateLimiter::new(opts.speed_limit_bps));
    let tmp = dest.with_extension("dcpart");
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .await?;
    let max_retries = opts.max_retries.max(1);
    for seg_url in &segs {
        wait_if_paused(shared).await?;
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        let mut attempt = 0;
        loop {
            attempt += 1;
            let got = opts.get(&client, seg_url).send().await;
            match got {
                Ok(resp) => {
                    let resp = resp.error_for_status().map_err(|e| anyhow!("segment HTTP: {e}"))?;
                    use futures_util::StreamExt;
                    let mut stream = resp.bytes_stream();
                    let mut ok = true;
                    while let Some(chunk) = stream.next().await {
                        wait_if_paused(shared).await?;
                        match chunk {
                            Ok(bytes) => {
                                use tokio::io::AsyncWriteExt;
                                out.write_all(&bytes).await.map_err(|e| anyhow!(io_message(&e)))?;
                                shared.downloaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
                                if let Ok(mut g) = shared.part_done.lock() {
                                    if let Some(slot) = g.first_mut() {
                                        *slot = slot.saturating_add(1);
                                    }
                                }
                                limiter.consume(bytes.len() as u64).await;
                            }
                            Err(e) => {
                                ok = false;
                                if attempt >= max_retries {
                                    return Err(anyhow!("segment network: {e}"));
                                }
                                break;
                            }
                        }
                    }
                    if ok {
                        break;
                    }
                }
                Err(e) => {
                    if attempt >= max_retries {
                        return Err(anyhow!("segment fetch: {e}"));
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
        }
    }
    use tokio::io::AsyncWriteExt;
    out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(out);
    // Adopt byte total now that assembly is done.
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
/// segment-by-segment; DASH (.mpd) and encrypted HLS are rejected loudly.
pub async fn download(
    mut url: String,
    mut dest: PathBuf,
    connections: usize,
    mut opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    // Route playlists before the progressive path so we never save a
    // playlist file as if it were the video.
    if is_playlist_url(&url) {
        if url.to_ascii_lowercase().split(['?', '#']).next().unwrap_or("").ends_with(".mpd")
            || url.contains(".mpd")
        {
            return Err(anyhow!(
                "this is a DASH manifest (.mpd) — not supported yet (needs segment muxing). Use the browser extension qualities (MP4) or the page link for a progressive stream"
            ));
        }
        return download_hls(&url, &dest, &opts, &shared).await;
    }
    let client = opts.build_client()?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let (mut total_opt, mut supports_range, ctype, disp) = probe(&client, &opts, &url).await;
    shared
        .resumable
        .store(supports_range, Ordering::SeqCst);
    if opts.allow_rename {
        if let Some(n) = disp {
            dest.set_file_name(&n);
            shared.resolved_name.lock().unwrap().replace(n);
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
            Some((media_url, title)) => {
                url = media_url;
                // media servers expect the page as referer
                if opts.referer.trim().is_empty() {
                    opts.referer = page_url;
                }
                if !title.trim().is_empty() {
                    let name = sanitize_filename(&title);
                    if opts.allow_rename {
                        dest.set_file_name(&name);
                        shared.resolved_name.lock().unwrap().replace(name);
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
                        shared.resolved_name.lock().unwrap().replace(n);
                    }
                }
            }
            None => {
                if shared.rate_limited.load(Ordering::SeqCst) {
                    return Err(anyhow!(
                        "YouTube is rate-limiting this network right now (bot check) — wait a few minutes and retry, or play the video in your browser and capture it with the Download Craft extension"
                    ));
                }
                return Err(anyhow!(
                    "page has no downloadable video link — open it in the browser and use the extension"
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
                shared.resolved_name.lock().unwrap().replace(name);
            }
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }

    let conns = connections.clamp(1, 16);

    // One shared bucket so N segments together honor the global cap.
    let limiter = std::sync::Arc::new(RateLimiter::new(opts.speed_limit_bps));
    // Transfer attempts (max 2). When a googlevideo link 403s on the first
    // attempt and we know the watch page (referer), the stream URL was
    // almost surely stale/session-locked: resolve ONE fresh link via the
    // player API and retry once instead of failing outright.
    let mut retried_fresh = false;
    loop {
        let use_segmented =
            supports_range && matches!(total_opt, Some(t) if t >= 1024 * 1024) && conns > 1;
        let r = if use_segmented {
            segmented(
                &client,
                &opts,
                &url,
                &dest,
                total_opt.unwrap(),
                conns,
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
                    Some((fresh, _)) if fresh != url && is_googlevideo_url(&fresh) => {
                        // Drop partials of the refused stream: resuming them
                        // against a different URL would corrupt the file.
                        let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        shared.downloaded.store(0, Ordering::SeqCst);
                        url = fresh;
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

async fn single_stream(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    // Retry the whole stream (resuming from the .dcpart file when the
    // server honors ranges), mirroring the per-segment retry policy.
    let max_retries = opts.max_retries.max(1);
    let mut attempt = 0;
    loop {
        attempt += 1;
        match single_stream_once(client, opts, url, dest, &shared, &limiter).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst)
                    || e.to_string() == "cancelled"
                    || is_final_http_error(&e)
                    || attempt >= max_retries
                {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
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
    // Resume support: continue from existing .part file if present.
    let tmp = dest.with_extension("dcpart");
    let mut start_at: u64 = 0;
    if tmp.exists() {
        start_at = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    }

    // Always request a range (even from 0): some media servers truncate
    // plain 200 responses but serve full content for range requests.
    let mut req = opts.get(client, url);
    req = req.header(reqwest::header::RANGE, format!("bytes={start_at}-"));
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
    // Adopt server-advertised total when we didn't have one.
    if !shared.has_total.load(Ordering::SeqCst) {
        if let Some(len) = resp.content_length() {
            shared.total.store(len + start_at, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
    }
    // Single part view for the details dialog.
    {
        let total = shared.total.load(Ordering::SeqCst);
        let mut totals = shared.part_total.lock().unwrap();
        let mut dones = shared.part_done.lock().unwrap();
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
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
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
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
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

    let chunk = total / connections as u64;

    let mut handles = Vec::new();
    {
        let mut totals = shared.part_total.lock().unwrap();
        let mut dones = shared.part_done.lock().unwrap();
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
        let (start, end) = shared.part_total.lock().unwrap()[i];
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
    let mut attempt = 0;
    loop {
        attempt += 1;
        // each attempt restarts this part from scratch: roll back the
        // previous attempt's contribution to the global counter first.
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.get_mut(part_idx) {
                shared.downloaded.fetch_sub(*slot, Ordering::SeqCst);
                *slot = 0;
            }
        }
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
                if e.to_string() == "cancelled"
                    || is_final_http_error(&e)
                    || attempt >= max_retries
                {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
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
    let resp = opts
        .get(client, url)
        .header(reqwest::header::RANGE, format!("bytes={}-{}", start, end))
        .send()
        .await?
        .error_for_status()
        .map_err(|e| http_status_err(url, e))?;

    let mut file = tokio::fs::File::create(part).await?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        wait_if_paused(shared).await?;
        let bytes = chunk.map_err(|e| anyhow!("network: {e}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.get_mut(part_idx) {
                *slot = slot.saturating_add(bytes.len() as u64);
            }
        }
        limiter.consume(bytes.len() as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
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
        let empty = reqwest::header::HeaderMap::new();
        assert_eq!(disposition_filename(&empty), None);
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

    #[tokio::test]
    async fn limiter_and_checksum() {
        // Unlimited limiter never waits.
        let free = RateLimiter::new(0);
        free.consume(1_000_000).await;
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
    async fn serve_conn(
        mut s: tokio::net::TcpStream,
        body: std::sync::Arc<Vec<u8>>,
        flaky_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
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
    async fn local_http_matrix() {
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 3 * 1024 * 1024; // >= 1 MiB forces segmented path
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));

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
                        tokio::spawn(async move { serve_conn(s, b, f).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_matrix_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

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
        assert!(is_final_http_error(&anyhow!("HTTP 403 Forbidden")));
        assert!(!is_final_http_error(&anyhow!("network: reset by peer")));
        assert!(is_googlevideo_url("https://r1---sn-abc.googlevideo.com/videoplayback?x=1"));
        assert!(!is_googlevideo_url("https://cdn.example/v.mp4"));
        assert!(is_youtube_page("https://www.youtube.com/watch?v=x"));
        assert!(is_youtube_page("https://youtu.be/x"));
        assert!(!is_youtube_page("https://vimeo.com/1"));

        srv.abort();
        std::fs::remove_dir_all(&dir).ok();
    }
}
