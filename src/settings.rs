//! Standalone Settings window (`--settings`): a separate OS window.
//!
//! The layout follows `settings_window.html`: a 640-wide window with an icon
//! rail (160 px), a scrolling content pane, and a Save/Close footer. Colours
//! come from the active app palette (`app::pl`), so the window follows
//! System/Dark/Light exactly like the rest of the app; geometry, type sizes
//! and icons follow the concept.
//!
//! It edits the shared `ui.json` with read-modify-write merging (only its
//! own keys are overlaid, so concurrent main-app saves elsewhere survive).
//! The main app watches `ui.json` mtime and reloads — no restart needed.

use crate::app::pl;
use crate::fonts::{reg, semi};
use crate::model::{default_folder_rules, FolderRule, SpeedWindow};
use egui::{Align2, Color32, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Rail entries: Material Icons glyph + label. The concept's YouTube pane is
/// the helper ("plugin") page, so it is labelled Plugins.
const PANES: &[(&str, &str)] = &[
    ("\u{e3dc}", "Appearance"),         // filter_center_focus
    ("\u{ea0b}", "Download Engine"),    // bolt
    ("\u{e9b0}", "Plugins"),            // grid_view
    ("\u{e80b}", "Browser Integration"), // public
    ("\u{e871}", "Categories & Rules"), // dashboard
    ("\u{e863}", "Updates"),            // autorenew
];

/// Rail index of the Plugins pane (the helper's status is probed when it is
/// first shown, not at startup: asking a frozen build for its version costs
/// about a second).
const PANE_YTDLP: usize = 2;

#[derive(Debug, Clone)]
struct Settings {
    // Appearance
    theme: String,
    close_to_tray: bool,
    /// Pop the standalone "Download complete" dialog when a transfer ends.
    show_complete: bool,
    // Engine
    save_dir: String,
    category_folders: bool,
    connections_default: usize,
    speed_limit_kbps: u64,
    user_agent: String,
    proxy_url: String,
    timeout_secs: u64,
    max_retries: u32,
    antivirus_cmd: String,
    finish_action: String,
    speed_windows_on: bool,
    speed_windows: Vec<SpeedWindow>,
    // Rules
    rename_template: String,
    folder_rules: Vec<FolderRule>,
    /// YouTube helper: use a yt-dlp found on PATH instead of the copy this app
    /// fetches and refreshes itself.
    ytdlp_prefer_path: bool,
    // Updates
    update_check_on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            close_to_tray: true,
            show_complete: true,
            save_dir: dirs::download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "C:\\Downloads".to_string()),
            category_folders: true,
            connections_default: 8,
            speed_limit_kbps: 0,
            user_agent: String::new(),
            proxy_url: String::new(),
            timeout_secs: 60,
            max_retries: 5,
            antivirus_cmd: String::new(),
            finish_action: "nothing".to_string(),
            speed_windows_on: false,
            speed_windows: vec![SpeedWindow {
                start: "02:00".to_string(),
                end: "06:00".to_string(),
                limit_kbps: 0,
            }],
            rename_template: "{name}".to_string(),
            folder_rules: default_folder_rules(),
            ytdlp_prefer_path: true,
            update_check_on: false,
        }
    }
}

fn ui_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|p| p.join("ui.json"))
}

fn load() -> Settings {
    let mut s = Settings::default();
    let v: serde_json::Value = ui_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let g = |k: &str| v.get(k);
    let str_in = |k: &str, dst: &mut String| {
        if let Some(x) = g(k).and_then(|x| x.as_str()) {
            if !x.trim().is_empty() {
                *dst = x.to_string();
            }
        }
    };
    str_in("theme", &mut s.theme);
    str_in("save_dir", &mut s.save_dir);
    str_in("user_agent", &mut s.user_agent);
    str_in("proxy_url", &mut s.proxy_url);
    str_in("antivirus_cmd", &mut s.antivirus_cmd);
    str_in("finish_action", &mut s.finish_action);
    str_in("rename_template", &mut s.rename_template);
    if let Some(b) = g("close_to_tray").and_then(|x| x.as_bool()) {
        s.close_to_tray = b;
    }
    if let Some(b) = g("show_complete").and_then(|x| x.as_bool()) {
        s.show_complete = b;
    }
    if let Some(b) = g("category_folders").and_then(|x| x.as_bool()) {
        s.category_folders = b;
    }
    if let Some(b) = g("speed_windows_on").and_then(|x| x.as_bool()) {
        s.speed_windows_on = b;
    }
    if let Some(b) = g("update_check_on").and_then(|x| x.as_bool()) {
        s.update_check_on = b;
    }
    if let Some(b) = g("ytdlp_prefer_path").and_then(|x| x.as_bool()) {
        s.ytdlp_prefer_path = b;
    }
    if let Some(n) = g("connections_default").and_then(|x| x.as_u64()) {
        s.connections_default = (n as usize).clamp(1, 16);
    }
    if let Some(n) = g("speed_limit_kbps").and_then(|x| x.as_u64()) {
        s.speed_limit_kbps = n;
    }
    if let Some(n) = g("timeout_secs").and_then(|x| x.as_u64()) {
        s.timeout_secs = n.clamp(5, 600);
    }
    if let Some(n) = g("max_retries").and_then(|x| x.as_u64()) {
        s.max_retries = (n as u32).clamp(1, 20);
    }
    if let Some(arr) = g("folder_rules").and_then(|x| x.as_array()) {
        let rules: Vec<FolderRule> = arr
            .iter()
            .filter_map(|r| {
                Some(FolderRule {
                    exts: r.get("exts")?.as_str()?.to_string(),
                    folder: r.get("folder")?.as_str()?.to_string(),
                })
            })
            .take(8)
            .collect();
        if !rules.is_empty() {
            s.folder_rules = rules;
        }
    }
    if let Some(arr) = g("speed_windows").and_then(|x| x.as_array()) {
        let wins: Vec<SpeedWindow> = arr
            .iter()
            .filter_map(|r| {
                Some(SpeedWindow {
                    start: r.get("start")?.as_str()?.to_string(),
                    end: r.get("end")?.as_str()?.to_string(),
                    limit_kbps: r.get("limit_kbps")?.as_u64()?,
                })
            })
            .take(6)
            .collect();
        if !wins.is_empty() {
            s.speed_windows = wins;
        }
    }
    s
}

/// Read-modify-write: overlay our keys onto whatever is on disk now.
fn save(s: &Settings) -> bool {
    let Some(path) = ui_path() else { return false };
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return false,
    };
    let set = |o: &mut serde_json::Map<String, serde_json::Value>, k: &str, v: serde_json::Value| {
        o.insert(k.to_string(), v);
    };
    set(obj, "theme", s.theme.clone().into());
    set(obj, "close_to_tray", s.close_to_tray.into());
    set(obj, "show_complete", s.show_complete.into());
    set(obj, "save_dir", s.save_dir.clone().into());
    set(obj, "category_folders", s.category_folders.into());
    set(obj, "connections_default", (s.connections_default as u64).into());
    set(obj, "speed_limit_kbps", s.speed_limit_kbps.into());
    set(obj, "user_agent", s.user_agent.clone().into());
    set(obj, "proxy_url", s.proxy_url.clone().into());
    set(obj, "timeout_secs", s.timeout_secs.into());
    set(obj, "max_retries", s.max_retries.into());
    set(obj, "antivirus_cmd", s.antivirus_cmd.clone().into());
    set(obj, "finish_action", s.finish_action.clone().into());
    set(obj, "speed_windows_on", s.speed_windows_on.into());
    set(obj, "rename_template", s.rename_template.clone().into());
    set(obj, "ytdlp_prefer_path", s.ytdlp_prefer_path.into());
    set(obj, "update_check_on", s.update_check_on.into());
    set(obj, "folder_rules", serde_json::to_value(&s.folder_rules).unwrap_or_default());
    set(
        obj,
        "speed_windows",
        serde_json::to_value(&s.speed_windows).unwrap_or_default(),
    );
    serde_json::to_string_pretty(&v)
        .ok()
        .and_then(|j| crate::model::atomic_write(&path, j.as_bytes()).ok())
        .is_some()
}

struct SettingsApp {
    pane: usize,
    s: Settings,
    centered: bool,
    notice: String,
    browser_msg: String,
    /// Last theme actually applied, so picking one in Appearance repaints
    /// this window at once (and "system" keeps tracking the OS).
    theme_applied: String,
    /// YouTube helper status. Both asking it for a version and re-fetching it
    /// run a process (or a download), so they happen on a worker thread and
    /// the pane shows the last answer.
    ytdlp: Option<crate::engine::YtdlpInfo>,
    ytdlp_msg: String,
    ytdlp_busy: bool,
    ytdlp_rx: Option<std::sync::mpsc::Receiver<(String, crate::engine::YtdlpInfo)>>,
    // Open/closed state of the three Download-Engine sections (Network starts
    // open, exactly like the concept).
    open_network: bool,
    open_complete: bool,
    open_windows: bool,
}

/// Probe the helper, and optionally re-download it first, off the UI thread.
fn ytdlp_probe(force: bool) -> std::sync::mpsc::Receiver<(String, crate::engine::YtdlpInfo)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut msg = String::new();
        if force {
            msg = match crate::engine::ytdlp_refresh_blocking() {
                Ok(p) => format!("Fetched the newest build into {}", p.display()),
                Err(e) => format!("Re-download failed: {e}"),
            };
        }
        let _ = tx.send((msg, crate::engine::ytdlp_info()));
    });
    rx
}

fn install_one(browser: &str) -> String {
    match crate::app::install_native_host(browser) {
        Ok(manifest) => format!("Installed {browser} host ✓ ({manifest})"),
        Err(e) => format!("Install failed: {e}"),
    }
}

/// Entry point for `--settings`.
pub fn run() -> eframe::Result<()> {
    let app = SettingsApp {
        pane: 0,
        s: load(),
        centered: false,
        notice: String::new(),
        browser_msg: String::new(),
        theme_applied: String::new(),
        ytdlp: None,
        ytdlp_msg: String::new(),
        ytdlp_busy: false,
        ytdlp_rx: None,
        open_network: true,
        open_complete: false,
        open_windows: false,
    };
    // Separate OS window (own process via `--settings`): never embedded in
    // the main GUI. 640 wide is the concept's window width.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download Craft Settings")
        .with_inner_size([640.0, 520.0])
        .with_min_inner_size([600.0, 430.0])
        .with_resizable(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Settings",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(|cc| {
            crate::app::setup_fonts(cc);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Register the native handle once so the title bar can follow the
        // theme (this is its own process, same helper as the main window).
        {
            use raw_window_handle::HasWindowHandle;
            if let Ok(handle) = frame.window_handle() {
                if let raw_window_handle::RawWindowHandle::Win32(w) = handle.as_raw() {
                    crate::tray::set_hwnd(w.hwnd.get());
                }
            }
        }
        let resolved = format!(
            "{}:{}",
            self.s.theme,
            crate::app::theme_is_dark(ctx, &self.s.theme)
        );
        if self.theme_applied != resolved {
            crate::app::apply_theme(ctx, &self.s.theme);
            crate::tray::set_titlebar_dark(crate::app::theme_is_dark(ctx, &self.s.theme));
            self.theme_applied = resolved;
        }
        if !self.centered && crate::model::center_window(ctx, egui::vec2(640.0, 520.0)) {
            self.centered = true;
        }
        // The helper's status is asked for when the pane is first opened.
        if self.pane == PANE_YTDLP && self.ytdlp.is_none() && self.ytdlp_rx.is_none() {
            self.ytdlp_rx = Some(ytdlp_probe(false));
            self.ytdlp_busy = true;
        }
        if let Some(rx) = &self.ytdlp_rx {
            if let Ok((msg, info)) = rx.try_recv() {
                if !msg.is_empty() {
                    self.ytdlp_msg = msg;
                }
                self.ytdlp = Some(info);
                self.ytdlp_rx = None;
                self.ytdlp_busy = false;
            }
        }

        let t = tok();
        // One manual composition instead of three nested panels: the concept
        // has a fixed rail border, a full-width footer rule and a 20 px content
        // inset, all of which are easier to hit exactly with rects.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let foot_h = 49.0;
                let rail_w = 168.0;
                let foot = Rect::from_min_max(
                    egui::pos2(full.left(), full.bottom() - foot_h),
                    full.max,
                );
                let rail =
                    Rect::from_min_max(full.min, egui::pos2(full.left() + rail_w, foot.top()));
                let body = Rect::from_min_max(
                    egui::pos2(rail.right(), full.top()),
                    egui::pos2(full.right(), foot.top()),
                );

                let p = ui.painter();
                p.rect_filled(full, 0.0, t.bg);
                p.vline(rail.right(), rail.y_range(), Stroke::new(1.0, t.border));
                p.hline(full.x_range(), foot.top(), Stroke::new(1.0, t.border));

                // ---- rail ----
                let mut rail_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rail.shrink(8.0))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                rail_ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
                for (i, (glyph, name)) in PANES.iter().enumerate() {
                    if rail_item(&mut rail_ui, glyph, name, self.pane == i) {
                        self.pane = i;
                    }
                }

                // ---- footer ----
                let mut foot_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(foot.shrink2(Vec2::new(14.0, 10.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                foot_ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                if button(&mut foot_ui, "Save", false).clicked() {
                    self.notice = if save(&self.s) {
                        "Saved ✓ (the main app picks it up live)".to_string()
                    } else {
                        "Could not write settings.".to_string()
                    };
                }
                if !self.notice.is_empty() {
                    foot_ui.add_space(4.0);
                    txt(&mut foot_ui, self.notice.as_str(), reg(10.0), t.muted);
                }
                foot_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if button(ui, "Close", false).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                // ---- content ----
                let mut body_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(body.shrink2(Vec2::new(20.0, 16.0)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                body_ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                let pane = self.pane;
                egui::ScrollArea::vertical()
                    .id_salt(("settings_scroll", pane))
                    .auto_shrink([false, false])
                    .show(&mut body_ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                        match pane {
                            0 => self.pane_appearance(ui),
                            1 => self.pane_engine(ui),
                            PANE_YTDLP => self.pane_plugins(ui),
                            3 => self.pane_browser(ui),
                            4 => self.pane_rules(ui),
                            _ => self.pane_updates(ui),
                        }
                        ui.add_space(16.0);
                    });
            });

        // The plugin probe runs on a worker thread: keep repainting while it
        // is pending so its answer appears without a mouse move.
        if self.ytdlp_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

// ===================== concept tokens + primitives =====================

/// The concept's token set, read from the live palette so the window follows
/// System/Dark/Light with every other surface. Slate/blue hex values from the
/// mockup map onto the app's nearest equivalent.
struct Tok {
    /// Window, rail and content background (`--bg`).
    bg: Color32,
    /// Cards and plain buttons (`--bg-secondary`, `--btn-bg`).
    card: Color32,
    /// Text fields and value boxes (`--bg-input`).
    input: Color32,
    /// Hover wash (`--bg-hover`).
    hover: Color32,
    /// Plain/secondary button fill (`--btn-bg`). A real step off the card, so a
    /// secondary button reads as a button instead of stray text: the mockup
    /// pairs `--btn-bg` with `--btn-border` on every non-primary `.btn`.
    btn: Color32,
    /// Plain button border (`--btn-border`).
    btn_bd: Color32,
    /// Plain button hover fill: the next step up from `btn`.
    btn_hover: Color32,
    text: Color32,
    text2: Color32,
    muted: Color32,
    border: Color32,
    accent: Color32,
    accent_hi: Color32,
    /// Toggle/slider trough when off (`--toggle-off`).
    track: Color32,
}

fn tok() -> Tok {
    let p = pl();
    let dark = p.txt.r() > 128;
    let card = if dark { p.surface_lt } else { p.bg };
    Tok {
        bg: p.surface,
        card,
        input: if dark { p.central_bg } else { p.bg },
        hover: if dark { p.dm_hover2 } else { p.surface_lt },
        btn: blend(card, p.txt, 0.08),
        btn_bd: blend(card, p.txt, 0.22),
        btn_hover: blend(card, p.txt, 0.18),
        text: p.txt,
        text2: p.dim,
        muted: if dark { p.dm_lbl } else { p.dm_head },
        border: p.card_bd,
        accent: p.primary,
        accent_hi: if dark {
            blend(p.primary, Color32::WHITE, 0.18)
        } else {
            blend(p.primary, Color32::BLACK, 0.12)
        },
        track: p.track,
    }
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Translucent accent wash for active rows and brand tiles.
fn wash(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

fn txt(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.add(
        egui::Label::new(egui::RichText::new(s.into()).font(font).color(color))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Extend)
            .sense(Sense::hover()),
    )
}

fn wrapped(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.add(
        egui::Label::new(egui::RichText::new(s.into()).font(font).color(color))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Wrap),
    )
}

/// `h2` of the concept: 16 px semibold with a 16 px gap below.
fn h2(ui: &mut Ui, s: &str) {
    txt(ui, s, semi(16.0), tok().text);
    ui.add_space(16.0);
}

/// Setting label: 12 px, secondary colour.
fn lbl(ui: &mut Ui, s: &str) {
    txt(ui, s, reg(12.0), tok().text2);
}

/// Compact label used inside expandable sections.
fn lbl11(ui: &mut Ui, s: &str) {
    txt(ui, s, reg(11.0), tok().text2);
}

/// 10 px muted hint.
fn hint(ui: &mut Ui, s: &str) {
    wrapped(ui, s, reg(10.0), tok().muted);
}

/// 11 px secondary paragraph.
fn para(ui: &mut Ui, s: &str) {
    wrapped(ui, s, reg(11.0), tok().text2);
}

/// Plain button: `--btn-bg` / `--btn-border`, 5 px radius, 6×16 padding.
fn button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let t = tok();
    let enabled = ui.is_enabled();
    let fg = if primary {
        Color32::WHITE
    } else if enabled {
        t.text2
    } else {
        t.muted
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let size = Vec2::new(
        galley.size().x + 32.0,
        (galley.size().y + 12.0).max(28.0),
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered() && enabled;
    let (bg, bd) = if primary {
        (
            if hov { t.accent_hi } else { t.accent },
            if hov { t.accent_hi } else { t.accent },
        )
    } else {
        (if hov { t.btn_hover } else { t.btn }, t.btn_bd)
    };
    // A hovered secondary button also brightens its label, so the highlight is
    // unmistakable even where the fill step is small.
    let galley = if !primary && hov {
        ui.painter().layout_no_wrap(label.to_owned(), reg(11.0), t.text)
    } else {
        galley
    };
    let p = ui.painter();
    p.rect(
        rect,
        5.0,
        bg,
        Stroke::new(1.0, bd),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// `--theme-opt` pill: fully rounded, accent-filled when selected.
fn pill(ui: &mut Ui, label: &str, active: bool) -> Response {
    let t = tok();
    let fg = if active { Color32::WHITE } else { t.text2 };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let size = Vec2::new(galley.size().x + 28.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let (bg, bd) = if active {
        (t.accent, t.accent)
    } else {
        (if resp.hovered() { t.btn_hover } else { t.btn }, t.btn_bd)
    };
    let p = ui.painter();
    p.rect(rect, 13.0, bg, Stroke::new(1.0, bd), StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Square icon button (`--btn-icon`, 30×30 / `--rule-btn`, 28×28).
fn icon_sq(ui: &mut Ui, glyph: &str, size: f32, radius: f32) -> Response {
    let t = tok();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    p.rect(
        rect,
        radius,
        if resp.hovered() { t.btn_hover } else { t.btn },
        Stroke::new(1.0, t.btn_bd),
        StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(size * 0.42),
        if resp.hovered() { t.text } else { t.text2 },
    );
    resp
}

/// Rule row button: bordered up/down, borderless delete with a red hover.
fn rule_btn(ui: &mut Ui, glyph: &str, danger: bool) -> Response {
    let t = tok();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if danger {
        let red = Color32::from_rgb(0xef, 0x44, 0x44);
        if resp.hovered() {
            p.rect_filled(rect, 4.0, wash(red, 24));
        }
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(12.0),
            if resp.hovered() { red } else { t.muted },
        );
    } else {
        p.rect(
            rect,
            4.0,
            if resp.hovered() { t.btn_hover } else { t.btn },
            Stroke::new(1.0, t.btn_bd),
            StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(12.0),
            if resp.hovered() { t.text } else { t.text2 },
        );
    }
    resp
}

/// Frameless editor inside a painted box (`.input`): 5 px radius, 1 px border,
/// accent border while focused.
fn field(ui: &mut Ui, buf: &mut String, width: Option<f32>, hint: &str) -> Response {
    let t = tok();
    let total = width.unwrap_or_else(|| ui.available_width()).max(60.0);
    let text_w = (total - 20.0).max(20.0);
    let out = egui::Frame::new()
        .fill(t.input)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(text_w);
            ui.add(
                egui::TextEdit::singleline(buf)
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .font(reg(11.0))
                    .text_color(t.text)
                    .hint_text(egui::RichText::new(hint).font(reg(11.0)).color(t.muted))
                    .desired_width(text_w)
                    .vertical_align(egui::Align::Center),
            )
        });
    if out.inner.has_focus() {
        ui.painter().rect_stroke(
            out.response.rect,
            5.0,
            Stroke::new(1.0, t.accent),
            StrokeKind::Inside,
        );
    }
    out.inner
}

/// `.toggle`: 32×18 track, 14 px white thumb; the whole row is clickable.
fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> bool {
    let t = tok();
    let w = ui.available_width().max(80.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let anim = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let track = Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - 9.0),
        Vec2::new(32.0, 18.0),
    );
    let p = ui.painter();
    p.rect_filled(track, 9.0, blend(t.track, t.accent, anim));
    p.circle_filled(
        egui::pos2(track.left() + 9.0 + 14.0 * anim, track.center().y),
        7.0,
        Color32::WHITE,
    );
    p.text(
        egui::pos2(track.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        reg(12.0),
        t.text,
    );
    if resp.clicked() {
        *on = !*on;
        true
    } else {
        false
    }
}

/// `.checkbox`: 14×14 box (12×12 compact), accent fill + tick when checked.
/// `full` stretches the click row to the pane width, otherwise it shrinks.
fn checkbox(
    ui: &mut Ui,
    on: &mut bool,
    label: &str,
    box_size: f32,
    font_size: f32,
    full: bool,
) -> bool {
    let t = tok();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), reg(font_size), t.text);
    let w = if full {
        ui.available_width().max(80.0)
    } else {
        box_size + 8.0 + galley.size().x + 2.0
    };
    let h = box_size.max(galley.size().y).max(16.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let br = Rect::from_min_size(
        egui::pos2(rect.left() + 1.0, rect.center().y - box_size / 2.0),
        Vec2::splat(box_size),
    );
    let p = ui.painter();
    let border = if *on {
        t.accent
    } else if resp.hovered() {
        t.text2
    } else {
        t.border
    };
    p.rect(
        br,
        3.0,
        if *on { t.accent } else { Color32::TRANSPARENT },
        Stroke::new(1.5, border),
        StrokeKind::Inside,
    );
    if *on {
        let c = br.center();
        let s = box_size / 14.0;
        let tick = Stroke::new(1.8, Color32::WHITE);
        p.line_segment(
            [c + Vec2::new(-4.5, 0.0) * s, c + Vec2::new(-1.5, 3.0) * s],
            tick,
        );
        p.line_segment(
            [c + Vec2::new(-1.5, 3.0) * s, c + Vec2::new(4.5, -3.5) * s],
            tick,
        );
    }
    p.galley(
        egui::pos2(br.right() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        t.text,
    );
    if resp.clicked() {
        *on = !*on;
        true
    } else {
        false
    }
}

/// `.slider-row`: 4 px track, accent fill, 14 px thumb, bordered value box.
fn slider(ui: &mut Ui, salt: &str, value: &mut f64, min: f64, max: f64, log: bool) -> bool {
    let t = tok();
    let w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::hover());
    let box_rect = Rect::from_min_size(
        egui::pos2(rect.right() - 50.0, rect.top()),
        Vec2::new(50.0, 24.0),
    );
    let track_area = Rect::from_min_max(
        egui::pos2(rect.left() + 2.0, rect.top()),
        egui::pos2(box_rect.left() - 10.0, rect.bottom()),
    );
    let resp = ui.interact(
        track_area,
        ui.id().with(("slider", salt)),
        Sense::click_and_drag(),
    );
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(ptr) = resp.interact_pointer_pos() {
            let frac = ((ptr.x - track_area.left()) / track_area.width().max(1.0)).clamp(0.0, 1.0)
                as f64;
            *value = if log {
                (min.ln() + frac * (max.ln() - min.ln())).exp().round()
            } else {
                (min + frac * (max - min)).round()
            };
            changed = true;
        }
    }
    let frac = if log {
        ((value.max(min).ln() - min.ln()) / (max.ln() - min.ln())).clamp(0.0, 1.0)
    } else {
        ((*value - min) / (max - min)).clamp(0.0, 1.0)
    } as f32;
    let cy = rect.center().y;
    let track = Rect::from_min_max(
        egui::pos2(track_area.left(), cy - 2.0),
        egui::pos2(track_area.right(), cy + 2.0),
    );
    let p = ui.painter();
    p.rect_filled(track, 2.0, t.track);
    if frac > 0.0 {
        p.rect_filled(
            Rect::from_min_max(track.min, egui::pos2(track.left() + track.width() * frac, track.max.y)),
            2.0,
            t.accent,
        );
    }
    let knob = egui::pos2(track.left() + track.width() * frac, cy);
    if resp.hovered() || resp.dragged() {
        p.circle_filled(knob, 9.0, wash(t.accent, 36));
    }
    p.circle_filled(knob, 7.0, t.accent);
    p.circle_stroke(knob, 7.0, Stroke::new(2.0, t.bg));
    p.rect(
        box_rect,
        4.0,
        t.input,
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    p.text(
        box_rect.center(),
        Align2::CENTER_CENTER,
        format!("{}", *value as i64),
        reg(11.0),
        t.text,
    );
    changed
}

/// `.expandable`: chevron header (rotates when open) + indented body.
fn expandable(ui: &mut Ui, title: &str, open: &mut bool, body: impl FnOnce(&mut Ui)) {
    let t = tok();
    ui.add_space(2.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let c = egui::pos2(rect.left() + 5.0, rect.center().y);
    let p = ui.painter();
    let st = Stroke::new(1.5, if resp.hovered() { t.text } else { t.text2 });
    if *open {
        p.line_segment([c + Vec2::new(-4.0, -3.0), c + Vec2::new(0.0, 1.0)], st);
        p.line_segment([c + Vec2::new(0.0, 1.0), c + Vec2::new(4.0, -3.0)], st);
    } else {
        p.line_segment([c + Vec2::new(-3.0, -4.0), c + Vec2::new(1.0, 0.0)], st);
        p.line_segment([c + Vec2::new(1.0, 0.0), c + Vec2::new(-3.0, 4.0)], st);
    }
    p.text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        reg(12.0),
        t.text2,
    );
    if resp.clicked() {
        *open = !*open;
    }
    if *open {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                body(ui);
            });
        });
        ui.add_space(2.0);
    }
}

/// Sidebar entry: 14 px icon + 12 px label, 6 px radius, accent active row.
fn rail_item(ui: &mut Ui, glyph: &str, label: &str, active: bool) -> bool {
    let t = tok();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 6.0, wash(t.accent, 30));
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, t.hover);
    }
    let col = if active { t.accent } else { t.text2 };
    p.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        glyph,
        FontId::proportional(14.0),
        col,
    );
    p.text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if active { semi(11.5) } else { reg(11.5) },
        col,
    );
    resp.clicked()
}

// ===================== Plugins card pieces =====================

/// Platform brands the helper understands. `follows_text` marks logos that
/// are black/white by design (they invert with the theme); `letter` entries
/// are drawn as a brand initial because Material Icons has no such logo.
const PLATFORMS: &[(&str, &str, Color32, bool)] = &[
    ("YouTube", "\u{f06a}", Color32::from_rgb(0xff, 0x00, 0x00), false), // smart_display
    ("Facebook", "\u{f234}", Color32::from_rgb(0x18, 0x77, 0xf2), false),
    ("TikTok", "\u{e405}", Color32::from_rgb(0x00, 0x00, 0x00), true), // music_note
    ("Instagram", "\u{e412}", Color32::from_rgb(0xe1, 0x30, 0x6c), false), // photo_camera
    ("X/Twitter", "X", Color32::from_rgb(0x00, 0x00, 0x00), true),
    ("Twitch", "\u{e04b}", Color32::from_rgb(0x91, 0x46, 0xff), false), // videocam
    ("Reddit", "\u{eaa0}", Color32::from_rgb(0xff, 0x45, 0x00), false),
    ("Discord", "\u{ea28}", Color32::from_rgb(0x58, 0x65, 0xf2), false), // sports_esports
    ("Vimeo", "V", Color32::from_rgb(0x1a, 0xb7, 0xea), false),
    ("Snapchat", "\u{ea6e}", Color32::from_rgb(0xe3, 0xc4, 0x00), false),
    ("Dailymotion", "d", Color32::from_rgb(0x00, 0xb8, 0xa9), false),
];

/// Platform tiles (32×32 icon + 8 px name), wrapping like the concept. The
/// rows keep the mockup's 8 px vertical gap (`.platform-grid { gap: 8px }`), so
/// the names of one row never sit against the tiles of the next.
fn platform_grid(ui: &mut Ui) {
    let t = tok();
    let item_w = 44.0;
    let gap = 8.0;
    let avail = ui.available_width();
    let per = (((avail + gap) / (item_w + gap)).floor() as usize)
        .clamp(1, PLATFORMS.len());
    let rows = PLATFORMS.len().div_ceil(per);
    // 32 px tile + 2 px + 8 px name = 44 px item, plus the concept's 8 px row
    // gap (the old 45 px stride left the rows touching).
    let row_h = 52.0;
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(avail, rows as f32 * row_h), Sense::hover());
    let p = ui.painter();
    for (i, (name, glyph, brand, follows_text)) in PLATFORMS.iter().enumerate() {
        let x = rect.left() + (i % per) as f32 * (item_w + gap);
        let y = rect.top() + (i / per) as f32 * row_h;
        let tile = Rect::from_min_size(egui::pos2(x + 6.0, y), Vec2::splat(32.0));
        let col = if *follows_text { t.text } else { *brand };
        p.rect_filled(tile, 8.0, wash(col, 28));
        let is_letter =
            glyph.chars().count() == 1 && glyph.as_bytes()[0].is_ascii_alphabetic();
        p.text(
            tile.center(),
            Align2::CENTER_CENTER,
            *glyph,
            if is_letter {
                semi(14.0)
            } else {
                FontId::proportional(16.0)
            },
            col,
        );
        p.text(
            egui::pos2(x + 22.0, y + 34.0),
            Align2::CENTER_TOP,
            *name,
            reg(8.0),
            t.muted,
        );
    }
}

/// Two-column bullet list of what the helper can fetch.
fn capabilities(ui: &mut Ui) {
    const CAPS: [&str; 6] = [
        "YouTube: Videos, audio, playlists, channels",
        "Facebook: Public videos and Reels",
        "TikTok, Instagram, Vimeo, Reddit",
        "X (Twitter), Twitch, and many more",
        "Resolutions up to 1080p and 4K",
        "Subtitles and thumbnails",
    ];
    let t = tok();
    let avail = ui.available_width();
    let col_w = (avail - 12.0) / 2.0;
    let line_h = 16.0;
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(avail, 3.0 * line_h), Sense::hover());
    let p = ui.painter();
    for (i, cap) in CAPS.iter().enumerate() {
        let x = rect.left() + (i % 2) as f32 * (col_w + 12.0);
        let y = rect.top() + (i / 2) as f32 * line_h + line_h / 2.0;
        p.text(
            egui::pos2(x, y),
            Align2::LEFT_CENTER,
            "•",
            reg(10.0),
            t.muted,
        );
        p.text(
            egui::pos2(x + 12.0, y),
            Align2::LEFT_CENTER,
            *cap,
            reg(10.0),
            t.text2,
        );
    }
}

impl SettingsApp {
    // ---------- Appearance ----------
    fn pane_appearance(&mut self, ui: &mut Ui) {
        h2(ui, "Appearance");
        lbl(ui, "Theme:");
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (key, name) in [("system", "System"), ("dark", "Dark"), ("light", "Light")] {
                if pill(ui, name, self.s.theme == key).clicked() {
                    self.s.theme = key.to_string();
                }
            }
        });
        ui.add_space(14.0);
        toggle(
            ui,
            &mut self.s.close_to_tray,
            "Minimize to system tray instead of quitting",
        );
        ui.add_space(10.0);
        hint(
            ui,
            "Fonts, radius, and accent stay identical — only the palette changes.",
        );
    }

    // ---------- Download Engine ----------
    fn pane_engine(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Download Engine");
        let s = &mut self.s;

        lbl(ui, "Save folder:");
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = (ui.available_width() - 36.0).max(120.0);
            field(ui, &mut s.save_dir, Some(w), "C:\\Downloads");
            if icon_sq(ui, "\u{e2c7}", 30.0, 5.0)
                .on_hover_text("Browse…")
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    s.save_dir = p.to_string_lossy().to_string();
                }
            }
        });
        ui.add_space(12.0);

        checkbox(
            ui,
            &mut s.category_folders,
            "Sort into category subfolders",
            14.0,
            12.0,
            true,
        );
        ui.add_space(10.0);
        checkbox(
            ui,
            &mut s.show_complete,
            "Show the \u{201c}Download complete\u{201d} dialog",
            14.0,
            12.0,
            true,
        );
        ui.add_space(12.0);

        lbl(ui, "Default connections:");
        ui.add_space(5.0);
        let mut conn = s.connections_default as f64;
        if slider(ui, "connections", &mut conn, 1.0, 16.0, false) {
            s.connections_default = conn.round() as usize;
        }
        ui.add_space(12.0);

        lbl(ui, "Speed limit KB/s (0 = unlimited):");
        ui.add_space(5.0);
        let mut lim = s.speed_limit_kbps as f64;
        if slider(ui, "speed_limit", &mut lim, 0.0, 10000.0, false) {
            s.speed_limit_kbps = lim.round() as u64;
        }
        ui.add_space(8.0);

        expandable(ui, "Network", &mut self.open_network, |ui| {
            lbl11(ui, "User-Agent:");
            ui.add_space(4.0);
            field(ui, &mut s.user_agent, None, "default (Download Craft)");
            ui.add_space(10.0);
            lbl11(ui, "Proxy:");
            ui.add_space(4.0);
            field(ui, &mut s.proxy_url, None, "http://host:port (empty = direct)");
            ui.add_space(10.0);
            lbl11(ui, "Timeout (seconds):");
            ui.add_space(4.0);
            let mut timeout = s.timeout_secs as f64;
            if slider(ui, "timeout", &mut timeout, 5.0, 600.0, false) {
                s.timeout_secs = timeout.round() as u64;
            }
            ui.add_space(10.0);
            lbl11(ui, "Retries:");
            ui.add_space(4.0);
            let mut retries = s.max_retries as f64;
            if slider(ui, "retries", &mut retries, 1.0, 20.0, false) {
                s.max_retries = retries.round() as u32;
            }
        });

        expandable(ui, "On completion", &mut self.open_complete, |ui| {
            lbl11(ui, "Antivirus command ({file} = downloaded path):");
            ui.add_space(4.0);
            field(
                ui,
                &mut s.antivirus_cmd,
                None,
                "\"C:\\Program Files\\Defender\\MpCmdRun.exe\" -Scan -ScanType 3 -File {file}",
            );
            ui.add_space(10.0);
            lbl11(ui, "When a download finishes:");
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for name in ["nothing", "shutdown", "sleep", "hibernate"] {
                    if pill(ui, name, s.finish_action == name).clicked() {
                        s.finish_action = name.to_string();
                    }
                }
            });
        });

        expandable(ui, "Speed windows", &mut self.open_windows, |ui| {
            checkbox(
                ui,
                &mut s.speed_windows_on,
                "Enable speed windows",
                14.0,
                12.0,
                true,
            );
            ui.add_space(6.0);
            hint(
                ui,
                "While a window is active its cap overrides the global limit (0 = full speed).",
            );
            ui.add_space(8.0);
            let mut del: Option<usize> = None;
            for (i, w) in s.speed_windows.iter_mut().enumerate().take(6) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    field(ui, &mut w.start, Some(60.0), "02:00");
                    txt(ui, "–", reg(11.0), t.text2);
                    field(ui, &mut w.end, Some(60.0), "06:00");
                    let mut cap = w.limit_kbps.to_string();
                    field(ui, &mut cap, Some(70.0), "0");
                    if let Ok(n) = cap.trim().parse::<u64>() {
                        w.limit_kbps = n;
                    }
                    txt(ui, "KB/s", reg(10.0), t.muted);
                    if rule_btn(ui, "\u{e5cd}", true).clicked() {
                        del = Some(i);
                    }
                });
                ui.add_space(6.0);
            }
            if let Some(i) = del {
                s.speed_windows.remove(i);
            }
            if button(ui, "Add window", false).clicked() && s.speed_windows.len() < 6 {
                s.speed_windows.push(SpeedWindow {
                    start: "02:00".to_string(),
                    end: "06:00".to_string(),
                    limit_kbps: 0,
                });
            }
        });
    }

    // ---------- Plugins (the yt-dlp helper, as a catalogue card) ----------
    fn pane_plugins(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Plugins");

        let info = self.ytdlp.clone();
        let msg = self.ytdlp_msg.clone();
        let busy = self.ytdlp_busy;

        let ver_line = match &info {
            None => {
                if busy {
                    "Checking the helper…".to_string()
                } else {
                    "Not checked yet".to_string()
                }
            }
            Some(i) => {
                let ver = i
                    .version
                    .clone()
                    .unwrap_or_else(|| "version unknown".to_string());
                let size = i
                    .size
                    .map(crate::model::format_bytes)
                    .unwrap_or_else(|| "—".to_string());
                format!("{ver} — {size}")
            }
        };

        let w = ui.available_width();
        let card_w = w - 28.0;
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(8.0)
            .inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 12,
                bottom: 12,
            })
            .show(ui, |ui| {
                ui.set_width(card_w);
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);

                // card header: tile + name + version
                let (hrect, _) = ui.allocate_exact_size(Vec2::new(card_w, 32.0), Sense::hover());
                let p = ui.painter();
                let icon = Rect::from_min_size(hrect.min, Vec2::splat(32.0));
                p.rect_filled(icon, 8.0, wash(t.accent, 28));
                p.text(
                    icon.center(),
                    Align2::CENTER_CENTER,
                    "\u{e24d}", // insert_drive_file
                    FontId::proportional(16.0),
                    t.accent,
                );
                p.text(
                    egui::pos2(icon.right() + 10.0, hrect.center().y - 8.0),
                    Align2::LEFT_CENTER,
                    "yt-dlp",
                    semi(13.0),
                    t.text,
                );
                p.text(
                    egui::pos2(icon.right() + 10.0, hrect.center().y + 8.0),
                    Align2::LEFT_CENTER,
                    &ver_line,
                    reg(10.0),
                    t.muted,
                );

                ui.add_space(8.0);
                platform_grid(ui);
                ui.add_space(8.0);
                capabilities(ui);
                ui.add_space(10.0);

                // actions
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add_enabled_ui(!busy, |ui| {
                        if button(ui, "Re-download", true).clicked() {
                            self.ytdlp_rx = Some(ytdlp_probe(true));
                            self.ytdlp_busy = true;
                            self.ytdlp_msg.clear();
                        }
                    });
                    if busy {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                    if button(ui, "Show in folder", false).clicked() {
                        if let Some(dir) = crate::engine::ytdlp_bin_dir() {
                            let _ = std::fs::create_dir_all(&dir);
                            crate::app::open_path(&dir, false);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        checkbox(
                            ui,
                            &mut self.s.ytdlp_prefer_path,
                            "Prefer PATH yt-dlp",
                            12.0,
                            10.0,
                            false,
                        );
                    });
                });

                // Live status: where the helper comes from, freshness, JS runtime.
                if !msg.is_empty() {
                    ui.add_space(6.0);
                    wrapped(ui, msg.as_str(), reg(10.0), t.accent);
                }
                if let Some(i) = &info {
                    let mut notes: Vec<(String, bool)> = Vec::new();
                    match (&i.path, i.managed) {
                        (None, _) => notes.push((
                            "No yt-dlp present yet — the first YouTube download fetches it."
                                .to_string(),
                            false,
                        )),
                        (Some(p), true) => {
                            notes.push((format!("This app's own copy: {}", p.display()), false))
                        }
                        (Some(p), false) => {
                            notes.push((format!("Using the yt-dlp on your PATH: {}", p.display()), false))
                        }
                    }
                    if i.stale {
                        notes.push((
                            "Past the 30-day refresh window — press Re-download.".to_string(),
                            true,
                        ));
                    }
                    if i.managed && i.on_path.is_some() {
                        notes.push((
                            "Your PATH copy is ignored right now — tick \u{201c}Prefer PATH yt-dlp\u{201d} to use it."
                                .to_string(),
                            false,
                        ));
                    }
                    notes.push(match &i.js_runtime {
                        Some(r) => (
                            format!("JavaScript runtime: {r} — solves YouTube's player challenge."),
                            false,
                        ),
                        None => (
                            "No JavaScript runtime found — install Node.js if YouTube downloads stall at \u{201c}Getting info\u{201d}."
                                .to_string(),
                            true,
                        ),
                    });
                    for (note, warn) in notes {
                        ui.add_space(4.0);
                        wrapped(
                            ui,
                            note,
                            reg(9.0),
                            if warn { pl().warning } else { t.muted },
                        );
                    }
                }
            });

        ui.add_space(8.0);
        wrapped(
            ui,
            "More plugins will appear here as they are added. Each plugin gets its own card with info, actions, and options.",
            reg(9.0),
            t.muted,
        );
    }

    // ---------- Browser Integration ----------
    fn pane_browser(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Browser Integration");
        para(
            ui,
            "Load browser-extension/ unpacked once — its ID is pinned, no pasting needed.",
        );
        ui.add_space(6.0);
        para(
            ui,
            "One-time setup: host auto-repairs on every launch — buttons below are manual repair only.",
        );
        ui.add_space(6.0);
        para(ui, &format!("Extension ID: {}", crate::app::EXTENSION_ID));
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            for b in ["chrome", "edge", "firefox", "brave", "opera"] {
                if button(ui, &format!("Install {b}"), false).clicked() {
                    self.browser_msg = install_one(b);
                }
            }
        });
        if !self.browser_msg.is_empty() {
            ui.add_space(8.0);
            wrapped(ui, self.browser_msg.as_str(), reg(10.0), t.muted);
        }
    }

    // ---------- Categories & Rules ----------
    fn pane_rules(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Categories & Rules");
        lbl(ui, "Rename template:");
        ui.add_space(5.0);
        field(ui, &mut self.s.rename_template, None, "{name}");
        ui.add_space(4.0);
        hint(ui, "Tokens: {name} {ext} {date} {host}");
        ui.add_space(2.0);
        hint(
            ui,
            &format!(
                "Preview: {}",
                crate::model::apply_rename_template(
                    &self.s.rename_template,
                    "My Video.mp4",
                    "https://cdn.example/v"
                )
            ),
        );
        ui.add_space(14.0);

        lbl(ui, "Type → folder rules (first match wins, max 8):");
        ui.add_space(8.0);

        let avail = ui.available_width();
        let input_w = (avail - 230.0).max(80.0);
        let mut up: Option<usize> = None;
        let mut down: Option<usize> = None;
        let mut del: Option<usize> = None;
        for i in 0..self.s.folder_rules.len().min(8) {
            let rule = &mut self.s.folder_rules[i];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                field(ui, &mut rule.exts, Some(input_w), "mp4,mkv");
                txt(ui, "→", reg(11.0), t.muted);
                field(ui, &mut rule.folder, Some(88.0), "Video");
                if rule_btn(ui, "\u{e5d8}", false).clicked() {
                    up = Some(i);
                }
                if rule_btn(ui, "\u{e5db}", false).clicked() {
                    down = Some(i);
                }
                if rule_btn(ui, "\u{e5cd}", true).clicked() {
                    del = Some(i);
                }
            });
            ui.add_space(8.0);
        }
        if let Some(i) = del {
            self.s.folder_rules.remove(i);
        }
        if let Some(i) = up {
            if i > 0 {
                self.s.folder_rules.swap(i - 1, i);
            }
        }
        if let Some(i) = down {
            if i + 1 < self.s.folder_rules.len() {
                self.s.folder_rules.swap(i, i + 1);
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if button(ui, "Add rule", false).clicked() && self.s.folder_rules.len() < 8 {
                self.s.folder_rules.push(FolderRule {
                    exts: String::new(),
                    folder: String::new(),
                });
            }
            if button(ui, "Reset defaults", false).clicked() {
                self.s.folder_rules = default_folder_rules();
            }
        });
    }

    // ---------- Updates ----------
    fn pane_updates(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Updates");
        checkbox(
            ui,
            &mut self.s.update_check_on,
            "Check for updates on launch",
            14.0,
            12.0,
            true,
        );
        ui.add_space(10.0);
        wrapped(
            ui,
            "Checks GitHub releases; never downloads automatically.",
            reg(11.0),
            t.muted,
        );
    }
}
