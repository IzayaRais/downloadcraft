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
    /// Where the helper comes from: "managed" (this app's own copy), "path"
    /// (the first yt-dlp on PATH) or "custom" (an executable the user picked).
    ytdlp_source: String,
    /// The executable picked for "custom"; empty until one is chosen.
    ytdlp_custom_path: String,
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
            ytdlp_prefer_path: false,
            ytdlp_source: "managed".to_string(),
            ytdlp_custom_path: String::new(),
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
    // Installs from before the three-way choice only carry the checkbox.
    match g("ytdlp_source").and_then(|x| x.as_str()) {
        Some(v @ ("managed" | "path" | "custom")) => s.ytdlp_source = v.to_string(),
        _ => {
            s.ytdlp_source = if s.ytdlp_prefer_path { "path" } else { "managed" }.to_string();
        }
    }
    if let Some(x) = g("ytdlp_custom_path").and_then(|x| x.as_str()) {
        s.ytdlp_custom_path = x.to_string();
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
    set(obj, "ytdlp_source", s.ytdlp_source.clone().into());
    set(obj, "ytdlp_custom_path", s.ytdlp_custom_path.clone().into());
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
    ytdlp_rx: Option<
        std::sync::mpsc::Receiver<(
            String,
            crate::engine::YtdlpInfo,
            crate::engine::FfmpegInfo,
        )>,
    >,
    /// ffmpeg is only used to merge separate video+audio streams; its card
    /// says whether there is one, and how new it is.
    ffmpeg: Option<crate::engine::FfmpegInfo>,
    /// Which plugin cards are expanded (yt-dlp starts open, like the concept).
    plugins_open: [bool; 2],
    // Open/closed state of the Download-Engine cards. The pane is long, so every
    // card starts collapsed: the five heads stay readable at a glance and each
    // section opens on demand.
    open_storage: bool,
    open_perf: bool,
    open_network: bool,
    open_complete: bool,
    open_windows: bool,
}

/// Probe the helper, and optionally re-download it first, off the UI thread.
/// ffmpeg is asked in the same pass (it also runs a process).
fn ytdlp_probe(
    force: bool,
) -> std::sync::mpsc::Receiver<(String, crate::engine::YtdlpInfo, crate::engine::FfmpegInfo)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut msg = String::new();
        if force {
            msg = match crate::engine::ytdlp_refresh_blocking() {
                Ok(p) => format!("Fetched the newest build into {}", p.display()),
                Err(e) => format!("Re-download failed: {e}"),
            };
        }
        let info = crate::engine::ytdlp_info();
        let ffmpeg = crate::engine::ffmpeg_info();
        let _ = tx.send((msg, info, ffmpeg));
    });
    rx
}

/// The native "open file" dialog, for picking a yt-dlp executable.
fn pick_executable() -> Option<std::path::PathBuf> {
    let picked = rfd::FileDialog::new()
        .set_title("Pick a yt-dlp executable")
        .add_filter("yt-dlp", &["exe"])
        .add_filter("All files", &["*"])
        .pick_file();
    picked.filter(|p| p.is_file())
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
        ffmpeg: None,
        plugins_open: [true, false],
        open_storage: false,
        open_perf: false,
        open_network: false,
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
            if let Ok((msg, info, ffmpeg)) = rx.try_recv() {
                if !msg.is_empty() {
                    self.ytdlp_msg = msg;
                }
                self.ytdlp = Some(info);
                self.ffmpeg = Some(ffmpeg);
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
        crate::fonts::icon(size * 0.42),
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
            crate::fonts::icon(12.0),
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
            crate::fonts::icon(12.0),
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
        // A bare `t.border` is a hairline tuned for the pane background; on a
        // card fill the two values are within one step of each other and the
        // box disappears, so an off box gets a real control border (the
        // concept's `--btn-border`) and the concept's input fill.
        blend(t.border, t.text2, 0.35)
    };
    p.rect(
        br,
        3.0,
        if *on { t.accent } else { t.input },
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

// ---------- Download Engine: cards + field rows ----------

/// `--border-soft`: the card body's hairline, a step off the card fill.
fn soft_line(t: &Tok) -> Color32 {
    blend(t.card, t.border, 0.55)
}

/// A collapsible card (`.card`): one painted head that owns the open state and
/// carries the chevron, then the body behind a hairline.
///
/// Everything is built inside a scope salted with the card title: `Frame` hands
/// every one of its content Uis the same id salt (`Id::from("child")`), so
/// without it the implicit ids of identical widgets in two cards (the head, a
/// checkbox) would collide and a click on one card would hit its twin.
fn card(
    ui: &mut Ui,
    title: &str,
    badge: Option<&str>,
    open: &mut bool,
    body: impl FnOnce(&mut Ui),
) {
    let t = tok();
    let mut scope = ui.new_child(egui::UiBuilder::new().id_salt(("engine_card", title)));
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::ZERO)
        .outer_margin(egui::Margin {
            bottom: 6,
            ..Default::default()
        })
        .show(&mut scope, |ui| {
            let w = ui.available_width();
            let (head, resp) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(
                    head,
                    egui::CornerRadius {
                        nw: 9,
                        ne: 9,
                        sw: 0,
                        se: 0,
                    },
                    t.hover,
                );
            }
            let tg = p.layout_no_wrap(title.to_owned(), semi(11.5), t.text);
            p.galley(
                egui::pos2(head.left() + 11.0, head.center().y - tg.size().y / 2.0),
                tg.clone(),
                t.text,
            );
            if let Some(b) = badge {
                let bg = p.layout_no_wrap(b.to_owned(), reg(9.0), t.muted);
                let chip = Rect::from_min_size(
                    egui::pos2(
                        head.left() + 11.0 + tg.size().x + 6.0,
                        head.center().y - 6.5,
                    ),
                    Vec2::new(bg.size().x + 12.0, 13.0),
                );
                p.rect(
                    chip,
                    6.5,
                    t.input,
                    Stroke::new(1.0, t.border),
                    StrokeKind::Inside,
                );
                p.galley(chip.center() - bg.size() / 2.0, bg, t.muted);
            }
            p.text(
                egui::pos2(head.right() - 12.0, head.center().y),
                Align2::CENTER_CENTER,
                if *open { "\u{e5ce}" } else { "\u{e5cf}" }, // expand_less / expand_more
                crate::fonts::icon(13.0),
                t.muted,
            );
            if resp.clicked() {
                *open = !*open;
            }
            if *open {
                let top = ui.cursor().min.y;
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    top,
                    Stroke::new(1.0, soft_line(&t)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 11,
                        right: 11,
                        top: 8,
                        bottom: 9,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        body(ui);
                    });
            }
        });
    ui.advance_cursor_after_rect(scope.min_rect());
}

/// One `.field`: a 138 px label column, the control column beside it and an
/// optional hint under the control. Returns the Ui the control goes into.
fn field_row(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    badge: Option<&str>,
    hint_text: Option<&str>,
) -> Ui {
    let t = tok();
    let full = ui.available_width();
    let label_w = 138.0_f32.min((full * 0.42).max(92.0));
    let gap = 11.0;
    let ctrl_w = (full - label_w - gap).max(60.0);
    let ctrl_h = 28.0;
    let hint = hint_text.map(|h| ui.painter().layout(h.to_owned(), reg(9.5), t.muted, ctrl_w));
    let hint_h = hint.as_ref().map_or(0.0, |g| g.size().y);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(full, ctrl_h + if hint_h > 0.0 { 4.0 + hint_h } else { 0.0 }),
        Sense::hover(),
    );
    let p = ui.painter();
    let lg = p.layout_no_wrap(label.to_owned(), reg(11.0), t.text2);
    let ly = if hint_h > 0.0 {
        rect.top() + 7.0
    } else {
        rect.top() + (ctrl_h - lg.size().y) / 2.0
    };
    p.galley(egui::pos2(rect.left(), ly), lg.clone(), t.text2);
    if let Some(b) = badge {
        let bg = p.layout_no_wrap(b.to_owned(), reg(9.0), t.muted);
        let chip = Rect::from_min_size(
            egui::pos2(rect.left() + lg.size().x + 6.0, ly + lg.size().y / 2.0 - 6.5),
            Vec2::new(bg.size().x + 12.0, 13.0),
        );
        p.rect(
            chip,
            6.5,
            t.input,
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );
        p.galley(chip.center() - bg.size() / 2.0, bg, t.muted);
    }
    if let Some(g) = &hint {
        p.galley(
            egui::pos2(rect.left() + label_w + gap, rect.top() + ctrl_h + 4.0),
            g.clone(),
            t.muted,
        );
    }
    ui.add_space(7.0);
    ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("field", id_salt))
            .max_rect(Rect::from_min_size(
                egui::pos2(rect.left() + label_w + gap, rect.top()),
                Vec2::new(ctrl_w, ctrl_h),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    )
}

/// `.input`: 6 px radius, 1 px border, accent ring while focused.
fn input_box(
    ui: &mut Ui,
    id: &str,
    buf: &mut String,
    w: f32,
    mono: bool,
    center: bool,
    hint: &str,
) -> Response {
    let t = tok();
    let font = if mono {
        egui::FontId::monospace(10.0)
    } else {
        reg(11.0)
    };
    let text_w = (w - 18.0).max(16.0);
    let out = egui::Frame::new()
        .fill(t.input)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.set_width(text_w);
            ui.add(
                egui::TextEdit::singleline(buf)
                    .id(egui::Id::new(id))
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .font(font.clone())
                    .text_color(t.text)
                    .horizontal_align(if center {
                        egui::Align::Center
                    } else {
                        egui::Align::Min
                    })
                    .hint_text(egui::RichText::new(hint).font(font).color(t.muted))
                    .desired_width(text_w)
                    .vertical_align(egui::Align::Center),
            )
        });
    if out.inner.has_focus() {
        ui.painter().rect_stroke(
            out.response.rect,
            6.0,
            Stroke::new(1.0, t.accent),
            StrokeKind::Inside,
        );
    }
    out.inner
}

/// `.slider-row`: the 4 px slider plus its editable value box (two-way).
fn slider_row(ui: &mut Ui, salt: &str, value: &mut f64, min: f64, max: f64) -> bool {
    let t = tok();
    let w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::hover());
    let box_rect = Rect::from_min_size(
        egui::pos2(rect.right() - 52.0, rect.top()),
        Vec2::new(52.0, 26.0),
    );
    let track_area = Rect::from_min_max(
        egui::pos2(rect.left() + 2.0, rect.top()),
        egui::pos2(box_rect.left() - 10.0, rect.bottom()),
    );
    let resp = ui.interact(
        track_area,
        ui.id().with(("srow", salt)),
        Sense::click_and_drag(),
    );
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(ptr) = resp.interact_pointer_pos() {
            let frac =
                ((ptr.x - track_area.left()) / track_area.width().max(1.0)).clamp(0.0, 1.0) as f64;
            *value = (min + frac * (max - min)).round();
            changed = true;
        }
    }
    let frac = (((*value - min) / (max - min)).clamp(0.0, 1.0)) as f32;
    let cy = rect.center().y;
    let track = Rect::from_min_max(
        egui::pos2(track_area.left(), cy - 2.0),
        egui::pos2(track_area.right(), cy + 2.0),
    );
    let p = ui.painter();
    p.rect_filled(track, 2.0, t.track);
    if frac > 0.0 {
        p.rect_filled(
            Rect::from_min_max(
                track.min,
                egui::pos2(track.left() + track.width() * frac, track.max.y),
            ),
            2.0,
            t.accent,
        );
    }
    let knob = egui::pos2(track.left() + track.width() * frac, cy);
    if resp.hovered() || resp.dragged() {
        p.circle_filled(knob, 9.0, wash(t.accent, 36));
    }
    p.circle_filled(knob, 7.0, t.accent);
    p.circle_stroke(knob, 7.0, Stroke::new(2.0, t.card));
    // An editable box, so a big jump needs no dragging.
    let mut buf = format!("{}", *value as i64);
    let mut box_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("srownum", salt))
            .max_rect(box_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let r = input_box(&mut box_ui, &format!("srownum{salt}"), &mut buf, 52.0, false, true, "");
    if r.lost_focus() || r.changed() {
        if let Ok(n) = buf.trim().parse::<f64>() {
            let n = n.clamp(min, max);
            if n != *value {
                *value = n;
                changed = true;
            }
        }
    }
    changed
}

/// `.segmented`: option group in an inset trough, accent-filled selection.
fn segmented(ui: &mut Ui, salt: &str, options: &[&str], current: &mut String) -> bool {
    let t = tok();
    let pad = 2.0;
    let gap = 3.0;
    let seg_h = 24.0;
    let gals: Vec<std::sync::Arc<egui::Galley>> = options
        .iter()
        .map(|o| {
            ui.painter()
                .layout_no_wrap((*o).to_owned(), reg(10.5), t.text2)
        })
        .collect();
    let widths: Vec<f32> = gals.iter().map(|g| g.size().x + 24.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + gap * (options.len() as f32 - 1.0) + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, seg_h + pad * 2.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        8.0,
        t.input,
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    let mut changed = false;
    let mut x = rect.left() + pad;
    for (i, o) in options.iter().enumerate() {
        let seg = Rect::from_min_size(
            egui::pos2(x, rect.top() + pad),
            Vec2::new(widths[i], seg_h),
        );
        let resp = ui
            .interact(seg, ui.id().with(("seg", salt, i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = current == o;
        if active {
            p.rect_filled(seg, 6.0, t.accent);
        } else if resp.hovered() {
            p.rect_filled(seg, 6.0, wash(t.text, 10));
        }
        let fg = if active {
            Color32::WHITE
        } else if resp.hovered() {
            t.text
        } else {
            t.text2
        };
        let g = p.layout_no_wrap((*o).to_owned(), reg(10.5), fg);
        p.galley(seg.center() - g.size() / 2.0, g, fg);
        if resp.clicked() && !active {
            *current = (*o).to_string();
            changed = true;
        }
        x += widths[i] + gap;
    }
    changed
}

/// `.btn-dashed` with a leading icon (the "Add window" button).
fn btn_dashed(ui: &mut Ui, glyph: &str, label: &str) -> Response {
    let t = tok();
    let (iw, tw) = {
        let p = ui.painter();
        (
            p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), t.text2)
                .size()
                .x,
            p.layout_no_wrap(label.to_owned(), reg(11.0), t.text2)
                .size()
                .x,
        )
    };
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(iw + 6.0 + tw + 22.0, 26.0),
        Sense::click(),
    );
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered();
    let fg = if hov { t.accent_hi } else { t.text2 };
    let p = ui.painter();
    dashed_rect(
        p,
        rect,
        6.0,
        Stroke::new(1.0, if hov { t.accent } else { t.btn_bd }),
    );
    let ig = p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), fg);
    let tg = p.layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let x = rect.center().x - (iw + 6.0 + tw) / 2.0;
    p.galley(egui::pos2(x, rect.center().y - ig.size().y / 2.0), ig, fg);
    p.galley(
        egui::pos2(x + iw + 6.0, rect.center().y - tg.size().y / 2.0),
        tg,
        fg,
    );
    resp
}

/// One `.window-row`: start — end, cap, unit, delete. Returns "delete me".
fn window_row(ui: &mut Ui, idx: usize, w: &mut SpeedWindow) -> bool {
    let t = tok();
    let full = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(full, 28.0), Sense::hover());
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("swrow", idx))
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 7.0;
    input_box(&mut row, "sw_start", &mut w.start, 60.0, true, true, "02:00");
    txt(&mut row, "—", reg(11.0), t.muted);
    input_box(&mut row, "sw_end", &mut w.end, 60.0, true, true, "06:00");
    let mut cap = w.limit_kbps.to_string();
    input_box(&mut row, "sw_cap", &mut cap, 52.0, false, true, "0");
    if let Ok(n) = cap.trim().parse::<u64>() {
        w.limit_kbps = n;
    }
    txt(&mut row, "KB/s", reg(10.0), t.muted);
    let del = rule_btn(&mut row, "\u{e5cd}", true).clicked();
    ui.add_space(6.0);
    del
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
        crate::fonts::icon(14.0),
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
/// are black/white by design (they invert with the theme); plain ASCII glyphs
/// ("X", "in", …) are drawn as brand initials because Material Icons has no
/// such logo, and are never asked of the icon font (which would box them).
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
    ("LinkedIn", "in", Color32::from_rgb(0x0a, 0x66, 0xc2), false),
    ("SoundCloud", "\u{e2bd}", Color32::from_rgb(0xff, 0x55, 0x00), false), // cloud
];

/// Closing tile of the platform grid: the size of the extractor list, so the
/// grid does not read as "these thirteen sites and nothing else".
const MORE_SITES: &str = "1,000+";

/// What the YouTube helper fetches (the card's tick list).
const YTDLP_CAPS: [&str; 6] = [
    "YouTube: videos, audio, playlists, channels",
    "Facebook: public videos and Reels",
    "TikTok, Instagram, Vimeo, Reddit",
    "X (Twitter), LinkedIn and 1,000+ more",
    "Up to 1080p and 4K (needs ffmpeg)",
    "Subtitles, thumbnails and metadata",
];

/// Why ffmpeg is worth having (its own card's tick list).
const FFMPEG_CAPS: [&str; 4] = [
    "Merges separate video and audio streams",
    "Required for 1080p and 4K on YouTube",
    "Remuxes and converts containers",
    "Embeds subtitles and cover art",
];

/// Platform tiles in the concept's `repeat(7, 1fr)` grid: 30 px tiles spread
/// edge to edge, 8 px names, closed by a dashed "1,000+ more" tile.
fn platform_grid(ui: &mut Ui) {
    let t = tok();
    let tiles = PLATFORMS.len() + 1; // + the closing "1,000+" tile
    let gap = 4.0; // `.platform-grid { gap: 6px 4px }`
    let avail = ui.available_width();
    let cols = 7.min(tiles).max(1);
    let col_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).max(24.0);
    let rows = tiles.div_ceil(cols);
    // The closing tile's name is two lines ("1,000+" / "more"), so the row
    // holding it is taller. Everything else is one 8 px line.
    let tall_row = (tiles - 1) / cols;
    let row_h = |r: usize| {
        if r == tall_row {
            30.0 + 3.0 + 19.0
        } else {
            30.0 + 3.0 + 10.0
        }
    };
    let total: f32 = (0..rows).map(row_h).sum::<f32>() + 6.0 * (rows as f32 - 1.0).max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, total), Sense::hover());
    let p = ui.painter();
    let mut y = rect.top();
    for r in 0..rows {
        for c in 0..cols {
            let i = r * cols + c;
            if i >= tiles {
                break;
            }
            let cx = rect.left() + c as f32 * (col_w + gap) + col_w / 2.0;
            let tile = Rect::from_center_size(egui::pos2(cx, y + 15.0), Vec2::splat(30.0));
            if i == PLATFORMS.len() {
                // Dashed outline + plus: the extractor list does not stop here.
                dashed_rect(p, tile, 8.0, Stroke::new(1.0, t.btn_bd));
                p.text(
                    tile.center(),
                    Align2::CENTER_CENTER,
                    "\u{e145}", // add
                    crate::fonts::icon(16.0),
                    t.muted,
                );
                p.text(
                    egui::pos2(cx, tile.bottom() + 3.0),
                    Align2::CENTER_TOP,
                    MORE_SITES,
                    reg(8.0),
                    t.muted,
                );
                p.text(
                    egui::pos2(cx, tile.bottom() + 11.5),
                    Align2::CENTER_TOP,
                    "more",
                    reg(8.0),
                    t.muted,
                );
                continue;
            }
            let (name, glyph, brand, follows_text) = PLATFORMS[i];
            let col = if follows_text { t.text } else { brand };
            p.rect_filled(tile, 8.0, wash(col, 30));
            // Brand initials are ASCII ("X", "V", "d", "in"); everything else is
            // a Material Icons codepoint. Asking the icon font for a letter
            // would paint a .notdef box, so the two are never mixed up.
            p.text(
                tile.center(),
                Align2::CENTER_CENTER,
                glyph,
                if glyph.is_ascii() {
                    semi(if glyph.len() > 1 { 11.0 } else { 13.0 })
                } else {
                    crate::fonts::icon(16.0)
                },
                col,
            );
            p.text(
                egui::pos2(cx, tile.bottom() + 3.0),
                Align2::CENTER_TOP,
                name,
                reg(8.0),
                t.muted,
            );
        }
        y += row_h(r) + 6.0;
    }
}

/// Dashed rounded outline (the concept's `1px dashed` "and 1,000+ more" tile).
fn dashed_rect(p: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    let (dash, gap) = (3.0, 2.0);
    // Straight runs, inset so the dashes sit inside the rounded corner.
    let r = Rect::from_min_max(
        egui::pos2(rect.left() + 2.0 + radius * 0.15, rect.top() + 0.5),
        egui::pos2(rect.right() - 2.0 - radius * 0.15, rect.bottom() - 0.5),
    );
    let mut x = r.left();
    while x < r.right() {
        let x2 = (x + dash).min(r.right());
        p.line_segment([egui::pos2(x, r.top()), egui::pos2(x2, r.top())], stroke);
        p.line_segment([egui::pos2(x, r.bottom()), egui::pos2(x2, r.bottom())], stroke);
        x += dash + gap;
    }
    let mut y = r.top();
    while y < r.bottom() {
        let y2 = (y + dash).min(r.bottom());
        p.line_segment([egui::pos2(r.left(), y), egui::pos2(r.left(), y2)], stroke);
        p.line_segment([egui::pos2(r.right(), y), egui::pos2(r.right(), y2)], stroke);
        y += dash + gap;
    }
}

/// Two-column tick list (`.feat-list`): a green vector check then 10 px text.
/// Both columns are pre-wrapped so neither can push the other around.
fn capabilities(ui: &mut Ui, caps: &[&str]) {
    let t = tok();
    let green = ok_green();
    let avail = ui.available_width();
    let gap = 14.0;
    let col_w = ((avail - gap) / 2.0).max(60.0);
    let marker = 11.0; // tick + air
    let rows = caps.len().div_ceil(2);
    let galleys: Vec<std::sync::Arc<egui::Galley>> = {
        let p = ui.painter();
        caps.iter()
            .map(|c| p.layout((*c).to_owned(), reg(10.0), t.text2, (col_w - marker).max(24.0)))
            .collect()
    };
    let heights: Vec<f32> = (0..rows)
        .map(|r| {
            (0..2)
                .filter_map(|c| galleys.get(r * 2 + c))
                .map(|g| g.size().y)
                .fold(0.0, f32::max)
        })
        .collect();
    let total: f32 = heights.iter().sum::<f32>() + 4.0 * (rows as f32 - 1.0).max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, total), Sense::hover());
    let p = ui.painter();
    let mut y = rect.top();
    for r in 0..rows {
        for c in 0..2 {
            let Some(g) = galleys.get(r * 2 + c) else {
                continue;
            };
            let x = rect.left() + c as f32 * (col_w + gap);
            // The check is a polyline, not a glyph: nothing can go .notdef.
            let tick = Stroke::new(1.4, green);
            let ty = y + 2.0;
            p.line_segment(
                [egui::pos2(x + 1.0, ty + 4.0), egui::pos2(x + 3.6, ty + 6.6)],
                tick,
            );
            p.line_segment(
                [egui::pos2(x + 3.6, ty + 6.6), egui::pos2(x + 8.6, ty + 1.0)],
                tick,
            );
            p.galley(egui::pos2(x + marker, y), g.clone(), t.text2);
        }
        y += heights[r] + 4.0;
    }
}

// ---------- pieces shared by the plugin cards ----------

/// The concept's success green (ticks, "Recommended", the active dot).
fn ok_green() -> Color32 {
    Color32::from_rgb(0x22, 0xc5, 0x5e)
}

/// Section heading (`.p-label`): 9 px semibold, uppercased, 7 px of air below.
fn sec_head(ui: &mut Ui, s: &str) {
    txt(ui, s.to_uppercase(), semi(9.0), tok().muted);
    ui.add_space(7.0);
}

/// The green "Recommended" chip (`.pill-rec`).
fn rec_pill(ui: &mut Ui, label: &str) {
    let green = ok_green();
    let g = ui
        .painter()
        .layout_no_wrap(label.to_uppercase(), semi(8.0), green);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 12.0, 14.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 7.0, wash(green, 40));
    p.galley(rect.center() - g.size() / 2.0, g, green);
}

/// Inline code chip (`code`): the palette's inset surface.
fn code_chip(ui: &mut Ui, s: &str) {
    let t = tok();
    let g = ui
        .painter()
        .layout_no_wrap(s.to_owned(), egui::FontId::monospace(9.0), t.text2);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 10.0, 16.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        4.0,
        t.input,
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - g.size() / 2.0, g, t.text2);
}

/// `.btn` with a leading Material icon. Sizes are measured with the resting
/// colour (identical metrics), so hover changes ink only, never layout.
fn btn_icon(ui: &mut Ui, glyph: &str, label: &str, primary: bool) -> Response {
    let t = tok();
    let fg0 = if primary { Color32::WHITE } else { t.text2 };
    let gap = 6.0;
    let (iw, tw) = {
        let p = ui.painter();
        let ig = p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), fg0);
        let tg = p.layout_no_wrap(label.to_owned(), reg(11.0), fg0);
        (ig.size().x, tg.size().x)
    };
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(iw + gap + tw + 28.0, 28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered();
    let fg = if !primary && hov { t.text } else { fg0 };
    let (bg, bd) = if primary {
        (if hov { t.accent_hi } else { t.accent }, t.accent)
    } else if hov {
        (t.btn_hover, t.btn_bd)
    } else {
        (t.btn, t.btn_bd)
    };
    let p = ui.painter();
    p.rect(rect, 6.0, bg, Stroke::new(1.0, bd), StrokeKind::Inside);
    let ig = p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), fg);
    let tg = p.layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let x = rect.center().x - (iw + gap + tw) / 2.0;
    p.galley(egui::pos2(x, rect.center().y - ig.size().y / 2.0), ig, fg);
    p.galley(
        egui::pos2(x + iw + gap, rect.center().y - tg.size().y / 2.0),
        tg,
        fg,
    );
    resp
}

/// `.btn-mini`: the compact button used inside the location options.
fn btn_mini(ui: &mut Ui, label: &str) -> Response {
    let t = tok();
    let w = {
        let g = ui
            .painter()
            .layout_no_wrap(label.to_owned(), reg(10.0), t.text2);
        g.size().x + 20.0
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered();
    let fg = if hov { t.text } else { t.text2 };
    let p = ui.painter();
    p.rect(
        rect,
        5.0,
        if hov { t.btn_hover } else { t.btn },
        Stroke::new(1.0, t.btn_bd),
        StrokeKind::Inside,
    );
    let g = p.layout_no_wrap(label.to_owned(), reg(10.0), fg);
    p.galley(rect.center() - g.size() / 2.0, g, fg);
    resp
}

/// The identity row every plugin card shares (`.plugin-head`): brand tile ·
/// name · version chip, then right-aligned meta and the open/closed chevron.
fn plugin_head(
    ui: &mut Ui,
    w: f32,
    glyph: &str,
    brand: Color32,
    name: &str,
    version: &str,
    meta: &str,
    open: bool,
) -> Response {
    let t = tok();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 58.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 11.0, wash(t.text, 8));
    }
    let tile = Rect::from_min_size(
        egui::pos2(rect.left() + 13.0, rect.center().y - 17.0),
        Vec2::splat(34.0),
    );
    p.rect_filled(tile, 9.0, wash(brand, 30));
    p.text(
        tile.center(),
        Align2::CENTER_CENTER,
        glyph,
        crate::fonts::icon(17.0),
        brand,
    );
    // name, with the version chip right after it
    let name_g = p.layout_no_wrap(name.to_owned(), semi(13.0), t.text);
    let nx = tile.right() + 11.0;
    p.galley(
        egui::pos2(nx, rect.center().y - name_g.size().y / 2.0),
        name_g.clone(),
        t.text,
    );
    let ver_g = p.layout_no_wrap(version.to_owned(), egui::FontId::monospace(9.0), t.accent);
    let chip = Rect::from_min_size(
        egui::pos2(nx + name_g.size().x + 8.0, rect.center().y - 9.0),
        Vec2::new(ver_g.size().x + 16.0, 18.0),
    );
    p.rect(
        chip,
        9.0,
        wash(t.accent, 30),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    p.galley(chip.center() - ver_g.size() / 2.0, ver_g, t.accent);
    // chevron at the far right, meta to its left
    p.text(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        Align2::CENTER_CENTER,
        if open { "\u{e5ce}" } else { "\u{e5cf}" }, // expand_less / expand_more
        crate::fonts::icon(16.0),
        t.muted,
    );
    let meta_g = p.layout_no_wrap(meta.to_owned(), reg(10.0), t.muted);
    p.galley(
        egui::pos2(
            rect.right() - 30.0 - meta_g.size().x,
            rect.center().y - meta_g.size().y / 2.0,
        ),
        meta_g,
        t.muted,
    );
    resp
}

/// One `.loc-opt` radio row: title (+ optional chip), a path/monospace sub-line
/// and an optional inline button. Returns `(row picked, button pressed)`.
#[allow(clippy::too_many_arguments)]
fn loc_option(
    ui: &mut Ui,
    selected: bool,
    title: &str,
    badge: Option<&str>,
    sub: &str,
    mono: bool,
    action: Option<&str>,
) -> (bool, bool) {
    let t = tok();
    let w = ui.available_width();
    let mut action_hit = false;
    let frame = egui::Frame::new()
        .fill(if selected { wash(t.accent, 26) } else { t.bg })
        .stroke(Stroke::new(1.0, if selected { t.accent } else { t.border }))
        .corner_radius(8.0)
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(w - 22.0);
            ui.spacing_mut().item_spacing.x = 9.0;
            ui.horizontal_top(|ui| {
                // Painted radio: the filled dot is the whole "active" signal.
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                let p = ui.painter();
                let c = egui::pos2(r.center().x, r.center().y + 1.0);
                p.circle_stroke(
                    c,
                    6.0,
                    Stroke::new(1.4, if selected { t.accent } else { t.btn_bd }),
                );
                if selected {
                    p.circle_filled(c, 3.2, t.accent);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        txt(ui, title, semi(11.0), t.text);
                        if let Some(b) = badge {
                            rec_pill(ui, b);
                        }
                    });
                    if mono {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(sub)
                                    .font(egui::FontId::monospace(9.0))
                                    .color(t.muted),
                            )
                            .selectable(false),
                        );
                    } else {
                        wrapped(ui, sub, reg(9.0), t.muted);
                    }
                });
                if let Some(label) = action {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if btn_mini(ui, label).clicked() {
                            action_hit = true;
                        }
                    });
                }
            });
        });
    let resp = frame
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    (resp.clicked(), action_hit)
}

/// Status line: a green (or amber) dot and what yt-dlp/ffmpeg will actually do.
fn loc_status(ui: &mut Ui, text: &str, warn: bool) {
    let t = tok();
    let col = if warn { pl().warning } else { ok_green() };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        let p = ui.painter();
        let c = r.center();
        p.circle_filled(c, 5.0, wash(col, 40)); // the concept's glow ring
        p.circle_filled(c, 3.0, col);
        txt(ui, text, reg(10.0), if warn { col } else { t.text2 });
    });
}

/// `.runtime-note`: "Runtime `node` — solves YouTube's player challenge",
/// right-aligned in the action row.
fn runtime_note(ui: &mut Ui, runtime: Option<&str>) {
    let t = tok();
    let (tail, warn) = runtime_tail(runtime);
    let fg = if warn { pl().warning } else { t.muted };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        txt(ui, tail, reg(9.0), fg);
        if let Some(r) = runtime {
            code_chip(ui, r);
        }
        txt(ui, "Runtime", reg(9.0), fg);
    });
}

/// Trailing words of the runtime note, plus whether they need the warning ink.
fn runtime_tail(runtime: Option<&str>) -> (&'static str, bool) {
    match runtime {
        Some(_) => ("— solves YouTube's player challenge", false),
        None => ("— none found; install Node.js", true),
    }
}

/// How much room the runtime note needs, so the actions row can keep it beside
/// the buttons or wrap it onto its own line instead of overlapping them.
fn runtime_note_width(ui: &Ui, runtime: Option<&str>) -> f32 {
    let (tail, _) = runtime_tail(runtime);
    let p = ui.painter();
    let word = p
        .layout_no_wrap("Runtime".to_owned(), reg(9.0), Color32::WHITE)
        .size()
        .x;
    let tail_w = p
        .layout_no_wrap(tail.to_owned(), reg(9.0), Color32::WHITE)
        .size()
        .x;
    // code_chip pads its monospace label by 10 px.
    let chip = runtime.map_or(0.0, |r| {
        p.layout_no_wrap(r.to_owned(), egui::FontId::monospace(9.0), Color32::WHITE)
            .size()
            .x
            + 10.0
    });
    word + chip + tail_w + 10.0 // the two 5 px gaps
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
        h2(ui, "Download Engine");
        hint(ui, "How downloads are saved, fetched, and finished.");
        ui.add_space(10.0);
        let s = &mut self.s;

        // ---- Storage ----
        card(ui, "Storage", None, &mut self.open_storage, |ui| {
            let mut c = field_row(ui, "save_dir", "Save folder", None, None);
            let iw = (c.available_width() - 34.0).max(60.0);
            input_box(
                &mut c,
                "engine_save_dir",
                &mut s.save_dir,
                iw,
                true,
                false,
                "C:\\Downloads",
            );
            c.add_space(6.0);
            if icon_sq(&mut c, "\u{e2c7}", 28.0, 6.0)
                .on_hover_text("Browse…")
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    s.save_dir = p.to_string_lossy().to_string();
                }
            }
            ui.add_space(6.0);
            checkbox(
                ui,
                &mut s.category_folders,
                "Sort into category subfolders",
                14.0,
                11.5,
                true,
            );
            ui.add_space(1.0);
            checkbox(
                ui,
                &mut s.show_complete,
                "Show the \u{201c}Download complete\u{201d} dialog",
                14.0,
                11.5,
                true,
            );
        });

        // ---- Performance ----
        card(ui, "Performance", None, &mut self.open_perf, |ui| {
            let mut c = field_row(ui, "connections", "Default connections", None, None);
            let mut conn = s.connections_default as f64;
            if slider_row(&mut c, "connections", &mut conn, 1.0, 16.0) {
                s.connections_default = conn.round() as usize;
            }
            let mut c = field_row(
                ui,
                "speed_limit",
                "Speed limit KB/s",
                None,
                Some("0 = unlimited"),
            );
            let mut lim = s.speed_limit_kbps as f64;
            if slider_row(&mut c, "speed_limit", &mut lim, 0.0, 10000.0) {
                s.speed_limit_kbps = lim.round() as u64;
            }
        });

        // ---- Network ----
        card(ui, "Network", None, &mut self.open_network, |ui| {
            let mut c = field_row(ui, "ua", "User-Agent", None, None);
            let w = c.available_width();
            input_box(
                &mut c,
                "engine_ua",
                &mut s.user_agent,
                w,
                true,
                false,
                "default (Download Craft)",
            );
            let mut c = field_row(ui, "proxy", "Proxy", None, None);
            let w = c.available_width();
            input_box(
                &mut c,
                "engine_proxy",
                &mut s.proxy_url,
                w,
                true,
                false,
                "http://host:port (empty = direct)",
            );
            let mut c = field_row(ui, "timeout", "Timeout", Some("sec"), None);
            let mut timeout = s.timeout_secs as f64;
            if slider_row(&mut c, "timeout", &mut timeout, 5.0, 600.0) {
                s.timeout_secs = timeout.round() as u64;
            }
            let mut c = field_row(ui, "retries", "Retries", None, None);
            let mut retries = s.max_retries as f64;
            if slider_row(&mut c, "retries", &mut retries, 1.0, 20.0) {
                s.max_retries = retries.round() as u32;
            }
        });

        // ---- On completion ----
        card(ui, "On completion", None, &mut self.open_complete, |ui| {
            let mut c = field_row(
                ui,
                "av",
                "Antivirus command",
                None,
                Some("{file} is replaced with the downloaded file path."),
            );
            let w = c.available_width();
            input_box(
                &mut c,
                "engine_av",
                &mut s.antivirus_cmd,
                w,
                true,
                false,
                "\"C:\\Program Files\\Defender\\MpCmdRun.exe\" -Scan -ScanType 3 -File {file}",
            );
            let mut c = field_row(ui, "finish", "When a download finishes", None, None);
            segmented(
                &mut c,
                "finish",
                &["nothing", "shutdown", "sleep", "hibernate"],
                &mut s.finish_action,
            );
        });

        // ---- Speed windows ----
        card(ui, "Speed windows", None, &mut self.open_windows, |ui| {
            checkbox(
                ui,
                &mut s.speed_windows_on,
                "Enable speed windows",
                14.0,
                11.5,
                true,
            );
            ui.add_space(4.0);
            hint(
                ui,
                "While a window is active its cap overrides the global limit (0 = full speed).",
            );
            ui.add_space(8.0);
            let mut del: Option<usize> = None;
            for (i, w) in s.speed_windows.iter_mut().enumerate().take(6) {
                if window_row(ui, i, w) {
                    del = Some(i);
                }
            }
            if let Some(i) = del {
                s.speed_windows.remove(i);
            }
            if btn_dashed(ui, "\u{e145}", "Add window").clicked() && s.speed_windows.len() < 6 {
                s.speed_windows.push(SpeedWindow {
                    start: "02:00".to_string(),
                    end: "06:00".to_string(),
                    limit_kbps: 0,
                });
            }
        });
    }

    // ---------- Plugins (helpers, one card each) ----------
    fn pane_plugins(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Plugins");
        hint(
            ui,
            "Each plugin is its own card. Name, version and date sit on one row.",
        );
        ui.add_space(12.0);

        self.plugin_card_ytdlp(ui);
        ui.add_space(10.0);
        self.plugin_card_ffmpeg(ui);
        ui.add_space(14.0);

        // `.empty-note`: what the catalogue grows into.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new("\u{e88e}") // info
                        .font(crate::fonts::icon(12.0))
                        .color(t.muted),
                )
                .selectable(false),
            );
            wrapped(
                ui,
                "More plugins will appear here as they are added. Each plugin gets its own card with info, actions, and options.",
                reg(9.0),
                t.muted,
            );
        });
    }

    /// yt-dlp: the card that starts open (`plugins_open[0]`).
    fn plugin_card_ytdlp(&mut self, ui: &mut Ui) {
        let t = tok();
        let info = self.ytdlp.clone();
        let msg = self.ytdlp_msg.clone();
        let busy = self.ytdlp_busy;
        let open = self.plugins_open[0];
        let source = self.s.ytdlp_source.clone();
        let custom = self.s.ytdlp_custom_path.clone();
        let managed_dir = crate::engine::ytdlp_managed_path();

        let pill = match info.as_ref().and_then(|i| i.version.as_deref()) {
            Some(v) => format!("v{v}"),
            None if busy => "checking…".to_string(),
            None => "unknown".to_string(),
        };
        let size = info
            .as_ref()
            .and_then(|i| i.size)
            .map(crate::model::format_bytes)
            .unwrap_or_else(|| "—".to_string());
        let meta = match (&info, busy) {
            (_, true) => "checking…".to_string(),
            (None, _) => "not checked yet".to_string(),
            (Some(i), _) if i.managed => format!("own copy · {size}"),
            (Some(_), _) => format!("from PATH · {size}"),
        };
        let custom_missing = !custom.trim().is_empty()
            && !info.as_ref().map(|i| i.custom_ok).unwrap_or(false);

        let w = ui.available_width();
        let mut want_open = open;
        let mut set_source: Option<&'static str> = None;
        let mut set_custom: Option<String> = None;
        let mut redownload = false;

        let edge = if open {
            blend(t.border, t.accent, 0.5)
        } else {
            t.border
        };
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let inner = ui.max_rect();
                let head = plugin_head(
                    ui,
                    inner.width(),
                    "\u{e24d}", // insert_drive_file
                    t.accent,
                    "yt-dlp",
                    &pill,
                    &meta,
                    open,
                );
                if head.clicked() {
                    want_open = !open;
                }
                if !open {
                    return;
                }
                // soft rule between the identity row and the body
                let y = ui.cursor().top();
                ui.painter().hline(
                    inner.left()..=inner.right(),
                    y,
                    Stroke::new(1.0, blend(t.card, t.border, 0.6)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 13,
                        right: 13,
                        top: 12,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(inner.width() - 26.0);

                        sec_head(ui, "Supported sites");
                        platform_grid(ui);
                        ui.add_space(13.0);

                        sec_head(ui, "Capabilities");
                        capabilities(ui, &YTDLP_CAPS);
                        ui.add_space(13.0);

                        sec_head(ui, "yt-dlp location");
                        let managed_sub = managed_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "your Windows app data folder".to_string());
                        let (hit, _) = loc_option(
                            ui,
                            source == "managed",
                            "App's own copy",
                            Some("Recommended"),
                            &managed_sub,
                            true,
                            None,
                        );
                        if hit {
                            set_source = Some("managed");
                        }
                        ui.add_space(6.0);

                        let path_sub = match info.as_ref().and_then(|i| i.on_path.as_deref()) {
                            Some(p) => p.display().to_string(),
                            None => "no yt-dlp found on PATH".to_string(),
                        };
                        let (hit, _) = loc_option(
                            ui,
                            source == "path",
                            "System PATH",
                            None,
                            &path_sub,
                            true,
                            None,
                        );
                        if hit {
                            set_source = Some("path");
                        }
                        ui.add_space(6.0);

                        let custom_sub = if custom.trim().is_empty() {
                            "pick any yt-dlp.exe on this PC".to_string()
                        } else if custom_missing {
                            format!("{custom}  — not there any more")
                        } else {
                            custom.clone()
                        };
                        let (hit, browse) = loc_option(
                            ui,
                            source == "custom",
                            "Custom path",
                            None,
                            &custom_sub,
                            true,
                            Some("Browse…"),
                        );
                        if browse {
                            // Picking the file *is* the choice: select it too.
                            if let Some(p) = pick_executable() {
                                set_custom = Some(p.to_string_lossy().to_string());
                                set_source = Some("custom");
                            }
                        } else if hit {
                            set_source = Some("custom");
                        }
                        ui.add_space(9.0);

                        // What a YouTube download would really run, right now.
                        let (status, warn) = match &info {
                            None => ("Checking what you have…".to_string(), false),
                            Some(i) => {
                                let from = if i.managed {
                                    "App's own copy"
                                } else if i.path.is_some() {
                                    "yt-dlp on PATH"
                                } else {
                                    "Nothing installed yet"
                                };
                                let what = match (&i.version, i.path.is_some()) {
                                    (Some(v), true) => format!(" — yt-dlp {v} detected"),
                                    (None, true) => " — it did not answer --version".to_string(),
                                    _ => " — the first YouTube download fetches it".to_string(),
                                };
                                (
                                    format!("Active: {from}{what}"),
                                    i.path.is_none() || i.stale || custom_missing,
                                )
                            }
                        };
                        loc_status(ui, &status, warn);
                        ui.add_space(13.0);

                        // Buttons left, runtime note pushed right — and dropped
                        // onto its own line when the two stop fitting, which is
                        // what the concept's `flex-wrap` does. The note is
                        // measured first because otherwise it ran straight over
                        // the last button in a narrow window.
                        let rt = info.as_ref().and_then(|i| i.js_runtime.as_deref());
                        let mut note_beside = false;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add_enabled_ui(!busy, |ui| {
                                // e5d5 renders as a refresh arrow in this build
                                // of Material Icons — which is what re-fetching
                                // the helper means anyway.
                                if btn_icon(ui, "\u{e5d5}", "Re-download", true).clicked() {
                                    redownload = true;
                                }
                            });
                            if btn_icon(ui, "\u{e2c7}", "Show in folder", false).clicked() {
                                if let Some(dir) = crate::engine::ytdlp_bin_dir() {
                                    let _ = std::fs::create_dir_all(&dir);
                                    crate::app::open_path(&dir, false);
                                }
                            }
                            if busy {
                                ui.add(egui::Spinner::new().size(14.0));
                            }
                            if ui.available_width() >= runtime_note_width(ui, rt) {
                                runtime_note(ui, rt);
                                note_beside = true;
                            }
                        });
                        if !note_beside {
                            ui.add_space(6.0);
                            runtime_note(ui, rt);
                        }

                        if !msg.is_empty() {
                            ui.add_space(7.0);
                            wrapped(ui, msg.as_str(), reg(10.0), t.accent);
                        }
                        if info.as_ref().map(|i| i.stale).unwrap_or(false) {
                            ui.add_space(4.0);
                            wrapped(
                                ui,
                                "This copy is past its 30-day refresh window — press Re-download.",
                                reg(9.0),
                                pl().warning,
                            );
                        }
                    });
            });

        self.plugins_open[0] = want_open;
        if let Some(src) = set_source {
            if set_custom.is_none() {
                self.s.ytdlp_source = src.to_string();
                // Keep the old switch truthful for anything still reading it,
                // then re-ask so the status line matches the new choice.
                self.s.ytdlp_prefer_path = src == "path";
                self.ytdlp_rx = Some(ytdlp_probe(false));
                self.ytdlp_busy = true;
                self.ytdlp_msg.clear();
            }
        }
        if let Some(p) = set_custom {
            self.s.ytdlp_custom_path = p;
            self.s.ytdlp_source = "custom".to_string();
            self.s.ytdlp_prefer_path = false;
            self.ytdlp_rx = Some(ytdlp_probe(false));
            self.ytdlp_busy = true;
            self.ytdlp_msg.clear();
        }
        if redownload {
            self.ytdlp_rx = Some(ytdlp_probe(true));
            self.ytdlp_busy = true;
            self.ytdlp_msg.clear();
        }
    }

    /// ffmpeg: the concept's second card, closed by default — with real
    /// detection behind it, since yt-dlp needs it for anything above 360p.
    fn plugin_card_ffmpeg(&mut self, ui: &mut Ui) {
        let t = tok();
        let ff = self.ffmpeg.clone();
        let open = self.plugins_open[1];
        let found = ff.as_ref().and_then(|f| f.path.clone());
        let pill = match ff.as_ref().and_then(|f| f.version.as_deref()) {
            Some(v) => format!("v{v}"),
            None => "missing".to_string(),
        };
        let meta = ff
            .as_ref()
            .map(|f| f.meta())
            .unwrap_or_else(|| "looking…".to_string());

        let w = ui.available_width();
        let mut want_open = open;
        let mut reveal: Option<std::path::PathBuf> = None;

        let edge = if open {
            blend(t.border, t.accent, 0.5)
        } else {
            t.border
        };
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let inner = ui.max_rect();
                let head = plugin_head(
                    ui,
                    inner.width(),
                    "\u{e02c}", // movie
                    ok_green(),
                    "ffmpeg",
                    &pill,
                    &meta,
                    open,
                );
                if head.clicked() {
                    want_open = !open;
                }
                if !open {
                    return;
                }
                let y = ui.cursor().top();
                ui.painter().hline(
                    inner.left()..=inner.right(),
                    y,
                    Stroke::new(1.0, blend(t.card, t.border, 0.6)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 13,
                        right: 13,
                        top: 12,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(inner.width() - 26.0);

                        sec_head(ui, "Capabilities");
                        capabilities(ui, &FFMPEG_CAPS);
                        ui.add_space(13.0);

                        sec_head(ui, "Windows build");
                        let (status, warn) = match (&ff, &found) {
                            (None, _) => ("Checking what you have…".to_string(), false),
                            (Some(_), Some(_)) => {
                                ("Active: the ffmpeg on your PATH".to_string(), false)
                            }
                            (Some(_), None) => (
                                "Not found — run \u{201c}winget install ffmpeg\u{201d}, then reopen this window"
                                    .to_string(),
                                true,
                            ),
                        };
                        loc_status(ui, &status, warn);
                        if let Some(p) = &found {
                            ui.add_space(5.0);
                            wrapped(
                                ui,
                                p.display().to_string(),
                                egui::FontId::monospace(9.0),
                                t.muted,
                            );
                        }
                        ui.add_space(13.0);

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add_enabled_ui(found.is_some(), |ui| {
                                if btn_icon(ui, "\u{e2c7}", "Show in folder", false).clicked() {
                                    reveal = found.clone();
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    txt(
                                        ui,
                                        "used only to merge video + audio",
                                        reg(9.0),
                                        t.muted,
                                    );
                                },
                            );
                        });
                    });
            });

        self.plugins_open[1] = want_open;
        if let Some(p) = reveal {
            crate::app::open_path(&p, true);
        }
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
