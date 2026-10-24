//! IDM-style "Download all links" window (`--links <file>`).
//!
//! The browser extension collects every link on a page (or in the selection)
//! and the native host writes them to `links_<ms>.json` in the config dir.
//! This window lists them with a checkbox each, fills in type and size with
//! background HEAD probes, and queues the checked ones straight into the
//! shared `queue.json` (+ `wake.json` to start them), like the confirm dialog.

use crate::engine::{self, EngineOpts};
use crate::model::{category_of, config_dir, filename_from_url, format_bytes, DownloadItem};
use crate::native_host::LinksBatch;
use std::sync::mpsc::Receiver;

/// Probes in flight at once (one per host would be politer, but pages that
/// list files usually serve them all from one host anyway).
const PROBE_CONCURRENCY: usize = 6;
/// Links beyond this are listed but not probed (type from the name only).
const PROBE_MAX: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    Pending,
    Done,
    Failed,
}

struct Row {
    url: String,
    text: String,
    name: String,
    /// The name is still our guess from the URL (a server name may replace it).
    guessed_name: bool,
    kind: String,
    size: Option<u64>,
    html: bool,
    image: bool,
    checked: bool,
    probe: Probe,
}

/// What the file is, from its name and (once probed) its Content-Type:
/// (label, is a web page, is an image).
fn type_of(name: &str, ctype: Option<&str>) -> (String, bool, bool) {
    let mime = ctype
        .map(|c| c.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "application/octet-stream");
    if let Some(m) = mime.as_deref() {
        if m == "text/html" || m == "application/xhtml+xml" {
            return ("Web page".to_string(), true, false);
        }
        if m.starts_with("image/") {
            return (format!("{} image", m[6..].to_ascii_uppercase()), false, true);
        }
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "htm" | "html" | "php" | "asp" | "aspx" | "jsp" | "cgi" | "shtml" => {
            return ("Web page".to_string(), true, false)
        }
        "" if mime.is_none() => return ("Web page".to_string(), true, false),
        _ => {}
    }
    let cat = category_of(name);
    if cat == "Image" {
        return (format!("{} image", ext.to_ascii_uppercase()), false, true);
    }
    if !ext.is_empty() && cat != "Other" {
        return (format!("{} {}", ext.to_ascii_uppercase(), cat.to_ascii_lowercase()), false, false);
    }
    match mime {
        Some(m) => (m, false, false),
        None => (format!("{} file", ext.to_ascii_uppercase()), false, false),
    }
}

/// Same host (cookies of the page are only sent back to it).
fn same_host(a: &str, b: &str) -> bool {
    let host = |u: &str| reqwest::Url::parse(u).ok().and_then(|x| x.host_str().map(|h| h.to_ascii_lowercase()));
    matches!((host(a), host(b)), (Some(x), Some(y)) if x == y)
}

/// Read the host's links file, but only from our own config dir and only
/// under the name the host gives it; the file is removed once read.
fn read_batch(path: &std::path::Path) -> Result<LinksBatch, String> {
    let dir = config_dir().ok_or("no config dir")?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let inside = path.parent().map(|p| p == dir).unwrap_or(false);
    if !inside || !name.starts_with("links_") || !name.ends_with(".json") {
        return Err("not a links file from the browser extension".into());
    }
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(path);
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

struct LinksApp {
    page_url: String,
    title: String,
    cookie: String,
    rows: Vec<Row>,
    filter: String,
    hide_html: bool,
    hide_images: bool,
    /// true = every file into its category folder (Settings rules);
    /// false = all into `one_dir`.
    by_category: bool,
    one_dir: String,
    prefs: crate::confirm::UiPrefs,
    /// Widths of the sized columns (see `COLS`), dragged by their dividers.
    cols: [f32; 4],
    probe_rx: Option<Receiver<(usize, Option<u64>, Option<String>, Option<String>)>>,
    probes_left: usize,
    _rt: std::sync::Arc<tokio::runtime::Runtime>,
    notice: String,
    centered: bool,
    done: bool,
    exited: bool,
}

impl LinksApp {
    fn visible(&self, r: &Row) -> bool {
        if self.hide_html && r.html {
            return false;
        }
        if self.hide_images && r.image {
            return false;
        }
        let f = self.filter.trim().to_lowercase();
        f.is_empty()
            || r.name.to_lowercase().contains(&f)
            || r.url.to_lowercase().contains(&f)
            || r.text.to_lowercase().contains(&f)
            || r.kind.to_lowercase().contains(&f)
    }

    fn start_probes(&mut self, ctx: egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.probe_rx = Some(rx);
        let jobs: Vec<(usize, String)> = self
            .rows
            .iter()
            .enumerate()
            .take(PROBE_MAX)
            .map(|(i, r)| (i, r.url.clone()))
            .collect();
        self.probes_left = jobs.len();
        for r in self.rows.iter_mut().skip(PROBE_MAX) {
            r.probe = Probe::Failed;
        }
        let base = EngineOpts {
            user_agent: if self.prefs.user_agent.trim().is_empty() {
                format!("download_craft/{}", crate::model::APP_VERSION)
            } else {
                self.prefs.user_agent.clone()
            },
            timeout_secs: self.prefs.timeout_secs.min(20),
            proxy_url: self.prefs.proxy_url.clone(),
            referer: self.page_url.clone(),
            ..Default::default()
        };
        let with_cookie = EngineOpts { cookie: self.cookie.clone(), ..base.clone() };
        let page = self.page_url.clone();
        self._rt.spawn(async move {
            use futures_util::StreamExt;
            let (Ok(plain), Ok(session)) = (base.build_client(), with_cookie.build_client()) else {
                for (i, _) in jobs {
                    let _ = tx.send((i, None, None, None));
                }
                ctx.request_repaint();
                return;
            };
            futures_util::stream::iter(jobs)
                .for_each_concurrent(PROBE_CONCURRENCY, |(i, url)| {
                    let (client, opts) = if same_host(&url, &page) {
                        (&session, &with_cookie)
                    } else {
                        (&plain, &base)
                    };
                    let tx = tx.clone();
                    let ctx = ctx.clone();
                    async move {
                        let (total, _, ctype, disp) = engine::probe(client, opts, &url).await;
                        let _ = tx.send((i, total, ctype, disp));
                        ctx.request_repaint();
                    }
                })
                .await;
        });
    }

    fn collect_probes(&mut self) {
        let Some(rx) = self.probe_rx.as_ref() else { return };
        while let Ok((i, total, ctype, disp)) = rx.try_recv() {
            self.probes_left = self.probes_left.saturating_sub(1);
            let Some(r) = self.rows.get_mut(i) else { continue };
            if total.is_none() && ctype.is_none() {
                r.probe = Probe::Failed;
                continue;
            }
            r.probe = Probe::Done;
            if r.guessed_name {
                if let Some(d) = disp.filter(|d| !d.trim().is_empty()) {
                    r.name = engine::sanitize_filename(&d);
                } else if !r.name.contains('.') {
                    if let Some(ext) = ctype.as_deref().and_then(crate::model::ext_for_mime) {
                        r.name.push_str(ext);
                    }
                }
            }
            let (kind, html, image) = type_of(&r.name, ctype.as_deref());
            r.kind = kind;
            r.html = html;
            r.image = image;
            // A page's length is its HTML, not a download size.
            r.size = total.filter(|s| *s > 0 && !html);
        }
    }

    fn dir_for(&self, name: &str) -> String {
        if !self.by_category {
            return self.one_dir.clone();
        }
        let mut p = std::path::PathBuf::from(&self.prefs.save_dir);
        match self.prefs.folder_rules.iter().find(|r| r.matches(name)) {
            Some(rule) if !rule.folder.trim().is_empty() => p.push(rule.folder.trim()),
            _ => p.push(category_of(name)),
        }
        p.to_string_lossy().to_string()
    }

    /// Queue the checked, visible rows; `start` also starts them.
    fn commit(&mut self, start: bool) {
        let picked: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.rows[i].checked && self.visible(&self.rows[i]))
            .collect();
        if picked.is_empty() {
            self.notice = "Check the links to download first.".into();
            return;
        }
        let (mut added, mut already, mut failed) = (Vec::new(), 0usize, None);
        for i in picked {
            let r = &self.rows[i];
            let name = crate::model::apply_rename_template(
                &self.prefs.rename_template,
                &engine::sanitize_filename(r.name.trim()),
                &r.url,
            );
            let mut item = DownloadItem::new(
                0,
                r.url.clone(),
                engine::sanitize_filename(&name),
                self.dir_for(&name),
                self.prefs.connections,
            );
            item.referer = self.page_url.clone();
            if same_host(&r.url, &self.page_url) {
                item.cookie = self.cookie.clone();
            }
            item.description = r.text.clone();
            item.total_bytes = r.size;
            match crate::model::append_to_queue(&mut item) {
                crate::model::AppendResult::Added(id) => added.push(id),
                crate::model::AppendResult::AlreadyQueued(_) => already += 1,
                crate::model::AppendResult::Error(e) => failed = Some(e),
            }
        }
        if start && !added.is_empty() {
            crate::confirm::wake_ids(&added);
        }
        if let Some(e) = failed {
            self.notice = format!("Queued {}; could not update the queue: {e}", added.len());
            return;
        }
        if added.is_empty() {
            self.notice = format!("All {already} checked link(s) are already in the queue.");
            return;
        }
        self.done = true;
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let visible: Vec<usize> = (0..self.rows.len()).filter(|&i| self.visible(&self.rows[i])).collect();
        let cb = 26.0;
        let row_h = 22.0;
        let widths = col_widths(&self.cols, ui.available_width() - cb - 14.0);
        let strong = ui.visuals().strong_text_color();
        let weak = ui.visuals().weak_text_color();
        let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let accent = crate::dm::DM_ACC;
        // Fixed-width, left-aligned cell (add_sized centers its widget), with
        // a little room before the next column's divider.
        fn cell(ui: &mut egui::Ui, w: f32, h: f32, add: impl FnOnce(&mut egui::Ui) -> egui::Response) -> egui::Response {
            ui.allocate_ui_with_layout(egui::vec2(w, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_size(egui::vec2(w, h));
                ui.set_max_width(w - 8.0);
                add(ui)
            })
            .inner
        }
        // ---- header: labels, and a divider after every column but the last
        // that can be dragged (a faint line always, accent while in use) ----
        let (hdr, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
        let p = ui.painter().clone();
        let mut x = hdr.min.x + cb;
        for (i, label) in COLS.iter().enumerate() {
            p.text(
                egui::pos2(x + 2.0, hdr.center().y),
                egui::Align2::LEFT_CENTER,
                *label,
                egui::FontId::proportional(13.0),
                strong,
            );
            x += widths[i];
            if i + 1 == COLS.len() {
                break;
            }
            let zone = egui::Rect::from_center_size(egui::pos2(x - 4.0, hdr.center().y), egui::vec2(8.0, row_h));
            let r = ui
                .interact(zone, egui::Id::new(("links-col", i)), egui::Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeColumn)
                .on_hover_text("Drag to resize");
            let active = r.hovered() || r.dragged();
            p.rect_filled(
                egui::Rect::from_center_size(
                    zone.center(),
                    egui::vec2(if active { 3.0 } else { 1.0 }, row_h - if active { 6.0 } else { 10.0 }),
                ),
                1.5,
                if active { accent } else { line },
            );
            if r.dragged() {
                let dx = r.drag_delta().x;
                if dx != 0.0 {
                    self.cols = drag_divider(&self.cols, i, dx);
                }
            }
            if r.drag_stopped() {
                save_cols(&self.cols);
            }
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height() - 4.0)
            .show_rows(ui, row_h, visible.len(), |ui, range| {
                for &i in &visible[range] {
                    let r = &mut self.rows[i];
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_sized([cb, row_h], egui::Checkbox::without_text(&mut r.checked));
                        let name = cell(ui, widths[0], row_h, |ui| {
                            ui.add(egui::Label::new(&r.name).truncate().sense(egui::Sense::click()))
                        });
                        if name.on_hover_text(&r.name).clicked() {
                            r.checked = !r.checked;
                        }
                        let kind = match r.probe {
                            Probe::Pending => egui::RichText::new(format!("{}…", r.kind)).color(weak),
                            _ => egui::RichText::new(&r.kind),
                        };
                        cell(ui, widths[1], row_h, |ui| ui.add(egui::Label::new(kind).truncate()));
                        let size = match (r.size, r.probe) {
                            (Some(s), _) => format_bytes(s),
                            (None, Probe::Pending) => "…".into(),
                            _ => String::new(),
                        };
                        cell(ui, widths[2], row_h, |ui| ui.add(egui::Label::new(size).truncate()));
                        cell(ui, widths[3], row_h, |ui| ui.add(egui::Label::new(egui::RichText::new(&r.url).color(weak)).truncate()))
                            .on_hover_text(&r.url);
                        cell(ui, widths[4], row_h, |ui| ui.add(egui::Label::new(&r.text).truncate()));
                    });
                }
            });
    }
}

/// Column titles, left to right. "Download from" takes whatever room the
/// others leave.
const COLS: [&str; 5] = ["File name", "Type", "Size", "Download from", "Link text"];
const COL_MIN: f32 = 50.0;
/// Stored widths of the four sized columns (name, type, size, link text).
const COLS_DEFAULT: [f32; 4] = [300.0, 110.0, 80.0, 170.0];

/// All five widths for a table `avail` wide: the stored four, and the rest
/// for "Download from" (never under 120).
fn col_widths(stored: &[f32; 4], avail: f32) -> [f32; 5] {
    let [n, t, s, x] = *stored;
    let flex = (avail - (n + t + s + x)).max(120.0);
    [n, t, s, flex, x]
}

/// Drag the divider after column `i` by `dx`: it follows the pointer, so the
/// column on its left grows by what the one on its right loses; the flexible
/// "Download from" column absorbs the change when it is one of the two.
fn drag_divider(stored: &[f32; 4], i: usize, dx: f32) -> [f32; 4] {
    // A column (0..5) to its slot in `stored`; "Download from" has none.
    let slot = |c: usize| match c {
        0..=2 => Some(c),
        4 => Some(3),
        _ => None,
    };
    let mut out = *stored;
    match (slot(i), slot(i + 1)) {
        (Some(l), None) => out[l] = (out[l] + dx).clamp(COL_MIN, 900.0),
        (None, Some(r)) => out[r] = (out[r] - dx).clamp(COL_MIN, 900.0),
        (Some(l), Some(r)) => {
            let dx = dx.clamp(COL_MIN - out[l], out[r] - COL_MIN);
            out[l] += dx;
            out[r] -= dx;
        }
        (None, None) => {}
    }
    out
}

/// Column widths kept in ui.json (`links_cols`), so the window reopens as left.
fn load_cols() -> [f32; 4] {
    let v: serde_json::Value = config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let mut out = COLS_DEFAULT;
    if let Some(a) = v.get("links_cols").and_then(|x| x.as_array()) {
        for (i, w) in a.iter().take(4).enumerate() {
            if let Some(w) = w.as_f64().filter(|w| w.is_finite()) {
                out[i] = (w as f32).clamp(COL_MIN, 900.0);
            }
        }
    }
    out
}

fn save_cols(cols: &[f32; 4]) {
    let Some(path) = config_dir().map(|d| d.join("ui.json")) else { return };
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("links_cols".into(), serde_json::json!(cols.iter().map(|w| w.round()).collect::<Vec<_>>()));
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            let _ = crate::model::atomic_write(&path, json.as_bytes());
        }
    }
}

/// A name for a link whose URL names no file (a folder listing, a page):
/// its link text, else the last part of the URL, as an .html page.
fn page_name(url: &str, text: &str) -> String {
    let from_url = || {
        let u = reqwest::Url::parse(url).ok()?;
        let tail = u
            .query_pairs()
            .last()
            .map(|(_, v)| v.into_owned())
            .or_else(|| u.path_segments().and_then(|mut s| s.next_back().map(|x| x.to_string())))
            .filter(|t| !t.trim().is_empty())
            .or_else(|| u.host_str().map(|h| h.to_string()))?;
        Some(tail.rsplit('/').find(|p| !p.trim().is_empty()).unwrap_or(&tail).to_string())
    };
    let t = text.trim();
    let base = if !t.is_empty() && t.chars().count() <= 80 && !t.contains('{') {
        t.to_string()
    } else {
        from_url().unwrap_or_else(|| "page".into())
    };
    engine::sanitize_filename(&format!("{base}.html"))
}

/// Entry point for `--links <file>`.
pub fn run() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .iter()
        .position(|a| a == "--links")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let batch = match read_batch(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("--links: {e}");
            std::process::exit(2);
        }
    };
    let prefs = crate::confirm::load_prefs();
    let rows = batch
        .links
        .into_iter()
        .map(|l| {
            let mut name = engine::sanitize_filename(&filename_from_url(&l.url));
            if crate::model::is_fallback_filename(&name) {
                name = page_name(&l.url, &l.text);
            }
            let (kind, html, image) = type_of(&name, None);
            Row { url: l.url, text: l.text, name, guessed_name: true, kind, size: None, html, image, checked: false, probe: Probe::Pending }
        })
        .collect();
    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    let mut app = LinksApp {
        page_url: batch.page_url,
        title: batch.title,
        cookie: batch.cookie,
        rows,
        filter: String::new(),
        hide_html: false,
        hide_images: true,
        by_category: true,
        one_dir: prefs.save_dir.clone(),
        cols: load_cols(),
        prefs,
        probe_rx: None,
        probes_left: 0,
        _rt: rt,
        notice: String::new(),
        centered: false,
        done: false,
        exited: false,
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download all links — Download Craft")
        .with_inner_size([LINKS_W, LINKS_H])
        .with_min_inner_size([640.0, 380.0])
        .with_resizable(true)
        .with_always_on_top();
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Download all links",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(|cc| {
            crate::app::setup_fonts(cc);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            app.start_probes(cc.egui_ctx.clone());
            Ok(Box::new(app))
        }),
    )
}

const LINKS_W: f32 = 1000.0;
const LINKS_H: f32 = 600.0;

impl eframe::App for LinksApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            if !self.exited {
                self.exited = true;
                // Let wake_ids launch the main window before this exits.
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    std::process::exit(0);
                });
            }
            return;
        }
        if !self.centered && crate::model::center_window(ctx, egui::vec2(LINKS_W, LINKS_H)) {
            self.centered = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.collect_probes();
        let accent = crate::dm::DM_ACC;

        egui::TopBottomPanel::top("links_top").show(ctx, |ui| {
            ui.add_space(8.0);
            let heading = if self.title.trim().is_empty() { "Download all links".to_string() } else { self.title.clone() };
            ui.label(egui::RichText::new(heading).strong().size(15.0));
            ui.label(egui::RichText::new(&self.page_url).small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by name, type or URL…").desired_width(260.0));
                ui.checkbox(&mut self.hide_html, "Hide web pages");
                ui.checkbox(&mut self.hide_images, "Hide images");
                if self.probes_left > 0 {
                    ui.spinner();
                    ui.label(egui::RichText::new(format!("Checking types and sizes… {} left", self.probes_left)).weak());
                }
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("links_bottom").show(ctx, |ui| {
            ui.add_space(6.0);
            let (n, bytes) = self
                .rows
                .iter()
                .filter(|r| r.checked && self.visible(r))
                .fold((0usize, 0u64), |(n, b), r| (n + 1, b + r.size.unwrap_or(0)));
            ui.horizontal(|ui| {
                if ui.button("Check all").on_hover_text("Check every link shown above").clicked() {
                    let shown: Vec<usize> = (0..self.rows.len()).filter(|&i| self.visible(&self.rows[i])).collect();
                    for i in shown {
                        self.rows[i].checked = true;
                    }
                }
                if ui.button("Uncheck all").clicked() {
                    for r in &mut self.rows {
                        r.checked = false;
                    }
                }
                ui.label(if bytes > 0 { format!("{n} checked · {}", format_bytes(bytes)) } else { format!("{n} checked") });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Save to");
                ui.radio_value(&mut self.by_category, true, "Category folders")
                    .on_hover_text("Every file into its category folder (Settings → Folders)");
                ui.radio_value(&mut self.by_category, false, "One folder");
                ui.add_enabled(!self.by_category, egui::TextEdit::singleline(&mut self.one_dir).desired_width(300.0));
                if ui.add_enabled(!self.by_category, egui::Button::new("Browse…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().set_directory(&self.one_dir).pick_folder() {
                        self.one_dir = p.to_string_lossy().to_string();
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !self.notice.is_empty() {
                    ui.label(egui::RichText::new(&self.notice).color(crate::dm::DM_YEL));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        self.done = true;
                    }
                    let go = egui::Button::new(egui::RichText::new("Download").strong().color(egui::Color32::WHITE)).fill(accent);
                    if ui.add_enabled(n > 0, go).on_hover_text("Queue the checked links and start them").clicked() {
                        self.commit(true);
                    }
                    if ui.add_enabled(n > 0, egui::Button::new("Add to queue")).on_hover_text("Queue the checked links without starting them").clicked() {
                        self.commit(false);
                    }
                });
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| self.table(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_come_from_content_type_then_name() {
        assert_eq!(type_of("A.Knight.S01E01.mp4", None), ("MP4 video".to_string(), false, false));
        assert_eq!(type_of("index.php", None).1, true);
        assert_eq!(type_of("S01", None).1, true, "no extension, not probed: a directory page");
        assert_eq!(type_of("S01", Some("text/html; charset=utf-8")).0, "Web page");
        assert!(type_of("logo.png", None).2);
        assert!(type_of("photo", Some("image/jpeg")).2);
        assert_eq!(type_of("file.bin", Some("application/octet-stream")).0, "BIN file");
        assert_eq!(type_of("watch", Some("video/mp4")).0, "video/mp4");
    }

    #[test]
    fn dividers_follow_the_pointer_and_name_pages() {
        let c = COLS_DEFAULT;
        // Name | Type: both sized, sum kept.
        let d = drag_divider(&c, 0, 40.0);
        assert_eq!((d[0], d[1]), (c[0] + 40.0, c[1] - 40.0));
        // Size | Download from: Size grows, the flexible column gives way.
        assert_eq!(drag_divider(&c, 2, 30.0)[2], c[2] + 30.0);
        // Download from | Link text: dragging right narrows the link text.
        assert_eq!(drag_divider(&c, 3, 30.0)[3], c[3] - 30.0);
        // Minimums hold.
        assert_eq!(drag_divider(&c, 0, 900.0)[1], COL_MIN);
        let w = col_widths(&c, 1000.0);
        assert_eq!(w.iter().sum::<f32>(), 1000.0);
        assert_eq!(col_widths(&c, 100.0)[3], 120.0);
        // Folder links name a page, not "download_<time>.bin".
        assert_eq!(page_name("https://d.example/?dir=Series/English/A", "A"), "A.html");
        assert_eq!(page_name("https://d.example/?dir=Series/English/A", ""), "A.html");
        assert_eq!(page_name("https://d.example/", ".cls-1{fill:url(#g);}"), "d.example.html");
        assert!(type_of(&page_name("https://d.example/?dir=S01", "S01"), None).1);
    }

    #[test]
    fn page_cookies_stay_on_the_page_host() {
        assert!(same_host("https://download.example.net/a.mp4", "https://download.example.net/?dir=x"));
        assert!(!same_host("https://twitter.com/x", "https://download.example.net/"));
        assert!(!same_host("not a url", "https://download.example.net/"));
    }

    #[test]
    fn links_file_must_come_from_the_config_dir() {
        assert!(read_batch(std::path::Path::new("C:/Windows/links_1.json")).is_err());
        if let Some(dir) = config_dir() {
            assert!(read_batch(&dir.join("queue.json")).is_err());
        }
    }
}
