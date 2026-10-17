//! IDM-style standalone "Download File Info" dialog.
//!
//! Launched as a separate OS window (always on top) when the browser
//! extension captures a download:
//!   `download_craft.exe --confirm-download --url <url> [--filename ..]
//!    [--referer ..] [--cookie ..] [--mime ..]`
//!
//! Unlike the in-canvas Add modal in the main window, this pops in front of
//! whatever the user is doing — one window per captured download, like IDM.
//! It writes straight to the shared `queue.json` (+ `wake.json` for
//! Start Download), so it works even when the main app is closed.

use crate::engine::{self, EngineOpts};
use crate::model::{
    atomic_write, category_of, config_dir, filename_from_url, format_bytes, is_download_url,
    DownloadItem,
};
use std::sync::mpsc::Receiver;

/// Seconds → "3:26" badge for browser-known media durations.
fn format_dur(secs: u64) -> String {
    if secs == 0 || secs >= 86400 {
        return String::new();
    }
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn wake_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("wake.json"))
}

fn ui_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("ui.json"))
}

/// Claim (remove) our URL from the native inbox so the main window does not
/// prompt for it too. Runs at dialog open; Cancel therefore discards.
fn claim_inbox(url: &str) {
    let Some(path) = crate::native_host::inbox_path() else {
        return;
    };
    let _guard = crate::model::fs_lock("inbox");
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return,
    };
    let items: Vec<crate::native_host::InboxItem> =
        serde_json::from_str(&data).unwrap_or_default();
    let rest: Vec<_> = items.into_iter().filter(|i| i.url != url).collect();
    if rest.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else if let Ok(json) = serde_json::to_string_pretty(&rest) {
        let _ = atomic_write(&path, json.as_bytes());
    }
}

#[derive(Debug, Default)]
struct UiPrefs {
    save_dir: String,
    connections: usize,
    category_folders: bool,
    user_agent: String,
    proxy_url: String,
    timeout_secs: u64,
    rename_template: String,
    folder_rules: Vec<crate::model::FolderRule>,
}

fn load_prefs() -> UiPrefs {
    let mut p = UiPrefs {
        connections: 8,
        category_folders: true,
        timeout_secs: 60,
        ..Default::default()
    };
    let v: serde_json::Value = ui_path()
        .and_then(|x| std::fs::read_to_string(x).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    if let Some(s) = v.get("save_dir").and_then(|x| x.as_str()) {
        if !s.trim().is_empty() {
            p.save_dir = s.to_string();
        }
    }
    if p.save_dir.is_empty() {
        p.save_dir = dirs::download_dir()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\Downloads".to_string());
    }
    if let Some(n) = v.get("connections_default").and_then(|x| x.as_u64()) {
        p.connections = (n as usize).clamp(1, 16);
    }
    if let Some(b) = v.get("category_folders").and_then(|x| x.as_bool()) {
        p.category_folders = b;
    }
    if let Some(s) = v.get("user_agent").and_then(|x| x.as_str()) {
        p.user_agent = s.to_string();
    }
    if let Some(s) = v.get("proxy_url").and_then(|x| x.as_str()) {
        p.proxy_url = s.to_string();
    }
    if let Some(n) = v.get("timeout_secs").and_then(|x| x.as_u64()) {
        p.timeout_secs = n.clamp(5, 600);
    }
    if let Some(s) = v.get("rename_template").and_then(|x| x.as_str()) {
        if !s.trim().is_empty() {
            p.rename_template = s.to_string();
        }
    }
    if p.rename_template.trim().is_empty() {
        p.rename_template = "{name}".to_string();
    }
    if let Some(arr) = v.get("folder_rules").and_then(|x| x.as_array()) {
        let rules: Vec<crate::model::FolderRule> = arr
            .iter()
            .filter_map(|r| {
                Some(crate::model::FolderRule {
                    exts: r.get("exts")?.as_str()?.to_string(),
                    folder: r.get("folder")?.as_str()?.to_string(),
                })
            })
            .take(8)
            .collect();
        if !rules.is_empty() {
            p.folder_rules = rules;
        }
    }
    if p.folder_rules.is_empty() {
        p.folder_rules = crate::model::default_folder_rules();
    }
    p
}

fn save_remember_category(value: bool) {
    let Some(path) = ui_path() else { return };
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "category_folders".to_string(),
            serde_json::Value::Bool(value),
        );
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            let _ = atomic_write(&path, json.as_bytes());
        }
    }
}

/// True when the main window is already running (fresh heartbeat).
/// Replaces process-name counting (two dialogs used to look "live").
fn main_app_running() -> bool {
    crate::model::is_main_live()
}

/// Launch the main window detached (fire-and-forget).
fn launch_main_app() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS so the GUI outlives this dialog.
        cmd.creation_flags(0x00000008);
    }
    let _ = cmd.spawn();
}

struct ConfirmApp {
    url: String,
    filename: String,
    category: String,
    remember_path: bool,
    save_base: String,
    description: String,
    referer: String,
    cookie: String,
    pair_url: String,
    pair_mime: String,
    /// Page/media MIME from the extension (first hint for naming when the
    /// URL carries no usable name and no probe has run yet).
    page_mime: String,
    /// Media duration in seconds from the extension (best-effort badge).
    duration_secs: u64,
    quality_pref: String,
    connections: usize,
    user_agent: String,
    proxy_url: String,
    timeout_secs: u64,
    rename_template: String,
    folder_rules: Vec<crate::model::FolderRule>,
    probing: bool,
    probe_size: Option<u64>,
    probe_rx: Option<Receiver<(Option<u64>, Option<String>)>>,
    rt: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    centered: bool,
    show_adv: bool,
    done: bool,
    exited: bool,
    notice: String,
    /// State for controls the concept does not show (dropdown + extras).
    cat_open: bool,
    win_h: f32,
    adding_cat: bool,
    new_cat: String,
    custom_cats: Vec<String>,
}

const CATEGORIES: &[&str] = &[
    "Video",
    "Music",
    "Image",
    "Document",
    "Compressed",
    "Apps",
    "Other",
];

// ---------- "Compact Blue" concept geometry ----------
// `popup_concepts.html` draws the card as `width:500px` with 16px side
// padding and a 1px border. CSS `box-sizing` is content-box, so the box that
// actually appears on screen is 534 x 228 px. Everything below is expressed
// in those exact pixels so the Rust dialog matches the mock 1:1.
const CARD_W: f32 = 534.0;
const CARD_H: f32 = 228.0;
const PAD_X: f32 = 16.0;
const PAD_Y: f32 = 12.0;
const CONTENT_W: f32 = CARD_W - 2.0 - 2.0 * PAD_X; // 500
const CARD_RAD: f32 = 10.0;
const RAD: f32 = 5.0; // inputs and buttons
const LBL_H: f32 = 11.0; // 9px label line box
const LBL_GAP: f32 = 2.0;
const ROW_GAP: f32 = 6.0;
const FIELD_H: f32 = 21.0;
const PATH_H: f32 = 19.0;
const DESC_H: f32 = 24.0;
const BTN_H: f32 = 24.0;

const C_CARD: egui::Color32 = egui::Color32::from_rgb(0x0f, 0x0f, 0x22);
const C_INPUT: egui::Color32 = egui::Color32::from_rgb(0x0a, 0x0a, 0x1a);
const C_BORDER: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x2a, 0x4a);
const C_ACCENT: egui::Color32 = egui::Color32::from_rgb(0x3b, 0x82, 0xf6);
const C_ACCENT_DK: egui::Color32 = egui::Color32::from_rgb(0x1d, 0x4e, 0xd8);
const C_ACCENT_HI: egui::Color32 = egui::Color32::from_rgb(0x4a, 0x8f, 0xf8);
const C_TXT: egui::Color32 = egui::Color32::from_rgb(0xe0, 0xe0, 0xf0);
const C_LBL: egui::Color32 = egui::Color32::from_rgb(0x88, 0x88, 0xa8);
const C_FTXT: egui::Color32 = egui::Color32::from_rgb(0xa0, 0xa0, 0xb8);
const C_DIM: egui::Color32 = egui::Color32::from_rgb(0x60, 0x60, 0x80);
const C_BTN_BD: egui::Color32 = egui::Color32::from_rgb(0x3a, 0x3a, 0x5a);
const C_HOVER: egui::Color32 = egui::Color32::from_rgb(0x1a, 0x1a, 0x2f);

// Material Icons glyphs for the concept's own chrome.
const GL_CARET: &str = "\u{e5c5}"; // arrow_drop_down, the "▾"
const GL_ADD: &str = "\u{e145}"; // add, the "+"
const GL_CLOSE: &str = "\u{e5cd}"; // close, the "✕"

const FONT_SEMI: &str = crate::fonts::FONT_SEMI;

fn reg(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Proportional)
}

fn semi(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_SEMI.into()))
}

/// Loads Segoe UI — the family the concept asks for — plus a semibold cut for
/// its 600-weight title and primary button. Missing system fonts fall back to
/// the bundled defaults, so this is safe on any machine.
fn setup_popup_fonts(cc: &eframe::CreationContext<'_>) {
    // `true`: this whole window is the concept, so Segoe UI also becomes its
    // default proportional face.
    crate::fonts::install(cc, true);
}

/// The 20px gradient mark: rounded accent square + white download arrow,
/// traced from the concept's inline SVG (24x24 viewBox scaled to 10px).
fn app_mark(p: &egui::Painter, rect: egui::Rect) {
    p.rect(rect, RAD, C_ACCENT, egui::Stroke::NONE, egui::StrokeKind::Inside);
    let s = 10.0 / 24.0;
    let o = rect.min.to_vec2() + egui::vec2(5.0, 5.0);
    let m = |x: f32, y: f32| egui::pos2(o.x + x * s, o.y + y * s);
    let stroke = egui::Stroke::new(3.0 * s, egui::Color32::WHITE);
    p.line_segment([m(12.0, 4.0), m(12.0, 16.0)], stroke);
    p.line_segment([m(6.0, 10.0), m(12.0, 16.0)], stroke);
    p.line_segment([m(12.0, 16.0), m(18.0, 10.0)], stroke);
}

/// Rounded input box with a frameless single-line editor inside it.
fn field(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    buf: &mut String,
    hint: &str,
) -> egui::Response {
    ui.painter().rect(
        rect,
        RAD,
        C_INPUT,
        egui::Stroke::new(1.0_f32, C_BORDER),
        egui::StrokeKind::Inside,
    );
    let inner = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.top()),
        egui::pos2(rect.right() - 8.0, rect.bottom()),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.add(
                egui::TextEdit::singleline(buf)
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .desired_width(inner.width())
                    .text_color(C_FTXT)
                    .font(reg(9.0))
                    .hint_text(egui::RichText::new(hint).size(9.0).color(C_DIM)),
            )
        },
    )
    .inner
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Ghost,
    Primary,
}

/// Concept button: equal-height pill, 10px label, accent fill for primary.
fn button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    kind: Btn,
    salt: u64,
) -> egui::Response {
    let r = ui.interact(rect, egui::Id::new(("popup-btn", salt)), egui::Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (fill, stroke, txt, font) = match kind {
        Btn::Primary => (
            if r.is_pointer_button_down_on() {
                C_ACCENT_DK
            } else if r.hovered() {
                C_ACCENT_HI
            } else {
                C_ACCENT
            },
            egui::Stroke::NONE,
            egui::Color32::WHITE,
            semi(10.0),
        ),
        Btn::Ghost => (
            if r.hovered() {
                C_HOVER
            } else {
                egui::Color32::TRANSPARENT
            },
            egui::Stroke::new(1.0_f32, C_BTN_BD),
            C_FTXT,
            reg(10.0),
        ),
    };
    let p = ui.painter();
    p.rect(rect, RAD, fill, stroke, egui::StrokeKind::Inside);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, txt);
    r
}

/// The concept's 20px squares: filled accent ("+") or outlined dark ("...").
fn square(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    glyph: &str,
    filled: bool,
    salt: u64,
) -> egui::Response {
    let r = ui.interact(rect, egui::Id::new(("popup-sq", salt)), egui::Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    if filled {
        p.rect(
            rect,
            RAD,
            if r.hovered() { C_ACCENT_HI } else { C_ACCENT },
            egui::Stroke::NONE,
            egui::StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            reg(12.0),
            egui::Color32::WHITE,
        );
    } else {
        p.rect(
            rect,
            RAD,
            if r.hovered() { C_HOVER } else { C_INPUT },
            egui::Stroke::new(1.0_f32, C_BORDER),
            egui::StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            reg(10.0),
            C_FTXT,
        );
    }
    r
}

/// Concept dropdown: dark rounded box, 9px value, trailing caret, and a
/// floating list anchored under the box.
fn combo(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &str,
    current: &str,
    options: &[String],
    open: &mut bool,
) -> Option<String> {
    let resp = ui.interact(rect, egui::Id::new(("popup-combo", id)), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    p.rect(
        rect,
        RAD,
        C_INPUT,
        egui::Stroke::new(1.0_f32, C_BORDER),
        egui::StrokeKind::Inside,
    );
    p.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        current,
        reg(9.0),
        C_FTXT,
    );
    p.text(
        egui::pos2(rect.right() - 4.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        GL_CARET,
        reg(12.0),
        C_DIM,
    );
    if resp.clicked() {
        *open = !*open;
    }
    let mut picked = None;
    egui::Popup::from_response(&resp)
        .open_bool(&mut *open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(2.0)
        .frame(
            egui::Frame::new()
                .fill(C_CARD)
                .stroke(egui::Stroke::new(1.0_f32, C_BORDER))
                .corner_radius(RAD)
                .inner_margin(egui::Margin::symmetric(3, 3)),
        )
        .show(|ui| {
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 1.0);
                    let w = rect.width() - 10.0;
                    for o in options {
                        let (r, ir) =
                            ui.allocate_exact_size(egui::vec2(w, 18.0), egui::Sense::click());
                        if ir.hovered() {
                            ui.painter().rect_filled(r, 3.0, C_HOVER);
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        ui.painter().text(
                            egui::pos2(r.left() + 6.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            o,
                            reg(9.0),
                            if o.as_str() == current { C_ACCENT } else { C_FTXT },
                        );
                        if ir.clicked() {
                            picked = Some(o.clone());
                        }
                    }
                });
        });
    if picked.is_some() {
        *open = false;
    }
    picked
}

impl ConfirmApp {
    fn effective_dir(&self) -> String {
        if self.remember_path {
            // Ordered user rules first, category fallback second (mirrors app).
            if let Some(rule) = self
                .folder_rules
                .iter()
                .find(|r| r.matches(&self.filename))
            {
                if !rule.folder.trim().is_empty() {
                    let mut p = std::path::PathBuf::from(&self.save_base);
                    p.push(rule.folder.trim());
                    return p.to_string_lossy().to_string();
                }
            }
            let mut p = std::path::PathBuf::from(&self.save_base);
            p.push(self.category.clone());
            p.to_string_lossy().to_string()
        } else {
            self.save_base.clone()
        }
    }

    /// Extra one-line facts the concept has no place for; shown under the
    /// hidden extras panel only.
    fn extra_notes(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.pair_url.starts_with("http") {
            v.push("video-only + audio — merged on completion (needs ffmpeg)".to_string());
        }
        if !self.quality_pref.trim().is_empty() {
            v.push(format!("Quality: {}", self.quality_pref.trim()));
        }
        v
    }

    fn start_probe(&mut self) {
        if !is_download_url(&self.url) {
            return;
        }
        let Some(rt) = self.rt.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.probe_rx = Some(rx);
        self.probing = true;
        self.probe_size = None;
        let url = self.url.clone();
        let referer = self.referer.clone();
        // Quality choice feeds the page resolve so the dialog measures the
        // chosen stream's size, not just the page's (IDM shows size upfront).
        let quality_pref = self.quality_pref.trim().to_string();
        // Bare URL guess to correct with the MIME type when the server
        // sends no disposition (P1: `download_123.bin` + video/mp4).
        let guess_name = self.filename.clone();
        let page_mime = self.page_mime.clone();
        let opts = EngineOpts {
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", env!("CARGO_PKG_VERSION"))
            } else {
                self.user_agent.clone()
            },
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            referer,
            quality_pref,
            ..Default::default()
        };
        rt.spawn(async move {
            let out = match opts.build_client() {
                Ok(client) => {
                    let (mut total, _, ctype, mut disp) =
                        engine::probe(&client, &opts, &url).await;
                    let is_page = ctype
                        .as_deref()
                        .map(|c| c.contains("text/html"))
                        .unwrap_or(false)
                        || url.contains("youtube.com")
                        || url.contains("youtu.be");
                    if total.is_none() && is_page {
                        let shared =
                            std::sync::Arc::new(engine::SharedState::new());
                        if let Some(hit) =
                            engine::resolve_media(&client, &opts, &url, &shared).await
                        {
                            let (t2, _, _, _) =
                                engine::probe(&client, &opts, &hit.url).await;
                            if total.is_none() {
                                total = t2;
                            }
                            if let Some(pair) = hit.pair_url.filter(|p| p.starts_with("http")) {
                                let (ta, _, _, _) =
                                    engine::probe(&client, &opts, &pair).await;
                                if let (Some(a), Some(b)) = (total, ta) {
                                    total = Some(a.saturating_add(b));
                                } else {
                                    total = total.or(ta);
                                }
                            }
                            if disp.is_none() && !hit.title.trim().is_empty() {
                                disp = Some(engine::sanitize_filename(&hit.title));
                            }
                        }
                    }
                    if disp.is_none()
                        && crate::model::is_fallback_filename(&guess_name)
                    {
                        // No disposition and no media title: at least fix the
                        // extension — probed MIME first, extension hint when
                        // the probe itself was refused off-session.
                        let ext = ctype
                            .as_deref()
                            .and_then(crate::model::ext_for_mime)
                            .or_else(|| crate::model::ext_for_mime(&page_mime));
                        if let Some(ext) = ext {
                            disp = Some(guess_name.trim_end_matches(".bin").to_string() + ext);
                        }
                    }
                    (total, disp)
                }
                Err(_) => (None, None),
            };
            let _ = tx.send(out);
        });
    }

    /// Queue the download. `start` = also wake the main app to begin it now
    /// (IDM "Start Download") vs merely queueing ("Download Later").
    fn commit(&mut self, start: bool) {
        if self.filename.trim().is_empty() {
            self.filename = filename_from_url(&self.url);
        }
        let mut item = DownloadItem::new(
            0, // id assigned atomically under the queue lock below
            self.url.trim().to_string(),
            engine::sanitize_filename(self.filename.trim()),
            self.effective_dir(),
            self.connections,
        );
        item.referer = self.referer.clone();
        item.cookie = self.cookie.clone();
        item.description = self.description.trim().to_string();
        // Browser-measured size (extension HEAD/Range in the real session):
        // keep it so the engine and progress UI know the total even when a
        // server-side re-probe would 403 off-session.
        item.total_bytes = self.probe_size.filter(|s| *s > 0);
        if self.pair_url.starts_with("http") {
            item.pair_url = self.pair_url.clone();
            item.pair_mime = self.pair_mime.clone();
        }
        item.quality_pref = self.quality_pref.trim().to_string();
        item.duration_secs = self.duration_secs;
        let id = match crate::model::append_to_queue(&mut item) {
            crate::model::AppendResult::Added(id) => {
                save_remember_category(self.remember_path);
                id
            }
            crate::model::AppendResult::AlreadyQueued(id) => {
                // Not an error. The URL is already in the list; if the user
                // asked to start it we wake that row, so this is never a dead
                // end. Stay open so the notice is actually read.
                self.notice = if start {
                    format!("Already in the queue (#{id}) — starting it.")
                } else {
                    format!("Already in the queue (#{id}).")
                };
                if start {
                    self.wake_targets(&[id]);
                }
                return;
            }
            crate::model::AppendResult::Error(e) => {
                // Never close on a failed write: closing with a stale message
                // is exactly what made this look like "already in queue".
                self.notice = format!("Could not update the queue: {e}");
                return;
            }
        };
        save_remember_category(self.remember_path);
        if start {
            self.wake_targets(&[id]);
        }
        self.done = true;
    }

    /// Ask the main app to start these ids (consumed on its next poll, or at
    /// startup when we launch it below).
    fn wake_targets(&self, targets: &[u64]) {
        if let Some(p) = wake_path() {
            let _guard = crate::model::fs_lock("wake");
            let mut ids: Vec<u64> = std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("start").cloned())
                .and_then(|v| serde_json::from_value::<Vec<u64>>(v).ok())
                .unwrap_or_default();
            for id in targets {
                if !ids.contains(id) {
                    ids.push(*id);
                }
            }
            if let Ok(json) =
                serde_json::to_string_pretty(&serde_json::json!({ "start": ids }))
            {
                let _ = atomic_write(&p, json.as_bytes());
            }
        }
        // tasklist + cold-start spawn can take seconds (debug builds
        // especially) — never block the dialog on it; close at once.
        std::thread::spawn(|| {
            if !main_app_running() {
                launch_main_app();
            }
        });
    }
}

/// Entry point for `--confirm-download`. Never returns normally
/// (runs the dialog window until it closes).
pub fn run() -> eframe::Result<()> {
    // Index-iterated flag parsing: a VALUE that looks like a flag (e.g. a
    // `Content-Disposition` filename of `--referer`) must not shift parsing.
    // Only the token immediately following a known flag is consumed.
    const KNOWN: &[&str] = &[
        "--url",
        "--filename",
        "--referer",
        "--cookie",
        "--mime",
        "--size",
        "--pair-url",
        "--pair-mime",
        "--quality",
        "--dur",
    ];
    let args: Vec<String> = std::env::args().collect();
    let mut map: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(flag) = KNOWN.iter().find(|k| **k == args[i].as_str()) {
            if i + 1 < args.len() {
                map.insert(*flag, args[i + 1].clone());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    let mut get = |name: &str| -> Option<String> { map.remove(name) };
    let url = get("--url").unwrap_or_default();
    if !is_download_url(&url) {
        eprintln!("--confirm-download needs --url <http(s)://...>");
        std::process::exit(2);
    }
    let page_mime = get("--mime").filter(|s| !s.trim().is_empty()).unwrap_or_default();
    let filename = get("--filename")
        .filter(|f| !f.trim().is_empty())
        .unwrap_or_else(|| {
            crate::model::filename_from_url_with_mime(&url, Some(page_mime.as_str()))
        });
    let referer = get("--referer").unwrap_or_default();
    let cookie = get("--cookie").unwrap_or_default();
    let pair_url = get("--pair-url").filter(|s| s.starts_with("http")).unwrap_or_default();
    let pair_mime = get("--pair-mime").unwrap_or_default();
    let quality_pref = get("--quality").filter(|s| !s.trim().is_empty()).unwrap_or_default();
    // Browser-measured size (menu HEAD/Range): trust it, skip probing.
    let known_size: Option<u64> = get("--size")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0);
    // Browser-known media duration in seconds (best-effort badge).
    let duration_secs: u64 = get("--dur")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0 && *s < 86400)
        .unwrap_or(0);

    // Claim the inbox entry first so the main window won't prompt too.
    claim_inbox(url.trim());

    let prefs = load_prefs();
    let category = category_of(&filename).to_string();
    // A row that is still usable (queued/downloading/paused, or finished with
    // its file present) is reported up front. A dead row (failed, or finished
    // with the file gone) is refreshed by the capture instead, so the buttons
    // never dead-end on a stale "already in queue".
    let initial_notice = match crate::model::queued_entry(url.trim()) {
        Some((id, false)) => format!("Already in the queue (#{id})."),
        _ => String::new(),
    };

    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    let mut app = ConfirmApp {
        url: url.trim().to_string(),
        filename: crate::model::apply_rename_template(&prefs.rename_template, &engine::sanitize_filename(&filename), url.trim()),
        category,
        remember_path: prefs.category_folders,
        save_base: prefs.save_dir,
        description: String::new(),
        referer,
        cookie,
        pair_url,
        pair_mime,
        page_mime,
        duration_secs,
        quality_pref,
        connections: prefs.connections,
        user_agent: prefs.user_agent,
        proxy_url: prefs.proxy_url,
        timeout_secs: prefs.timeout_secs,
        rename_template: prefs.rename_template,
        folder_rules: prefs.folder_rules,
        probing: false,
        probe_size: known_size,
        probe_rx: None,
        rt: Some(rt.clone()),
        centered: false,
        show_adv: false,
        done: false,
        exited: false,
        notice: initial_notice,
        cat_open: false,
        win_h: 0.0,
        adding_cat: false,
        new_cat: String::new(),
        custom_cats: Vec::new(),
    };
    // Prefill filename from server when the extension sent none useful.
    // A browser-known size skips probing entirely (server re-probes of
    // media URLs often 403 off-session).
    if known_size.is_none() {
        app.start_probe();
    }
    let _rt_keepalive = rt;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Add download")
        .with_inner_size([CARD_W, CARD_H])
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top();
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Add download",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| {
            setup_popup_fonts(cc);
            cc.egui_ctx.set_visuals(crate::app::dark_visuals());
            crate::app::apply_widget_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for ConfirmApp {
    /// The window is transparent and frameless: the concept's rounded card is
    /// painted by hand, so the surface behind it must stay clear.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Same bulletproofing as the progress window: Close can no-op, so back
    /// it with a delayed exit once the dialog is done.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            if !self.exited {
                self.exited = true;
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    std::process::exit(0);
                });
            }
            return;
        }
        // Center once on the primary monitor (Windows otherwise cascades).
        if !self.centered {
            self.centered = true;
            let mon = ctx.input(|i| i.viewport().monitor_size);
            if let Some(m) = mon {
                let (w, h) = (CARD_W, CARD_H);
                if m.x > w && m.y > h {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                        egui::Pos2::new((m.x - w) / 2.0, (m.y - h) / 2.0),
                    ));
                }
            }
        }
        // Collect probe result.
        if let Some(rx) = self.probe_rx.as_ref() {
            while let Ok((sz, nm)) = rx.try_recv() {
                self.probing = false;
                self.probe_size = sz;
                if let Some(n) = nm {
                    // Prefer the server/video title when our name is a bare
                    // URL guess (or a last-resort fallback whose timestamp
                    // drifted since dialog open).
                    if self.filename == filename_from_url(&self.url)
                        || crate::model::is_fallback_filename(&self.filename)
                    {
                        self.filename = crate::model::apply_rename_template(
                            &self.rename_template,
                            &n,
                            &self.url,
                        );
                        self.category = category_of(&self.filename).to_string();
                    }
                }
            }
        }
        if self.probing {
            ctx.request_repaint();
        }
        // Grow the window when the hidden extras are open, so the concept
        // surface itself never changes size or position.
        let notes = self.extra_notes();
        let mut extra_h = if self.show_adv {
            10.0 + 3.0 * (FIELD_H + ROW_GAP)
        } else {
            0.0
        };
        if self.adding_cat {
            extra_h += FIELD_H + ROW_GAP;
        }
        if self.show_adv && !notes.is_empty() {
            extra_h += 4.0 + 12.0 * notes.len() as f32;
        }
        if !self.notice.is_empty() {
            extra_h += 16.0;
        }
        let want_h = CARD_H + extra_h;
        if (self.win_h - want_h).abs() > 0.5 {
            self.win_h = want_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(CARD_W, want_h)));
        }

        let panel = egui::CentralPanel::default().frame(egui::Frame::NONE);
        panel.show(ctx, |ui| {
            // The concept card IS the window: 534x228, 1px #2a2a4a border,
            // 10px radius, painted edge to edge.
            let card = ui.max_rect();
            let p = ui.painter().clone();
            p.rect(
                card,
                CARD_RAD,
                C_CARD,
                egui::Stroke::new(1.0_f32, C_BORDER),
                egui::StrokeKind::Inside,
            );
            // Content box: card minus the 1px border minus the 16/12 padding.
            let left = card.left() + 1.0 + PAD_X;
            let right = card.right() - 1.0 - PAD_X;
            let mut y = card.top() + 1.0 + PAD_Y;

            // --- header: mark + title + gear/close; drag anywhere to move ---
            let hdr = egui::Rect::from_min_size(
                egui::pos2(left, y),
                egui::vec2(CONTENT_W - 46.0, 20.0),
            );
            if ui
                .interact(
                    hdr,
                    egui::Id::new("popup-header"),
                    egui::Sense::click_and_drag(),
                )
                .drag_started()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            app_mark(
                &p,
                egui::Rect::from_min_size(egui::pos2(left, y), egui::vec2(20.0, 20.0)),
            );
            p.text(
                egui::pos2(left + 26.0, y + 10.0),
                egui::Align2::LEFT_CENTER,
                "Download File Info",
                semi(12.0),
                C_TXT,
            );
            let close_r = egui::Rect::from_center_size(
                egui::pos2(right - 5.5, y + 10.0),
                egui::vec2(12.0, 12.0),
            );
            let gear_r = egui::Rect::from_center_size(
                egui::pos2(right - 27.5, y + 10.0),
                egui::vec2(12.0, 12.0),
            );
            let gear = ui.interact(gear_r, egui::Id::new("popup-gear"), egui::Sense::click());
            let close = ui.interact(close_r, egui::Id::new("popup-close"), egui::Sense::click());
            p.text(
                gear_r.center(),
                egui::Align2::CENTER_CENTER,
                crate::app::MI_SETTINGS,
                reg(11.0),
                if gear.hovered() { C_FTXT } else { C_DIM },
            );
            p.text(
                close_r.center(),
                egui::Align2::CENTER_CENTER,
                GL_CLOSE,
                reg(11.0),
                if close.hovered() { C_FTXT } else { C_DIM },
            );
            if gear.hovered() || close.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if gear.clicked() {
                self.show_adv = !self.show_adv;
            }
            if close.clicked() {
                self.done = true;
            }
            y += 20.0 + 8.0;

            // --- URL ---
            p.text(
                egui::pos2(left, y + LBL_H / 2.0),
                egui::Align2::LEFT_CENTER,
                "URL",
                reg(9.0),
                C_LBL,
            );
            let url_rect = egui::Rect::from_min_size(
                egui::pos2(left, y + LBL_H + LBL_GAP),
                egui::vec2(CONTENT_W, FIELD_H),
            );
            field(ui, url_rect, &mut self.url, "https://...");
            y += LBL_H + LBL_GAP + FIELD_H + ROW_GAP;
            // --- Category (flex 1) + Save As (flex 2) ---
            let unit = (CONTENT_W - ROW_GAP) / 3.0;
            let cat_w = unit;
            let save_x = left + cat_w + ROW_GAP;
            let save_w = unit * 2.0;
            p.text(
                egui::pos2(left, y + LBL_H / 2.0),
                egui::Align2::LEFT_CENTER,
                "Category",
                reg(9.0),
                C_LBL,
            );
            p.text(
                egui::pos2(save_x, y + LBL_H / 2.0),
                egui::Align2::LEFT_CENTER,
                "Save As",
                reg(9.0),
                C_LBL,
            );
            let fy = y + LBL_H + LBL_GAP;
            let dd_w = cat_w - 4.0 - 20.0;
            let cat_rect =
                egui::Rect::from_min_size(egui::pos2(left, fy), egui::vec2(dd_w, FIELD_H));
            let plus_rect = egui::Rect::from_min_size(
                egui::pos2(left + dd_w + 4.0, fy),
                egui::vec2(20.0, 20.0),
            );
            let sd_w = save_w - 4.0 - 20.0;
            let save_rect =
                egui::Rect::from_min_size(egui::pos2(save_x, fy), egui::vec2(sd_w, FIELD_H));
            let dots_rect = egui::Rect::from_min_size(
                egui::pos2(save_x + sd_w + 4.0, fy),
                egui::vec2(20.0, 20.0),
            );
            let mut cats: Vec<String> = CATEGORIES.iter().map(|c| c.to_string()).collect();
            cats.extend(self.custom_cats.iter().cloned());
            if let Some(pick) =
                combo(ui, cat_rect, "cat", &self.category, &cats, &mut self.cat_open)
            {
                self.category = pick;
            }
            if square(ui, plus_rect, GL_ADD, true, 1).clicked() {
                self.adding_cat = !self.adding_cat;
            }
            field(ui, save_rect, &mut self.filename, "filename");
            if square(ui, dots_rect, "...", false, 2).clicked() {
                let mut dlg = rfd::FileDialog::new();
                if !self.save_base.trim().is_empty() {
                    dlg = dlg.set_directory(&self.save_base);
                }
                if let Some(dir) = dlg.pick_folder() {
                    self.save_base = dir.to_string_lossy().to_string();
                }
            }
            y += LBL_H + LBL_GAP + FIELD_H + ROW_GAP;

            if self.adding_cat {
                let w = CONTENT_W - 4.0 - 44.0;
                let f = egui::Rect::from_min_size(egui::pos2(left, y), egui::vec2(w, FIELD_H));
                field(ui, f, &mut self.new_cat, "New category name");
                let b = egui::Rect::from_min_size(
                    egui::pos2(left + w + 4.0, y),
                    egui::vec2(44.0, FIELD_H),
                );
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if button(ui, b, "Add", Btn::Primary, 3).clicked() || enter {
                    let name = self.new_cat.trim().to_string();
                    if !name.is_empty() {
                        if !self.custom_cats.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
                            self.custom_cats.push(name.clone());
                        }
                        self.category = name;
                        self.new_cat.clear();
                        self.adding_cat = false;
                    }
                }
                y += FIELD_H + ROW_GAP;
            }
            // --- remember + size + folder (one compact row) ---
            let cy = y + PATH_H / 2.0;
            let cb = ui.interact(
                egui::Rect::from_center_size(egui::pos2(left + 5.0, cy), egui::vec2(18.0, 18.0)),
                egui::Id::new("popup-remember"),
                egui::Sense::click(),
            );
            if cb.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let box_r =
                egui::Rect::from_center_size(egui::pos2(left + 5.0, cy), egui::vec2(10.0, 10.0));
            if self.remember_path {
                p.rect(box_r, 2.0, C_ACCENT, egui::Stroke::NONE, egui::StrokeKind::Inside);
                let stroke = egui::Stroke::new(1.3_f32, egui::Color32::WHITE);
                let m = |x: f32, yy: f32| egui::pos2(box_r.left() + x, box_r.top() + yy);
                p.line_segment([m(2.4, 5.2), m(4.3, 7.2)], stroke);
                p.line_segment([m(4.3, 7.2), m(7.7, 3.0)], stroke);
            } else {
                p.rect(
                    box_r,
                    2.0,
                    C_INPUT,
                    egui::Stroke::new(1.0_f32, C_BORDER),
                    egui::StrokeKind::Inside,
                );
            }
            if cb.clicked() {
                self.remember_path = !self.remember_path;
            }
            let lr = p.text(
                egui::pos2(left + 14.0, cy),
                egui::Align2::LEFT_CENTER,
                "Remember this path",
                reg(8.0),
                C_LBL,
            );
            let mut sx = lr.right() + 8.0;
            let known = self.probe_size.filter(|s| *s > 0);
            let size_txt = if self.probing {
                "…".to_string()
            } else {
                match known {
                    Some(sz) if self.duration_secs > 0 => {
                        format!("{} • {}", format_dur(self.duration_secs), format_bytes(sz))
                    }
                    Some(sz) => format_bytes(sz),
                    None => "size unknown".to_string(),
                }
            };
            let sr = p.text(
                egui::pos2(sx, cy),
                egui::Align2::LEFT_CENTER,
                &size_txt,
                reg(8.0),
                if known.is_some() { C_ACCENT } else { C_DIM },
            );
            sx = sr.right() + 8.0;
            let br = p.text(
                egui::pos2(sx, cy),
                egui::Align2::LEFT_CENTER,
                "|",
                reg(8.0),
                C_DIM,
            );
            sx = br.right() + 8.0;
            let path_rect = egui::Rect::from_min_size(
                egui::pos2(sx, y),
                egui::vec2((right - sx).max(60.0), PATH_H),
            );
            field(ui, path_rect, &mut self.save_base, "folder")
                .on_hover_text(format!("→ {}", self.effective_dir()));
            y += PATH_H + ROW_GAP;

            // --- description ---
            p.text(
                egui::pos2(left, y + LBL_H / 2.0),
                egui::Align2::LEFT_CENTER,
                "Description",
                reg(9.0),
                C_LBL,
            );
            let desc_rect = egui::Rect::from_min_size(
                egui::pos2(left, y + LBL_H + LBL_GAP),
                egui::vec2(CONTENT_W, DESC_H),
            );
            field(ui, desc_rect, &mut self.description, "optional note");
            y += LBL_H + LBL_GAP + DESC_H + 8.0;
            // --- actions: Download Later | Start Download | Cancel ---
            let bw = (CONTENT_W - 2.0 * ROW_GAP) / 3.0;
            let later = button(
                ui,
                egui::Rect::from_min_size(egui::pos2(left, y), egui::vec2(bw, BTN_H)),
                "Download Later",
                Btn::Ghost,
                4,
            );
            let start = button(
                ui,
                egui::Rect::from_min_size(
                    egui::pos2(left + bw + ROW_GAP, y),
                    egui::vec2(bw, BTN_H),
                ),
                "Start Download",
                Btn::Primary,
                5,
            );
            let cancel = button(
                ui,
                egui::Rect::from_min_size(
                    egui::pos2(left + 2.0 * (bw + ROW_GAP), y),
                    egui::vec2(bw, BTN_H),
                ),
                "Cancel",
                Btn::Ghost,
                6,
            );
            if later.clicked() {
                self.commit(false);
            }
            if start.clicked() {
                self.commit(true);
            }
            if cancel.clicked() {
                self.done = true;
            }
            y += BTN_H;

            // --- extras: the concept has no such rows, so they live in a
            // panel that is closed by default and grows the window. ---
            if self.show_adv {
                y += 10.0;
                let rows: [(&str, &mut String, &str); 2] = [
                    ("Referer", &mut self.referer, "http://..."),
                    ("Cookie", &mut self.cookie, "name=value; ..."),
                ];
                for (lbl, buf, hint) in rows {
                    p.text(
                        egui::pos2(left, y + FIELD_H / 2.0),
                        egui::Align2::LEFT_CENTER,
                        lbl,
                        reg(9.0),
                        C_LBL,
                    );
                    let f = egui::Rect::from_min_size(
                        egui::pos2(left + 62.0, y),
                        egui::vec2(CONTENT_W - 62.0, FIELD_H),
                    );
                    field(ui, f, buf, hint);
                    y += FIELD_H + ROW_GAP;
                }
                let ccy = y + FIELD_H / 2.0;
                p.text(
                    egui::pos2(left, ccy),
                    egui::Align2::LEFT_CENTER,
                    "Connections",
                    reg(9.0),
                    C_LBL,
                );
                p.text(
                    egui::pos2(left + 62.0, ccy),
                    egui::Align2::LEFT_CENTER,
                    self.connections.to_string(),
                    reg(9.0),
                    C_FTXT,
                );
                ui.put(
                    egui::Rect::from_min_size(
                        egui::pos2(left + 96.0, ccy - 8.0),
                        egui::vec2(CONTENT_W - 96.0, 16.0),
                    ),
                    egui::Slider::new(&mut self.connections, 1..=16).show_value(false),
                );
                y += FIELD_H + ROW_GAP;
                for n in &notes {
                    p.text(
                        egui::pos2(left, y),
                        egui::Align2::LEFT_CENTER,
                        n,
                        reg(8.0),
                        C_DIM,
                    );
                    y += 12.0;
                }
            }

            if !self.notice.is_empty() {
                // Clamp: the notice carries I/O messages that can be long, and
                // `p.text` does not wrap or clip inside the fixed card.
                let shown: String = self.notice.chars().take(70).collect();
                p.text(
                    egui::pos2(left, y + 8.0),
                    egui::Align2::LEFT_CENTER,
                    &shown,
                    reg(9.0),
                    egui::Color32::from_rgb(255, 150, 120),
                );
            }
            let _ = y;
        });
    }
}
