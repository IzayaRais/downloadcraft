//! Native-messaging host for the browser extension (`--native-host`).
//!
//! Protocol (Chrome/Edge/Firefox native messaging = 4-byte LE length + UTF-8 JSON):
//!   -> {"type":"ping"}                                  <- {"type":"pong","version":1}
//!   -> {"type":"download","url":"https://...","filename":"optional",
//!       "pageUrl":"https://page...","referer":"...","cookie":"...","mime":"..."}
//!                                                      <- {"type":"ok","queued":true}
//!
//! Download requests are appended to an inbox file that the GUI polls and
//! imports as queued (auto-started) downloads.

use serde::{Deserialize, Serialize};
use std::io::Read;

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
    #[serde(default)]
    mime: Option<String>,
    /// Browser-measured size bytes (HEAD/Range from the page session).
    /// The engine can't always re-probe media URLs (403 off-session), so
    /// the dialog prefers this over probing.
    #[serde(default)]
    size: Option<u64>,
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
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

pub fn inbox_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|p| p.join("native_inbox.json"))
}

fn read_message() -> Option<Incoming> {
    let mut len_buf = [0u8; 4];
    if std::io::stdin().read_exact(&mut len_buf).is_err() {
        return None; // browser closed the pipe
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 4 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; len];
    if std::io::stdin().read_exact(&mut buf).is_err() {
        return None;
    }
    serde_json::from_slice(&buf).ok()
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

fn queue_download(msg: &Incoming) -> Result<(), String> {
    let url = msg.url.clone().unwrap_or_default();
    let path = inbox_path().ok_or_else(|| "no config dir".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut items: Vec<InboxItem> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // De-dupe identical URLs.
    if !items.iter().any(|i| i.url == url) {
        let referer = msg
            .referer
            .clone()
            .or_else(|| msg.page_url.clone())
            .filter(|s| !s.trim().is_empty());
        items.push(InboxItem {
            url,
            filename: msg.filename.clone(),
            received_at: chrono::Local::now().to_rfc3339(),
            page_url: msg.page_url.clone(),
            referer,
            cookie: msg.cookie.clone().filter(|s| !s.trim().is_empty()),
            mime: msg.mime.clone().filter(|s| !s.trim().is_empty()),
            size: msg.size.filter(|s| *s > 0),
        });
        let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Fire-and-forget the standalone Download File Info dialog (detached, so
/// it outlives this short-lived native-host process).
fn spawn_confirm(msg: &Incoming) {
    let url = msg.url.clone().unwrap_or_default();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
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
    if let Some(c) = msg.cookie.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--cookie").arg(c);
    }
    if let Some(s) = msg.size.filter(|s| *s > 0) {
        cmd.arg("--size").arg(s.to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS: the dialog must survive our own exit.
        cmd.creation_flags(0x00000008);
    }
    let _ = cmd.spawn();
}

/// Entry point when launched with `--native-host`. Never returns normally.
pub fn run() -> eframe::Result<()> {
    // Keep stderr quiet; any panic text would corrupt the pipe (stdout stays clean).
    while let Some(msg) = read_message() {
        match msg.kind.as_str() {
            "ping" => write_message(&Reply {
                kind: "pong".into(),
                version: Some(1),
                queued: None,
                error: None,
            }),
            "download" => {
                let url = msg.url.clone().unwrap_or_default();
                let ok = url.starts_with("http://") || url.starts_with("https://");
                if !ok {
                    write_message(&Reply {
                        kind: "error".into(),
                        version: None,
                        queued: None,
                        error: Some("need http/https url".into()),
                    });
                    continue;
                }
                // Blob/media-source URLs are not directly downloadable by the
                // engine — reject loudly so the extension can fall back to the
                // page URL instead of queueing a dead link.
                if url.starts_with("blob:") {
                    write_message(&Reply {
                        kind: "error".into(),
                        version: None,
                        queued: None,
                        error: Some("blob: URLs need the page URL — send the page link instead".into()),
                    });
                    continue;
                }
                match queue_download(&msg) {
                    Ok(()) => {
                        write_message(&Reply {
                            kind: "ok".into(),
                            version: None,
                            queued: Some(true),
                            error: None,
                        });
                        // IDM-style: pop the standalone Download File Info
                        // window in front. The inbox entry doubles as a
                        // fallback — the dialog claims it on open, and the
                        // main window only prompts for entries the dialog
                        // never claimed (see poll_inbox age gate).
                        spawn_confirm(&msg);
                    }
                    Err(e) => write_message(&Reply {
                        kind: "error".into(),
                        version: None,
                        queued: None,
                        error: Some(e),
                    }),
                }
            }
            _ => write_message(&Reply {
                kind: "error".into(),
                version: None,
                queued: None,
                error: Some("unknown message".into()),
            }),
        }
    }
    Ok(())
}
