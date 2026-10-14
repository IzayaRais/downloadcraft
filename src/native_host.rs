//! Native-messaging host for the browser extension (`--native-host`).
//!
//! Protocol (Chrome/Edge/Firefox native messaging = 4-byte LE length + UTF-8 JSON):
//!   -> {"type":"ping"}                                  <- {"type":"pong","version":1}
//!   -> {"type":"download","url":"https://...","filename":"optional"}
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

fn queue_download(url: String, filename: Option<String>) -> Result<(), String> {
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
        items.push(InboxItem {
            url,
            filename,
            received_at: chrono::Local::now().to_rfc3339(),
        });
        let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(())
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
                let url = msg.url.unwrap_or_default();
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
                match queue_download(url, msg.filename) {
                    Ok(()) => write_message(&Reply {
                        kind: "ok".into(),
                        version: None,
                        queued: Some(true),
                        error: None,
                    }),
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
