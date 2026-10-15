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

fn queue_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("queue.json"))
}

fn wake_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("wake.json"))
}

fn ui_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("ui.json"))
}

fn read_queue() -> Vec<DownloadItem> {
    let _guard = crate::model::fs_lock("queue");
    queue_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
    notice: String,
    already_queued: bool,
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
        let opts = EngineOpts {
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", env!("CARGO_PKG_VERSION"))
            } else {
                self.user_agent.clone()
            },
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            referer,
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
                        if let Some((media_url, title)) =
                            engine::resolve_media(&client, &opts, &url, &shared).await
                        {
                            let (t2, _, _, _) =
                                engine::probe(&client, &opts, &media_url).await;
                            if total.is_none() {
                                total = t2;
                            }
                            if disp.is_none() && !title.trim().is_empty() {
                                disp = Some(engine::sanitize_filename(&title));
                            }
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
        if self.pair_url.starts_with("http") {
            item.pair_url = self.pair_url.clone();
            item.pair_mime = self.pair_mime.clone();
        }
        let id = match crate::model::append_to_queue(&mut item) {
            Some(id) => id,
            None => {
                self.notice = "Already in queue.".to_string();
                self.done = true;
                return;
            }
        };
        save_remember_category(self.remember_path);
        if start {
            // Tell the main app to start this id (consumed on next poll /
            // at its startup when we launch it below).
            if let Some(p) = wake_path() {
                let _guard = crate::model::fs_lock("wake");
                let mut ids: Vec<u64> = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v.get("start").cloned())
                    .and_then(|v| serde_json::from_value::<Vec<u64>>(v).ok())
                    .unwrap_or_default();
                if !ids.contains(&id) {
                    ids.push(id);
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
        self.done = true;
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
    let filename = get("--filename")
        .filter(|f| !f.trim().is_empty())
        .unwrap_or_else(|| filename_from_url(&url));
    let referer = get("--referer").unwrap_or_default();
    let cookie = get("--cookie").unwrap_or_default();
    let pair_url = get("--pair-url").filter(|s| s.starts_with("http")).unwrap_or_default();
    let pair_mime = get("--pair-mime").unwrap_or_default();
    // Browser-measured size (menu HEAD/Range): trust it, skip probing.
    let known_size: Option<u64> = get("--size")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0);

    // Claim the inbox entry first so the main window won't prompt too.
    claim_inbox(url.trim());

    let prefs = load_prefs();
    let category = category_of(&filename).to_string();
    let already_queued = read_queue().iter().any(|d| d.url == url.trim());

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
        notice: String::new(),
        already_queued,
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
        .with_inner_size([620.0, 500.0])
        .with_min_inner_size([560.0, 440.0])
        .with_always_on_top()
        .with_resizable(true);
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
            crate::app::setup_fonts(cc);
            cc.egui_ctx.set_visuals(crate::app::dark_visuals());
            // ABDM-style: big rounded fields + pill buttons.
            crate::app::apply_widget_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for ConfirmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // Center once on the primary monitor (Windows otherwise cascades).
        if !self.centered {
            self.centered = true;
            let mon = ctx.input(|i| i.viewport().monitor_size);
            if let Some(m) = mon {
                let (w, h) = (620.0, 500.0);
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
                    // URL guess.
                    if self.filename == filename_from_url(&self.url) {
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
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            if self.already_queued {
                ui.label("This URL is already in the queue.");
                ui.add_space(6.0);
                if ui.button("Close").clicked() {
                    self.done = true;
                }
                return;
            }

            // Row 1: URL + copy.
            ui.horizontal(|ui| {
                ui.add_sized(
                    [ui.available_width() - 52.0, 38.0],
                    egui::TextEdit::singleline(&mut self.url).hint_text("https://..."),
                );
                if ui
                    .add_sized([44.0, 38.0], egui::Button::new(crate::app::MI_COPY))
                    .on_hover_text("Copy URL")
                    .clicked()
                {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(self.url.clone());
                    }
                }
            });
            // Row 2: category + remember + size badge (ABDM keeps size here).
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("cf_cat")
                    .selected_text(&self.category)
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for c in CATEGORIES {
                            ui.selectable_value(&mut self.category, c.to_string(), *c);
                        }
                    });
                ui.checkbox(
                    &mut self.remember_path,
                    format!("Remember for \"{}\"", self.category),
                );
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if self.probing {
                            ui.spinner();
                        } else if let Some(sz) = self.probe_size.filter(|s| *s > 0) {
                            ui.label(egui::RichText::new(format_bytes(sz)).size(14.0).strong());
                            ui.colored_label(
                                egui::Color32::from_rgb(80, 200, 120),
                                egui::RichText::new(crate::app::MI_CHECK).size(14.0),
                            );
                        } else {
                            ui.weak("size unknown");
                        }
                    },
                );
            });
            // Row 3: save folder + browse.
            ui.horizontal(|ui| {
                ui.add_sized(
                    [ui.available_width() - 52.0, 38.0],
                    egui::TextEdit::singleline(&mut self.save_base),
                );
                if ui
                    .add_sized([44.0, 38.0], egui::Button::new(crate::app::MI_FOLDER_OPEN))
                    .on_hover_text("Browse folder")
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.save_base = p.to_string_lossy().to_string();
                    }
                }
            });
            // Row 4: filename + re-probe + advanced gear.
            ui.horizontal(|ui| {
                ui.add_sized(
                    [ui.available_width() - 104.0, 38.0],
                    egui::TextEdit::singleline(&mut self.filename).hint_text("Filename"),
                );
                if ui
                    .add_sized([44.0, 38.0], egui::Button::new(crate::app::MI_REFRESH))
                    .on_hover_text("Re-check name and size")
                    .clicked()
                {
                    self.probe_size = None;
                    self.start_probe();
                }
                if ui
                    .add_sized([44.0, 38.0], egui::Button::new(crate::app::MI_SETTINGS))
                    .on_hover_text("Advanced")
                    .clicked()
                {
                    self.show_adv = !self.show_adv;
                }
            });
            ui.weak(format!("→ {}", self.effective_dir()));
            if self.pair_url.starts_with("http") {
                ui.weak("video-only + audio — merged to one file on completion (needs ffmpeg)");
            }
            if self.show_adv {
                egui::Frame::new()
                    .corner_radius(crate::app::RADIUS)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Referer");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.referer)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Cookie");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.cookie)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Description");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.description)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("optional note"),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(format!("Connections: {}", self.connections));
                            ui.add(
                                egui::Slider::new(&mut self.connections, 1..=16).show_value(false),
                            );
                        });
                    });
            } else {
                // Description stays one line under, like IDM.
                ui.horizontal(|ui| {
                    ui.label("Description");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.description)
                            .desired_width(f32::INFINITY)
                            .hint_text("optional note"),
                    );
                });
            }
            if !self.notice.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(255, 150, 120), &self.notice);
            }
            ui.add_space(6.0);
            // Action row: Add | Download (accent) ... Cancel.
            ui.horizontal(|ui| {
                if ui.add_sized([120.0, 42.0], egui::Button::new("Add")).clicked() {
                    self.commit(false);
                }
                let dl = egui::Button::new(egui::RichText::new("Download").strong().color(egui::Color32::WHITE))
                    .fill(egui::Color32::from_rgb(88, 101, 242));
                if ui.add_sized([150.0, 42.0], dl).clicked() {
                    self.commit(true);
                }
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.add_sized([120.0, 42.0], egui::Button::new("Cancel")).clicked() {
                            self.done = true;
                        }
                    },
                );
            });
        });
    }
}
