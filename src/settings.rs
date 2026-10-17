//! Standalone Settings window (`--settings`): a separate OS window with a
//! two-pane layout (rail + content), built with standard egui widgets.
//!
//! It edits the shared `ui.json` with read-modify-write merging (only its
//! own keys are overlaid, so concurrent main-app saves elsewhere survive).
//! The main app watches `ui.json` mtime and reloads — no restart needed.

use crate::model::{default_folder_rules, FolderRule, SpeedWindow};

const PANES: &[&str] = &[
    "Appearance",
    "Download Engine",
    "YouTube",
    "Browser Integration",
    "Categories & Rules",
    "Updates",
];

/// Rail index of the YouTube pane (the helper's status is probed when it is
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
            close_to_tray: false,
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
    };
    // Separate OS window (own process via `--settings`): never embedded in
    // the main GUI. Compact default size so it doesn't feel "too wide".
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download Craft Settings")
        .with_inner_size([600.0, 480.0])
        .with_min_inner_size([560.0, 440.0])
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let resolved = format!(
            "{}:{}",
            self.s.theme,
            crate::app::theme_is_dark(ctx, &self.s.theme)
        );
        if self.theme_applied != resolved {
            crate::app::apply_theme(ctx, &self.s.theme);
            self.theme_applied = resolved;
        }
        if !self.centered && crate::model::center_window(ctx, egui::vec2(600.0, 480.0)) {
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
        // Action bar as its own panel: the pane above it then gets exactly the
        // space that is left, and a short window scrolls instead of clipping
        // its last rows (the previous fixed-height scroll area cut off
        // whatever did not fit).
        egui::TopBottomPanel::bottom("settings_actions").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.notice = if save(&self.s) {
                        "Saved ✓ (main app picks it up live)".to_string()
                    } else {
                        "Could not write settings.".to_string()
                    };
                }
                if !self.notice.is_empty() {
                    ui.label(&self.notice);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
            ui.add_space(6.0);
        });
        egui::SidePanel::left("settings_rail")
            .exact_width(148.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                for (i, name) in PANES.iter().enumerate() {
                    if ui.selectable_label(self.pane == i, *name).clicked() {
                        self.pane = i;
                    }
                }
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.pane {
                    0 => self.pane_appearance(ui),
                    1 => self.pane_engine(ui),
                    PANE_YTDLP => self.pane_ytdlp(ui),
                    3 => self.pane_browser(ui),
                    4 => self.pane_rules(ui),
                    _ => self.pane_updates(ui),
                });
        });
    }
}

impl SettingsApp {
    fn pane_appearance(&mut self, ui: &mut egui::Ui) {
        ui.heading("Appearance");
        ui.horizontal(|ui| {
            ui.label("Theme:");
            for (name, label) in [("system", "System"), ("dark", "Dark"), ("light", "Light")] {
                if ui.selectable_label(self.s.theme == name, label).clicked() {
                    self.s.theme = name.to_string();
                }
            }
        });
        ui.checkbox(&mut self.s.close_to_tray, "Minimize to system tray instead of quitting");
        ui.small("Fonts, radius, and accent stay identical — only the palette changes.");
    }

    fn pane_engine(&mut self, ui: &mut egui::Ui) {
        ui.heading("Download Engine");
        ui.horizontal(|ui| {
            ui.label("Save folder:");
            ui.add(egui::TextEdit::singleline(&mut self.s.save_dir).desired_width(220.0));
            if ui.button("…").on_hover_text("Browse").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.s.save_dir = p.to_string_lossy().to_string();
                }
            }
        });
        ui.checkbox(&mut self.s.category_folders, "Sort into category subfolders");
        ui.checkbox(
            &mut self.s.show_complete,
            "Show the \u{201c}Download complete\u{201d} dialog",
        );
        ui.horizontal(|ui| {
            ui.label("Default connections:");
            ui.add(egui::Slider::new(&mut self.s.connections_default, 1..=16));
        });
        ui.horizontal(|ui| {
            ui.label("Speed limit KB/s (0 = unlimited):");
            let mut v = self.s.speed_limit_kbps as f64;
            if ui.add(egui::Slider::new(&mut v, 0.0..=20000.0).logarithmic(true)).changed() {
                self.s.speed_limit_kbps = v as u64;
            }
        });
        ui.collapsing("Network", |ui| {
            ui.horizontal(|ui| {
                ui.label("User-Agent:");
                ui.add(egui::TextEdit::singleline(&mut self.s.user_agent).desired_width(220.0));
            });
            ui.horizontal(|ui| {
                ui.label("Proxy:");
                ui.add(egui::TextEdit::singleline(&mut self.s.proxy_url).desired_width(220.0));
            });
            ui.horizontal(|ui| {
                ui.label("Timeout (s):");
                ui.add(egui::Slider::new(&mut self.s.timeout_secs, 5..=600));
            });
            ui.horizontal(|ui| {
                ui.label("Retries:");
                ui.add(egui::Slider::new(&mut self.s.max_retries, 1..=20));
            });
        });
        ui.collapsing("On completion", |ui| {
            ui.horizontal(|ui| {
                ui.label("Antivirus ({file} = path):");
                ui.add(egui::TextEdit::singleline(&mut self.s.antivirus_cmd).desired_width(200.0));
            });
            ui.horizontal(|ui| {
                ui.label("Finish action:");
                for name in ["nothing", "shutdown", "sleep", "hibernate"] {
                    if ui.selectable_label(self.s.finish_action == name, name).clicked() {
                        self.s.finish_action = name.to_string();
                    }
                }
            });
        });
        ui.collapsing("Speed windows", |ui| {
            ui.checkbox(&mut self.s.speed_windows_on, "Enable speed windows");
            ui.small("While a window is active its cap overrides the global limit (0 = full speed).");
            let mut del: Option<usize> = None;
            for (i, w) in self.s.speed_windows.iter_mut().enumerate().take(6) {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut w.start).desired_width(60.0));
                    ui.label("–");
                    ui.add(egui::TextEdit::singleline(&mut w.end).desired_width(60.0));
                    let mut lim = w.limit_kbps.to_string();
                    ui.add(egui::TextEdit::singleline(&mut lim).desired_width(80.0));
                    if let Ok(n) = lim.trim().parse::<u64>() {
                        w.limit_kbps = n;
                    }
                    ui.label("KB/s");
                    if ui.button("×").clicked() {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                self.s.speed_windows.remove(i);
            }
            if ui.button("Add window").clicked() && self.s.speed_windows.len() < 6 {
                self.s.speed_windows.push(SpeedWindow {
                    start: "02:00".to_string(),
                    end: "06:00".to_string(),
                    limit_kbps: 0,
                });
            }
        });
    }

    /// YouTube: the helper those links are handed to, where it comes from, and
    /// how to refresh it. The engine fetches and updates this copy itself, so
    /// there was no way to see it from inside the app before.
    fn pane_ytdlp(&mut self, ui: &mut egui::Ui) {
        ui.heading("YouTube");
        ui.small(
            "YouTube links are handed to yt-dlp: it speaks the streaming protocol browsers \
             use (what gates 1080p and up) and merges the audio leg itself.",
        );
        ui.add_space(8.0);
        match self.ytdlp.clone() {
            None => {
                ui.label(if self.ytdlp_busy {
                    "Checking the helper…"
                } else {
                    "Not checked yet."
                });
            }
            Some(info) => {
                let ver = info
                    .version
                    .clone()
                    .unwrap_or_else(|| "version unknown".to_string());
                match (&info.path, info.managed) {
                    (None, _) => {
                        ui.label("No yt-dlp present yet.");
                        ui.small(
                            "The first YouTube download fetches it automatically — there is \
                             nothing to install by hand.",
                        );
                    }
                    (Some(p), true) => {
                        ui.label(format!("Using this app's own yt-dlp — {ver}."));
                        ui.small(p.display().to_string());
                    }
                    (Some(p), false) => {
                        ui.label(format!("Using the yt-dlp installed on your PATH — {ver}."));
                        ui.small(p.display().to_string());
                    }
                }
                if let Some(sz) = info.size {
                    ui.small(format!("{} on disk.", crate::model::format_bytes(sz)));
                }
                if info.stale {
                    ui.small(
                        "Past the 30-day refresh window — YouTube's extractors change \
                         often: press Re-download.",
                    );
                }
                if info.managed && info.on_path.is_some() {
                    ui.small(
                        "Your PATH copy is ignored right now. Tick the box below to use it \
                         instead.",
                    );
                }
                ui.small(match &info.js_runtime {
                    Some(r) => format!(
                        "JavaScript runtime: {r} — solves YouTube's player challenge."
                    ),
                    None => "No JavaScript runtime found. Install Node.js if YouTube \
                             downloads stop at \"Getting info\" or fail with \"The page \
                             needs to be reloaded\"."
                        .to_string(),
                });
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.ytdlp_busy, egui::Button::new("Re-download"))
                .on_hover_text("Fetch the newest yt-dlp build now")
                .clicked()
            {
                self.ytdlp_rx = Some(ytdlp_probe(true));
                self.ytdlp_busy = true;
                self.ytdlp_msg.clear();
            }
            if ui
                .button("Show in folder")
                .on_hover_text("Open the folder the app keeps it in")
                .clicked()
            {
                if let Some(dir) = crate::engine::ytdlp_bin_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    crate::app::open_path(&dir, false);
                }
            }
            if self.ytdlp_busy {
                ui.spinner();
            }
        });
        if !self.ytdlp_msg.is_empty() {
            ui.small(&self.ytdlp_msg);
        }
        ui.add_space(6.0);
        ui.checkbox(
            &mut self.s.ytdlp_prefer_path,
            "Prefer a yt-dlp already installed on my PATH",
        );
        ui.small(
            "Unchecked: always use the copy this app fetches and keeps current. Applies to \
             the next download (press Save).",
        );
    }

    fn pane_browser(&mut self, ui: &mut egui::Ui) {
        ui.heading("Browser Integration");
        ui.small("Load browser-extension/ unpacked once — its ID is pinned, no pasting needed.");
        ui.small("One-time setup: host auto-repairs on every launch — buttons below are manual repair only.");
        ui.small(format!("Extension ID: {}", crate::app::EXTENSION_ID));
        ui.horizontal_wrapped(|ui| {
            for b in ["chrome", "edge", "firefox", "brave", "opera"] {
                if ui.button(format!("Install {b}")).clicked() {
                    self.browser_msg = install_one(b);
                }
            }
        });
        if !self.browser_msg.is_empty() {
            ui.small(&self.browser_msg);
        }
    }

    fn pane_rules(&mut self, ui: &mut egui::Ui) {
        ui.heading("Categories & Rules");
        ui.horizontal(|ui| {
            ui.label("Rename template:");
            ui.add(egui::TextEdit::singleline(&mut self.s.rename_template).desired_width(220.0));
        });
        ui.small("Tokens: {name} {ext} {date} {host}");
        ui.small(format!(
            "Preview: {}",
            crate::model::apply_rename_template(&self.s.rename_template, "My Video.mp4", "https://cdn.example/v")
        ));
        ui.separator();
        ui.label("Type → folder rules (first match wins, max 8):");
        let mut del: Option<usize> = None;
        let mut up: Option<usize> = None;
        let mut down: Option<usize> = None;
        for (i, rule) in self.s.folder_rules.iter_mut().enumerate().take(8) {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut rule.exts).hint_text("mp4,mkv").desired_width(140.0));
                ui.label("→");
                ui.add(egui::TextEdit::singleline(&mut rule.folder).hint_text("Video").desired_width(140.0));
                if ui.button("▲").clicked() {
                    up = Some(i);
                }
                if ui.button("▼").clicked() {
                    down = Some(i);
                }
                if ui.button("×").clicked() {
                    del = Some(i);
                }
            });
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
        ui.horizontal(|ui| {
            if ui.button("Add rule").clicked() && self.s.folder_rules.len() < 8 {
                self.s.folder_rules.push(FolderRule { exts: String::new(), folder: String::new() });
            }
            if ui.button("Reset defaults").clicked() {
                self.s.folder_rules = default_folder_rules();
            }
        });
    }

    fn pane_updates(&mut self, ui: &mut egui::Ui) {
        ui.heading("Updates");
        ui.checkbox(&mut self.s.update_check_on, "Check for updates on launch");
        ui.small("Checks GitHub releases; never downloads automatically.");
    }
}
