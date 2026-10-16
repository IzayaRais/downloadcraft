//! IDM-style standalone per-download progress window.
//!
//! Launched as a separate always-on-top OS window when a download starts
//! from a confirm dialog or the main Add dialog:
//!   `download_craft.exe --progress <id>`
//!
//! The main app (which owns the engine tasks) publishes live numbers to
//! `progress.json` ~1/s; this window polls that file + `queue.json` and
//! renders Name/Status/Size/Downloaded/Speed/ETA + per-part table.
//! Pause/Resume/Cancel go back through `cmd.json`, polled by the main app.
//! Closing this window never stops the download (like IDM).

use crate::model::{config_dir, format_bytes, DownloadItem};
use std::time::{Duration, Instant};

fn queue_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("queue.json"))
}

fn progress_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("progress.json"))
}

fn cmd_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("cmd.json"))
}

fn read_queue() -> Vec<DownloadItem> {
    queue_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[derive(Debug, Default, Clone)]
struct Snap {
    downloaded: u64,
    total: Option<u64>,
    speed: f64,
    resumable: bool,
    live: bool,
    parts: Vec<(u64, u64, u64)>, // (start, end, done)
}

fn read_snap(id: u64) -> Snap {
    let mut out = Snap::default();
    let data = progress_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    if data.is_empty() {
        return out;
    }
    let v: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
    let e = match v.get("items").and_then(|m| m.get(&id.to_string())) {
        Some(e) => e,
        None => return out,
    };
    out.live = true;
    out.downloaded = e.get("downloaded").and_then(|x| x.as_u64()).unwrap_or(0);
    out.total = e.get("total").and_then(|x| x.as_u64());
    out.speed = e.get("speed").and_then(|x| x.as_f64()).unwrap_or(0.0);
    out.resumable = e.get("resumable").and_then(|x| x.as_bool()).unwrap_or(false);
    if let Some(arr) = e.get("parts").and_then(|x| x.as_array()) {
        for p in arr.iter().take(32) {
            let n = |i: usize| p.get(i).and_then(|x| x.as_u64()).unwrap_or(0);
            out.parts.push((n(0), n(1), n(2)));
        }
    }
    out
}

fn read_ui_prefs() -> (String, String) {
    let v: serde_json::Value = config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let lim = v
        .get("speed_limit_kbps")
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
        .to_string();
    let fin = v
        .get("finish_action")
        .and_then(|x| x.as_str())
        .unwrap_or("nothing")
        .to_string();
    (lim, fin)
}

/// Command with a payload (`limit` kbps, `finish` action).
fn send_cmd_kv(action: &str, k: &str, v: serde_json::Value) {
    let Some(p) = cmd_path() else { return };
    let _guard = crate::model::fs_lock("cmd");
    let mut val: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = val.as_array_mut() {
        let mut obj = serde_json::Map::new();
        obj.insert("action".to_string(), serde_json::Value::String(action.to_string()));
        obj.insert(k.to_string(), v);
        arr.push(serde_json::Value::Object(obj));
        if let Ok(json) = serde_json::to_string_pretty(&val) {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}
fn send_cmd(id: u64, action: &str) {
    let Some(p) = cmd_path() else { return };
    let _guard = crate::model::fs_lock("cmd");
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = v.as_array_mut() {
        arr.retain(|e| !(e.get("id").and_then(|x| x.as_u64()) == Some(id)));
        arr.push(serde_json::json!({ "id": id, "action": action }));
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

/// Open a file/folder with the OS default handler on any platform.
fn open_path(p: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(p).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(p).spawn();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

fn fmt_eta(left: u64, speed: f64) -> String {    if speed <= 1.0 {
        return "-".to_string();
    }
    let secs = (left as f64 / speed) as u64;
    if secs >= 3600 {
        format!("{} h {} m left", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} m {} s left", secs / 60, secs % 60)
    } else {
        format!("{} s left", secs)
    }
}

struct ProgressApp {
    id: u64,
    centered: bool,
    last_poll: Instant,
    item: Option<DownloadItem>,
    snap: Snap,
    tab: u8, // 0 info, 1 speed, 2 on-completion
    limit_input: String,
    limit_known: Option<u64>,
    finish_sel: String,
}

impl ProgressApp {
    fn refresh(&mut self) {
        let items = read_queue();
        self.item = items.into_iter().find(|d| d.id == self.id);
        self.snap = read_snap(self.id);
        // Queue file lags live counters: prefer the snapshot while active.
        // Keep displayed progress monotonic across retry legs (see app).
        if self.snap.live {
            if let Some(it) = self.item.as_mut() {
                let new_total = self.snap.total;
                if new_total != it.total_bytes {
                    it.total_bytes = new_total;
                    it.downloaded_bytes = self.snap.downloaded;
                } else {
                    it.downloaded_bytes =
                        it.downloaded_bytes.max(self.snap.downloaded);
                }
                it.speed_bps = self.snap.speed;
            }
        }
        self.last_poll = Instant::now();
    }

    /// Close bulletproofing: viewport Close is async and has been observed
    /// to no-op; back it with a delayed process exit (harmless if the
    /// window already closed).
    fn close_soon(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            std::process::exit(0);
        });
    }

    /// Single-line middle-truncated filename (`AQPu4kS…-fq.mp4`); full name
    /// rides along as hover text at the call site.
    fn short_name(name: &str) -> String {
        const MAX: usize = 44;
        let chars: Vec<char> = name.chars().collect();
        if chars.len() <= MAX {
            return name.to_string();
        }
        let head: String = chars[..20].iter().collect();
        let tail: String = chars[chars.len() - 20..].iter().collect();
        format!("{head}…{tail}")
    }
}

/// Entry point for `--progress <id>`.
pub fn run() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let id: u64 = args
        .iter()
        .position(|a| a == "--progress")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("--progress needs <id>");
            std::process::exit(2);
        });
    // Seed controls from shared settings (global limit + finish action).
    let (limit_input, finish_sel) = read_ui_prefs();
    let mut app = ProgressApp {
        id,
        centered: false,
        last_poll: Instant::now() - Duration::from_secs(10),
        item: None,
        snap: Snap::default(),
        tab: 0,
        limit_input,
        limit_known: None,
        finish_sel,
    };
    app.refresh();
    let title = app
        .item
        .as_ref()
        .map(|d| d.filename.clone())
        .unwrap_or_else(|| format!("Download #{id}"));

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([420.0, 320.0])
        .with_min_inner_size([400.0, 300.0])
        .with_always_on_top()
        .with_resizable(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Download Craft",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| {
            crate::app::setup_fonts(cc);
            cc.egui_ctx.set_visuals(crate::app::dark_visuals());
            crate::app::apply_widget_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for ProgressApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.centered {
            self.centered = true;
            let mon = ctx.input(|i| i.viewport().monitor_size);
            if let Some(m) = mon {
                let (w, h) = (420.0, 320.0);
                if m.x > w && m.y > h {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                        egui::Pos2::new((m.x - w) / 2.0, (m.y - h) / 2.0),
                    ));
                }
            }
        }
        if self.last_poll.elapsed() >= Duration::from_millis(500) {
            self.refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(500));

        let green = egui::Color32::from_rgb(80, 200, 120);
        let Some(item) = self.item.clone() else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading(format!("Download #{}", self.id));
                ui.label("No longer in the queue.");
                if ui.button("Close").clicked() {
                    Self::close_soon(ctx);
                }
            });
            return;
        };
        let downloading = self.snap.live
            && matches!(
                item.status,
                crate::model::Status::Downloading | crate::model::Status::Queued
            );
        let paused = matches!(item.status, crate::model::Status::Paused);
        let done = matches!(item.status, crate::model::Status::Completed);
        let failed = matches!(item.status, crate::model::Status::Failed(_));

        // Pinned action row: reserved by the OS layout pass, so the buttons
        // can never be scrolled or clipped out of the window.
        egui::TopBottomPanel::bottom("progress_actions").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if done {
                    if ui.add_sized([110.0, 32.0], egui::Button::new("Open file")).clicked() {
                        open_path(&item.dest_path());
                    }
                    if ui.add_sized([110.0, 32.0], egui::Button::new("Open folder")).clicked() {
                        if let Some(dir) = item.dest_path().parent() {
                            open_path(dir);
                        }
                    }
                } else if !failed {
                    let label = if paused {
                        "Resume".to_string()
                    } else {
                        format!("{} Pause", crate::app::MI_PAUSE)
                    };
                    if ui.add_sized([110.0, 32.0], egui::Button::new(label)).clicked() {
                        send_cmd(self.id, if paused { "resume" } else { "pause" });
                    }
                    if ui.add_sized([110.0, 32.0], egui::Button::new("Cancel")).clicked() {
                        send_cmd(self.id, "cancel");
                    }
                }
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.add_sized([110.0, 32.0], egui::Button::new("Close")).clicked() {
                            // Finished/failed: closes immediately. Active:
                            // hides this window only — the download keeps
                            // running in the main app (never cancelled here).
                            Self::close_soon(ctx);
                        }
                    },
                );
            });
            if failed {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 150, 120),
                    item.status.label(),
                );
            }
            ui.add_space(2.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(Self::short_name(&item.filename))
                .on_hover_text(item.filename.clone());
            // Tab bar: Info | Speed | On Completion.
            ui.horizontal(|ui| {
                for (i, name) in [(0u8, "Info"), (1, "Speed"), (2, "On Completion")] {
                    let sel = self.tab == i;
                    if ui
                        .selectable_label(sel, name)
                        .on_hover_text(match i {
                            0 => "Progress and parts",
                            1 => "Live speed cap (applies instantly)",
                            _ => "What happens when the queue drains",
                        })
                        .clicked()
                    {
                        self.tab = i;
                    }
                }
            });
            ui.separator();
            // Scrollable content with a pinned action row below: short
            // windows scroll instead of clipping buttons off-screen.
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.tab == 1 {
                // ---- Speed tab: live global cap (main app applies via the
                // shared atomic, no restart). Thread count is fixed at start:
                // changing it mid-transfer would discard part files, so it
                // is shown read-only with that explanation.
                ui.label(format!("This download runs {} thread(s) (fixed at start).", item.connections));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Global cap KB/s (0 = unlimited):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.limit_input)
                            .desired_width(90.0),
                    );
                    if ui.button("Apply").clicked() {
                        match self.limit_input.trim().parse::<u64>() {
                            Ok(k) => {
                                send_cmd_kv("limit", "kbps", serde_json::Value::from(k));
                                self.limit_known = Some(k);
                            }
                            Err(_) => {
                                self.limit_known = None;
                            }
                        }
                    }
                });
                ui.small(match self.limit_known {
                    Some(k) => format!("Sent cap {k} KB/s — main app applies it live."),
                    None => "Type a number and Apply.".to_string(),
                });
                ui.small("Tip: Settings → Speed windows automates caps by time of day.");
            } else if self.tab == 2 {
                // ---- On Completion tab.
                ui.label("When the queue drains:");
                for name in ["nothing", "shutdown", "sleep", "hibernate"] {
                    if ui
                        .selectable_label(self.finish_sel == name, name)
                        .clicked()
                    {
                        self.finish_sel = name.to_string();
                        send_cmd_kv("finish", "value", serde_json::Value::from(name));
                    }
                }
                ui.add_space(4.0);
                if ui.button("Open download folder").clicked() {
                    if let Some(dir) = item.dest_path().parent() {
                        open_path(dir);
                    }
                }
            } else {
            egui::Frame::new()
                .corner_radius(crate::app::RADIUS)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, k: &str, v: String| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(k).weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(v);
                                },
                            );
                        });
                    };
                    row(ui, "Status:", item.status.label());
                    row(
                        ui,
                        "Size:",
                        item.total_bytes.map(format_bytes).unwrap_or_else(|| "-".into()),
                    );
                    row(ui, "Downloaded:", format_bytes(item.downloaded_bytes));
                    row(
                        ui,
                        "Speed:",
                        if downloading && item.speed_bps > 0.0 {
                            format!("{}/s", format_bytes(item.speed_bps as u64))
                        } else {
                            "-".into()
                        },
                    );
                    row(
                        ui,
                        "Remaining Time:",
                        match item.total_bytes {
                            Some(t) => fmt_eta(
                                t.saturating_sub(item.downloaded_bytes),
                                item.speed_bps,
                            ),
                            None => "-".into(),
                        },
                    );
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Resume Support:").weak());
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let r = item.resumable || self.snap.resumable;
                                ui.colored_label(
                                    if r { green } else { egui::Color32::GRAY },
                                    if r { "Yes" } else { "No" },
                                );
                            },
                        );
                    });
                });

            // Progress bar.
            let frac = match item.total_bytes {
                Some(t) if t > 0 => {
                    (item.downloaded_bytes as f32 / t as f32).clamp(0.0, 1.0)
                }
                _ => 0.0,
            };
            ui.add_space(4.0);
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_height(14.0)
                    .corner_radius(7)
                    .show_percentage(),
            );

            // Clean segment progress: a single unified chunk map. No per-part
            // text percentages underneath (the bar itself is sufficient).
            if !self.snap.parts.is_empty() {
                ui.add_space(4.0);
                let n = self.snap.parts.len();
                // Unified chunk map: one cell per part, filled by fraction.
                let bar_w = ui.available_width();
                let (rrect, _) = ui.allocate_exact_size(
                    egui::vec2(bar_w, 12.0),
                    egui::Sense::hover(),
                );
                let p = ui.painter();
                p.rect_filled(rrect, 6.0, egui::Color32::from_rgb(30, 31, 42));
                let gap = 2.0;
                let cw = (bar_w - gap * (n as f32 - 1.0).max(0.0)) / n as f32;
                for (i, (s, e, d)) in self.snap.parts.iter().enumerate() {
                    let len = e.saturating_sub(*s) + 1;
                    let f = if len > 0 { (*d as f32 / len as f32).clamp(0.0, 1.0) } else { 0.0 };
                    let x0 = rrect.min.x + i as f32 * (cw + gap);
                    let cell = egui::Rect::from_min_size(
                        egui::pos2(x0, rrect.min.y),
                        egui::vec2(cw.max(1.0), 12.0),
                    );
                    if f >= 1.0 {
                        p.rect_filled(cell, 3.0, green);
                    } else if f > 0.0 {
                        p.rect_filled(cell, 3.0, egui::Color32::from_rgb(30, 31, 42));
                        let fill = egui::Rect::from_min_size(
                            cell.min,
                            egui::vec2(cell.width() * f, 12.0),
                        );
                        p.rect_filled(fill, 3.0, egui::Color32::from_rgb(96, 165, 250));
                    }
                }
            }
            } // end Info tab
            }); // end scrollable content
        });
    }
}
