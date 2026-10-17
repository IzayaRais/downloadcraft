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
    "Browser Integration",
    "Categories & Rules",
    "Updates",
];

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
            cc.egui_ctx.set_visuals(crate::app::dark_visuals());
            crate::app::apply_widget_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.centered && crate::model::center_window(ctx, egui::vec2(600.0, 480.0)) {
            self.centered = true;
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Rail.
                ui.vertical(|ui| {
                    ui.set_width(140.0);
                    ui.add_space(4.0);
                    for (i, name) in PANES.iter().enumerate() {
                        if ui.selectable_label(self.pane == i, *name).clicked() {
                            self.pane = i;
                        }
                    }
                });
                ui.separator();
                // Content.
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .max_height(370.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| match self.pane {
                            0 => self.pane_appearance(ui),
                            1 => self.pane_engine(ui),
                            2 => self.pane_browser(ui),
                            3 => self.pane_rules(ui),
                            _ => self.pane_updates(ui),
                        });
                });
            });
            ui.separator();
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
        });
    }
}

impl SettingsApp {
    fn pane_appearance(&mut self, ui: &mut egui::Ui) {
        ui.heading("Appearance");
        ui.horizontal(|ui| {
            ui.label("Theme:");
            for name in ["dark", "black", "light"] {
                if ui.selectable_label(self.s.theme == name, name).clicked() {
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
