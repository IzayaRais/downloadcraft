//! Native-messaging host for the browser extension (`--native-host`).
//!
//! Protocol (Chrome/Edge/Firefox native messaging = 4-byte LE length + UTF-8 JSON):
//!   -> {"type":"ping"}                                  <- {"type":"pong","version":1}
//!   -> {"type":"download","url":"https://...","filename":"optional",
//!       "pageUrl":"https://page...","referer":"...","cookie":"...","mime":"...",
//!       "headers":{"origin":"...","authorization":"...","x-...":"..."}}
//!                                                      <- {"type":"ok","queued":true}
//!
//! Download requests are appended to an inbox file that the GUI polls and
//! imports as queued (auto-started) downloads.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct Incoming {
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    filename: Option<String>,
    /// Page the download was captured from (IDM-style referer fallback).
    #[serde(default, alias = "pageUrl")]
    page_url: Option<String>,
    #[serde(default)]
    referer: Option<String>,
    #[serde(default)]
    cookie: Option<String>,
    /// Origin / Authorization / x-* headers the player sent with this media.
    #[serde(default)]
    headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    mime: Option<String>,
    /// Browser-measured size bytes (HEAD/Range from the page session).
    /// The engine can't always re-probe media URLs (403 off-session), so
    /// the dialog prefers this over probing.
    #[serde(default)]
    size: Option<u64>,
    /// Adaptive pair audio URL (video-only + audio merge). The extension
    /// sends these in camelCase (`pairUrl`/`pairMime`) like `pageUrl`, so
    /// without the aliases the pair was silently dropped and the merge never
    /// happened.
    #[serde(default, alias = "pairUrl")]
    pub pair_url: Option<String>,
    #[serde(default, alias = "pairMime")]
    pub pair_mime: Option<String>,
    /// Batch presence check (`{"type":"query"}`): which of these URLs are
    /// already queued. Answered from queue.json without side effects.
    /// (Only read on Incoming; stored nowhere.)
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    /// Preferred YouTube quality from the extension quality menu.
    #[serde(default)]
    pub quality: Option<String>,
    /// Media duration in seconds (page player / JSON, best-effort).
    /// Shown in the confirm dialog and checked against the completed file.
    #[serde(default)]
    pub dur: Option<u64>,
    #[serde(default, alias = "audioExpected")]
    pub audio_expected: bool,
}

#[derive(Debug, Serialize)]
struct Reply {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    urls: Option<Vec<String>>,
}

impl Reply {
    fn ok() -> Self {
        Reply { kind: "ok".into(), version: None, queued: Some(true), error: None, urls: None }
    }
    fn err(e: impl Into<String>) -> Self {
        Reply { kind: "error".into(), version: None, queued: None, error: Some(e.into()), urls: None }
    }
}

/// Which of `urls` are already in the shared queue (read-only, locked).
fn queued_urls(urls: &[String]) -> Vec<String> {
    if urls.is_empty() {
        return Vec::new();
    }
    let _guard = crate::model::fs_lock("queue");
    if !_guard.owned() { return Vec::new(); }
    let items: Vec<crate::model::DownloadItem> = crate::model::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("queue.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    urls.iter()
        .filter(|u| items.iter().any(|d| &d.url == *u))
        .cloned()
        .collect()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InboxItem {
    pub url: String,
    pub filename: Option<String>,
    pub received_at: String,
    /// Page URL the media was captured from (used as Referer fallback).
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
    /// Captured request headers (credentials: kept in the private inbox,
    /// never on a command line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub pair_url: Option<String>,
    #[serde(default)]
    pub pair_mime: Option<String>,
    /// Batch presence check (`{"type":"query"}`): which of these URLs are
    /// already queued. Answered from queue.json without side effects.
    /// (Only read on Incoming; stored nowhere.)
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    /// Preferred YouTube quality from the extension quality menu.
    #[serde(default)]
    pub quality: Option<String>,
    /// Media duration in seconds (page player / JSON, best-effort).
    /// Shown in the confirm dialog and checked against the completed file.
    #[serde(default)]
    pub dur: Option<u64>,
    #[serde(default, alias = "audioExpected")]
    pub audio_expected: bool,
}

pub fn inbox_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|p| p.join("native_inbox.json"))
}

/// One stdin frame: clean EOF (browser gone → exit the host) vs a broken
/// frame (reply with an error and keep serving — one bad message must not
/// kill the host and abandon everything behind it).
enum HostFrame {
    Eof,
    Bad(String),
    Msg(Incoming),
}

fn read_message() -> HostFrame {
    use std::io::Read;
    let mut len_buf = [0u8; 4];
    match std::io::stdin().read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return HostFrame::Eof,
        Err(_) => return HostFrame::Eof, // pipe broken: nothing more to serve
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return HostFrame::Eof; // clean close
    }
    if len > 4 * 1024 * 1024 {
        // Length prefix is corrupt; the stream cannot be resynced, so the
        // only honest action is to exit rather than misparse everything.
        return HostFrame::Eof;
    }
    let mut buf = vec![0u8; len];
    if std::io::stdin().read_exact(&mut buf).is_err() {
        return HostFrame::Eof;
    }
    match serde_json::from_slice::<Incoming>(&buf) {
        Ok(m) => HostFrame::Msg(m),
        Err(e) => HostFrame::Bad(format!("bad message: {e}")),
    }
}

fn write_message(reply: &Reply) {
    if let Ok(data) = serde_json::to_vec(reply) {
        let len = (data.len() as u32).to_le_bytes();
        let mut out = std::io::stdout();
        use std::io::Write;
        let _ = out.write_all(&len);
        let _ = out.write_all(&data);
        let _ = out.flush();
    }
}

/// Keep only headers the engine may replay, with sane sizes. The extension
/// already filters; this is the trust boundary, so filter again.
fn replayable_headers(
    headers: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<std::collections::BTreeMap<String, String>> {
    let kept: std::collections::BTreeMap<String, String> = headers?
        .iter()
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .filter(|(k, v)| {
            crate::engine::replayable_header(k)
                && k.len() <= 64
                && !v.is_empty()
                && v.len() <= 4096
                && !v.contains(['\r', '\n'])
        })
        .take(20)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

fn queue_download(msg: &Incoming) -> Result<bool, String> {
    const MAX_INBOX: usize = 200;
    let url = msg.url.clone().unwrap_or_default();
    let path = inbox_path().ok_or_else(|| "no config dir".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _guard = crate::model::fs_lock("inbox");
    if !_guard.owned() { return Err("Download inbox is busy; retry shortly".into()); }
    // Never destroy pending captures on an unreadable inbox.
    let data = std::fs::read_to_string(&path).ok();
    let mut items: Vec<InboxItem> = match data {
        None => Vec::new(), // no file yet
        Some(s) => serde_json::from_str(&s)
            .map_err(|_| "inbox unreadable — kept for the next poll".to_string())?,
    };
    let referer = msg
        .referer
        .clone()
        .or_else(|| msg.page_url.clone())
        .filter(|s| !s.trim().is_empty());
    let fresh = InboxItem {
        url: url.clone(),
        filename: msg.filename.clone(),
        received_at: chrono::Local::now().to_rfc3339(),
        page_url: msg.page_url.clone(),
        referer,
        cookie: msg.cookie.clone().filter(|s| !s.trim().is_empty()),
        headers: replayable_headers(msg.headers.as_ref()),
        mime: msg.mime.clone().filter(|s| !s.trim().is_empty()),
        size: msg.size.filter(|s| *s > 0),
        pair_url: msg.pair_url.clone().filter(|s| s.starts_with("http")),
        pair_mime: msg.pair_mime.clone().filter(|s| !s.trim().is_empty()),
        urls: None,
        quality: msg.quality.clone().filter(|s| !s.trim().is_empty()),
        dur: msg.dur.filter(|s| *s > 0),
        audio_expected: msg.audio_expected,
    };
    // Same URL re-captured: refresh metadata instead of ignoring it (the
    // filename/referer/cookie may be corrected this time).
    let is_new = match items.iter_mut().find(|i| i.url == url) {
        Some(existing) => {
            *existing = fresh;
            false
        }
        None => {
            items.push(fresh);
            true
        }
    };
    if items.len() > MAX_INBOX {
        items.sort_by(|a, b| b.received_at.cmp(&a.received_at));
        items.truncate(MAX_INBOX);
    }
    let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
    crate::model::atomic_write(&path,json.as_bytes()).map_err(|e|e.to_string())?;
    Ok(is_new)
}

/// Fire-and-forget the standalone Download File Info dialog (detached, so
/// it outlives this short-lived native-host process).
fn spawn_confirm(msg: &Incoming) {
    let url = msg.url.clone().unwrap_or_default();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let Ok(exe) = crate::platform::launch_path() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--confirm-download").arg("--url").arg(&url);
    if let Some(f) = msg.filename.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--filename").arg(f);
    }
    if let Some(r) = msg.page_url.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--referer").arg(r);
    } else if let Some(r) = msg.referer.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--referer").arg(r);
    }
    // The dialog claims credentials from the owner-only inbox.
    if let Some(m) = msg.mime.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--mime").arg(m);
    }
    if let Some(s) = msg.size.filter(|s| *s > 0) {
        cmd.arg("--size").arg(s.to_string());
    }
    if let Some(p) = msg.pair_url.clone().filter(|s| s.starts_with("http")) {
        cmd.arg("--pair-url").arg(p);
        if let Some(m) = msg.pair_mime.clone().filter(|s| !s.trim().is_empty()) {
            cmd.arg("--pair-mime").arg(m);
        }
    }
    if let Some(q) = msg.quality.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--quality").arg(q);
    }
    if msg.audio_expected { cmd.arg("--expect-audio"); }
    if let Some(d) = msg.dur.filter(|s| *s > 0) {
        cmd.arg("--dur").arg(d.to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // DETACHED_PROCESS so the dialog outlives this short-lived host, and
        // BREAKAWAY_FROM_JOB because Firefox runs native hosts inside a job
        // object it kills when the host exits: children inherit that job, so a
        // plain detached dialog was being torn down the moment Firefox closed
        // our stdin (verified live — Chrome does not use such a job, which is
        // why only Firefox looked like it did nothing). A job that forbids
        // breaking away makes the spawn fail, and a plain detached dialog
        // still beats no dialog at all.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return;
        }
        cmd.creation_flags(DETACHED_PROCESS);
    }
    let _ = cmd.spawn();
}

/// Entry point when launched with `--native-host`. Never returns normally.
pub fn run() -> eframe::Result<()> {
    // Keep stderr quiet; any panic text would corrupt the pipe (stdout stays clean).
    loop {
        match read_message() {
            HostFrame::Eof => break,
            HostFrame::Bad(e) => {
                write_message(&Reply::err(e));
                continue;
            }
            HostFrame::Msg(msg) => match msg.kind.as_str() {
            "ping" => write_message(&Reply {
                kind: "pong".into(),
                version: Some(1),
                queued: None,
                error: None,
                urls: None,
            }),
            "query" => {
                // Presence check only: no queueing, no dialogs, no locks held
                // beyond the read.
                let urls = msg.urls.clone().unwrap_or_default();
                let hits = queued_urls(&urls);
                write_message(&Reply {
                    kind: "queued".into(),
                    version: None,
                    queued: None,
                    error: None,
                    urls: Some(hits),
                });
            }
            "download" => {
                let url = msg.url.clone().unwrap_or_default();
                let ok = url.starts_with("http://") || url.starts_with("https://");
                if !ok {
                    write_message(&Reply::err("need http/https url"));
                    continue;
                }
                // Blob/media-source URLs are not directly downloadable by the
                // engine — reject loudly so the extension can fall back to the
                // page URL instead of queueing a dead link.
                if url.starts_with("blob:") {
                    write_message(&Reply::err(
                        "blob: URLs need the page URL — send the page link instead",
                    ));
                    continue;
                }
                match queue_download(&msg) {
                    Ok(_is_new) => {
                        write_message(&Reply::ok());
                        // IDM-style: pop the standalone Download File Info
                        // window in front. The inbox entry doubles as a
                        // fallback — the dialog claims it on open, and the
                        // main window only prompts for entries the dialog
                        // never claimed (see poll_inbox age gate).
                        spawn_confirm(&msg);
                    }
                    Err(e) => write_message(&Reply::err(e)),
                }
            }
            _ => write_message(&Reply::err("unknown message")),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod media_tests {
    use super::*;
    #[test]
    fn player_expectations_survive_browser_and_inbox_formats() {
        let msg: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://cdn.example.org/video.mp4","dur":2700,"audioExpected":true}"#).unwrap();
        assert_eq!(msg.dur, Some(2700));
        assert!(msg.audio_expected);
        let stored: InboxItem = serde_json::from_str(r#"{"url":"https://cdn.example.org/video.mp4","filename":null,"received_at":"now","dur":2700,"audio_expected":true}"#).unwrap();
        assert_eq!(stored.dur, Some(2700));
        assert!(stored.audio_expected);
        let old: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://cdn.example.org/video.mp4"}"#).unwrap();
        assert!(!old.audio_expected);
        assert!(old.headers.is_none());
    }

    #[test]
    fn captured_headers_keep_only_replayable_tokens() {
        let msg: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://cdn.example.org/v",
            "headers":{"Origin":"https://site.example","authorization":"Bearer t","x-token":"1",
                       "x-client-data":"variations","Range":"bytes=0-","host":"evil","x-bad":"a\r\nb: c"}}"#).unwrap();
        let kept = replayable_headers(msg.headers.as_ref()).unwrap();
        assert_eq!(kept.keys().cloned().collect::<Vec<_>>(), ["authorization", "origin", "x-token"]);
        assert!(replayable_headers(None).is_none());
        let none = std::collections::BTreeMap::from([("cookie".to_string(), "a=b".to_string())]);
        assert!(replayable_headers(Some(&none)).is_none());
    }
}
