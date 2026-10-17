use crate::dm::*;
use crate::engine::{download, EngineOpts, SharedState};
use crate::manual::ManState;
use crate::model::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

// ---------- theme (AB-style dark) ----------
/// Unified geometry: every card, modal, field, and button across ALL
/// windows (main, Add-download dialog, progress) uses this radius.
pub const RADIUS: f32 = 10.0;
const BG: egui::Color32 = egui::Color32::from_rgb(18, 19, 26);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(26, 27, 36);
const SURFACE_LT: egui::Color32 = egui::Color32::from_rgb(36, 37, 50);
const HEADER_BG: egui::Color32 = egui::Color32::from_rgb(13, 14, 20);
const CENTRAL_BG: egui::Color32 = egui::Color32::from_rgb(15, 16, 22);
const CARD_BD: egui::Color32 = egui::Color32::from_rgb(35, 36, 48);
const PRIMARY: egui::Color32 = egui::Color32::from_rgb(124, 92, 252);
const SECONDARY: egui::Color32 = egui::Color32::from_rgb(184, 93, 255);
const TXT: egui::Color32 = egui::Color32::from_rgb(230, 230, 240);
const DIM: egui::Color32 = egui::Color32::from_rgb(140, 142, 160);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(76, 217, 100);
const WARNING: egui::Color32 = egui::Color32::from_rgb(255, 159, 10);
const ERROR: egui::Color32 = egui::Color32::from_rgb(255, 69, 58);
const SEL_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(18, 14, 38, 38);
const SEL_BD: egui::Color32 = egui::Color32::from_rgb(124, 92, 252);
const HOVER_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(15, 16, 19, 26);

pub fn dark_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.window_fill = SURFACE;
    v.panel_fill = BG;
    v.faint_bg_color = SURFACE;
    v.extreme_bg_color = BG;
    v.override_text_color = Some(TXT);
    v.weak_text_color = Some(DIM);
    // Table cells paint faint_bg: keep the list on app background.
    v.faint_bg_color = BG;
    // Hide idle table resize lines + plain separators for a minimal look
    // (hover/drag lines still show while resizing).
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BG);
    v.selection.bg_fill = SEL_BG;
    v.selection.stroke = egui::Stroke::new(1.0_f32, SEL_BD);
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.hovered.bg_fill = SURFACE_LT;
    v.widgets.active.bg_fill = egui::Color32::from_rgb(0x3A, 0x3B, 0x44);
    v.widgets.noninteractive.bg_fill = SURFACE;
    v
}

/// Pure-black theme preset.
pub fn black_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let black = egui::Color32::BLACK;
    let panel = egui::Color32::from_rgb(0x0C, 0x0C, 0x0E);
    v.window_fill = panel;
    v.panel_fill = black;
    v.faint_bg_color = black;
    v.extreme_bg_color = panel;
    v.override_text_color = Some(TXT);
    v.weak_text_color = Some(DIM);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, black);
    v.selection.bg_fill = SEL_BG;
    v.selection.stroke = egui::Stroke::new(1.0_f32, SEL_BD);
    v.widgets.inactive.bg_fill = panel;
    v.widgets.hovered.bg_fill = SURFACE;
    v.widgets.active.bg_fill = SURFACE_LT;
    v.widgets.noninteractive.bg_fill = panel;
    v
}

/// Light theme preset.
pub fn light_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.selection.bg_fill = egui::Color32::from_rgb(0xD6, 0xE4, 0xF5);
    v.selection.stroke = egui::Stroke::new(1.0_f32, PRIMARY);
    v
}

pub fn visuals_for(theme: &str) -> egui::Visuals {
    match theme {
        "black" => black_visuals(),
        "light" => light_visuals(),
        _ => dark_visuals(),
    }
}

/// Widget-level theme shared by EVERY window: rounded controls, roomy
/// buttons, consistent spacing. Call after each `set_visuals` (theme
/// switches) so visuals + geometry never drift apart again.
pub fn apply_widget_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    for w in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(RADIUS as u8);
    }
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(16.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(10.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(11.0, egui::FontFamily::Monospace),
    );
    style.spacing.button_padding = egui::Vec2::new(14.0, 10.0);
    style.spacing.item_spacing = egui::Vec2::new(8.0, 10.0);
    style.spacing.icon_width = 14.0;
    ctx.set_style(style);
}

// ---------- Material Icons codepoints (font bundled under assets/fonts) ----------
pub(crate) const MI_SEARCH: &str = "\u{e8b6}";
pub(crate) const MI_ADD_LINK: &str = "\u{e178}";
pub(crate) const MI_DOWNLOAD: &str = "\u{f090}";
pub(crate) const MI_PLAY: &str = "\u{e037}";
pub(crate) const MI_STOP: &str = "\u{e047}";
pub(crate) const MI_QUEUE: &str = "\u{e03d}";
pub(crate) const MI_SETTINGS: &str = "\u{e8b8}";
pub(crate) const MI_FOLDER: &str = "\u{e2c7}";
pub(crate) const MI_IMAGE: &str = "\u{e3f4}";
pub(crate) const MI_MUSIC: &str = "\u{e405}";
pub(crate) const MI_MOVIE: &str = "\u{e02c}";
pub(crate) const MI_GRID: &str = "\u{e9b0}";
pub(crate) const MI_DOC: &str = "\u{e873}";
pub(crate) const MI_ARCHIVE: &str = "\u{e149}";
pub(crate) const MI_FILE: &str = "\u{e24d}";
pub(crate) const MI_AUDIO_FILE: &str = "\u{eb82}";
pub(crate) const MI_VIDEO_FILE: &str = "\u{eb87}";
pub(crate) const MI_APPS: &str = "\u{e5c3}";
pub(crate) const MI_MENU: &str = "\u{e5d2}";
pub(crate) const MI_CLOUD_DL: &str = "\u{e2c0}";
pub(crate) const MI_PASTE: &str = "\u{e14f}";
pub(crate) const MI_REFRESH: &str = "\u{e5d5}";
pub(crate) const MI_CHECK: &str = "\u{e5ca}";
pub(crate) const MI_FOLDER_OPEN: &str = "\u{e2c8}";
pub(crate) const MI_PAUSE: &str = "\u{e034}";
pub(crate) const MI_COPY: &str = "\u{e14d}";
pub(crate) const MI_PALETTE: &str = "\u{e40a}";
pub(crate) const MI_PUBLIC: &str = "\u{e80b}";
pub(crate) const MI_UPDATE: &str = "\u{e923}";
pub(crate) const MI_SPEED: &str = "\u{e9e4}";

/// Material Icons (Apache-2.0, Google) as fallback font for UI glyphs, plus
/// the named Segoe UI families the Add-download dialog asks for.
/// Every window (main, confirm, progress, basket, mini) must call this —
/// otherwise icon glyphs render as tofu boxes.
pub fn setup_fonts(cc: &eframe::CreationContext<'_>) {
    // `false`: the manager window keeps its own typography and only the named
    // families ("segoe"/"popup-semibold") are added for the dialog.
    crate::fonts::install(cc, false);
}

fn file_icon(cat: &str) -> &'static str {
    match cat {
        "Image" => MI_IMAGE,
        "Music" => MI_AUDIO_FILE,
        "Video" => MI_VIDEO_FILE,
        "Apps" => MI_APPS,
        "Document" => MI_DOC,
        "Compressed" => MI_ARCHIVE,
        _ => MI_FILE,
    }
}

fn mi(ui: &mut egui::Ui, glyph: &str, size: f32, color: egui::Color32) {
    // Hover-only on purpose: icon glyphs must never steal row clicks.
    ui.add(
        egui::Label::new(
            egui::RichText::new(glyph)
                .size(size)
                .color(color)
                .family(egui::FontFamily::Proportional),
        )
        .sense(egui::Sense::hover()),
    );
}

/// Standard modal dialog frame (dark card, 1px border, rounded).
fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(20, 21, 29))
        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(45, 47, 65)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(16))
}

/// Centered-modal width that can never overflow the viewport: clamped to
/// 90% of the screen (and to a sane min/max), so input fields and buttons
/// always fit, even on narrow windows.
fn modal_width(ctx: &egui::Context, max_w: f32) -> f32 {
    (ctx.screen_rect().width() * 0.90).clamp(320.0, max_w)
}

/// Advanced ("settings") panel height. The concept draws it collapsed, so this
/// part is app design rendered in the concept's own tokens (see `crate::dm`).
const DM_PANEL_H: f32 = 224.0;


/// Concept icon button: hover wash plus glyph. Input stays on the manual hit
/// table, so this only paints.
fn dm_icon_btn(
    p: &egui::Painter,
    man: &ManState,
    rect: egui::Rect,
    icon: DmIcon,
    size: f32,
    w: f32,
    color: egui::Color32,
) {
    crate::dm::icon_btn(p, rect, man.hover(&rect), icon, size, w, color);
}

/// Outline button (concept "Add" / "Cancel").
fn dm_btn_flat(p: &egui::Painter, man: &ManState, r: egui::Rect, label: &str) {
    crate::dm::btn_flat(p, r, man.hover(&r), label);
}


/// Manual slider painted in the concept's tokens. Geometry comes from
/// `slider_track`, so the manual drag handling stays exact.
fn dm_slider(p: &egui::Painter, man: &ManState, rect: egui::Rect, frac: f32) {
    let (tx0, tw) = slider_track(&rect);
    crate::dm::slider(p, rect, man.hover(&rect), frac, tx0, tw);
}


/// Static field keys for the capped dynamic rows (rules ≤8, windows ≤6).
/// `HitKind::Field` needs `&'static str`, so a fixed pool stands in for
/// per-row dynamic ids.
fn rule_ext_key(i: usize) -> &'static str {
    match i {
        0 => "rule_ext_0", 1 => "rule_ext_1", 2 => "rule_ext_2", 3 => "rule_ext_3",
        4 => "rule_ext_4", 5 => "rule_ext_5", 6 => "rule_ext_6",
        _ => "rule_ext_7",
    }
}
fn rule_dir_key(i: usize) -> &'static str {
    match i {
        0 => "rule_dir_0", 1 => "rule_dir_1", 2 => "rule_dir_2", 3 => "rule_dir_3",
        4 => "rule_dir_4", 5 => "rule_dir_5", 6 => "rule_dir_6",
        _ => "rule_dir_7",
    }
}
fn win_start_key(i: usize) -> &'static str {
    match i {
        0 => "win_s_0", 1 => "win_s_1", 2 => "win_s_2",
        3 => "win_s_3", 4 => "win_s_4",
        _ => "win_s_5",
    }
}
fn win_end_key(i: usize) -> &'static str {
    match i {
        0 => "win_e_0", 1 => "win_e_1", 2 => "win_e_2",
        3 => "win_e_3", 4 => "win_e_4",
        _ => "win_e_5",
    }
}
fn win_lim_key(i: usize) -> &'static str {
    match i {
        0 => "win_l_0", 1 => "win_l_1", 2 => "win_l_2",
        3 => "win_l_3", 4 => "win_l_4",
        _ => "win_l_5",
    }
}

/// Full-width sidebar navigation row: icon + label left, count badge
/// right, accent bar + highlight when selected. Manual-input version:
/// reserves space, paints (hover from caller), returns the hit rect.
/// The whole row is one click target.
fn side_nav_row(
    ui: &mut egui::Ui,
    man: &ManState,
    selected: bool,
    icon: &str,
    label: &str,
    count: Option<usize>,
    dot: bool,
) -> egui::Rect {
    const H: f32 = 32.0;
    let w = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::hover());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 6.0, SURFACE_LT);
        p.rect_filled(
            egui::Rect::from_min_size(
                rect.min + egui::vec2(0.0, 6.0),
                egui::vec2(3.0, H - 12.0),
            ),
            1.5,
            PRIMARY,
        );
    } else if man.hover(&rect) {
        p.rect_filled(rect, 6.0, HOVER_BG);
    }
    let fg = if selected { TXT } else { DIM };
    if dot {
        p.circle_filled(
            egui::pos2(rect.min.x + 14.0, rect.center().y),
            3.0,
            SUCCESS,
        );
    } else {
        p.text(
            egui::pos2(rect.min.x + 14.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon.to_string(),
            egui::FontId::proportional(15.0),
            fg,
        );
    }
    p.text(
        egui::pos2(rect.min.x + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_string(),
        egui::FontId::proportional(12.5),
        fg,
    );
    if let Some(n) = count {
        let font = egui::FontId::proportional(11.0);
        let galley = p.layout_no_wrap(n.to_string(), font, DIM);
        let bw = galley.size().x + 14.0;
        let bh = 18.0;
        let br = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - bw - 8.0, rect.center().y - bh / 2.0),
            egui::vec2(bw, bh),
        );
        p.rect_filled(br, bh / 2.0, SURFACE_LT);
        p.galley(
            egui::pos2(
                br.center().x - galley.size().x / 2.0,
                br.center().y - galley.size().y / 2.0,
            ),
            galley,
            DIM,
        );
    }
    rect
}

/// Small caps section header for the sidebar.
fn side_section(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).size(10.0).color(DIM).strong());
    ui.add_space(2.0);
}

/// Small square icon button (Material glyph, guaranteed to render).
fn icon_btn(ui: &mut egui::Ui, glyph: &str, tip: &str) -> egui::Response {
    ui.add_sized(
        [30.0, 30.0],
        egui::Button::new(egui::RichText::new(glyph).size(16.0).color(DIM))
            .fill(egui::Color32::TRANSPARENT)
            .corner_radius(RADIUS),
    )
    .on_hover_text(tip)
}

/// Small rounded status pill, painted into an explicit rect (no cursor use).
fn status_pill_at(p: &egui::Painter, rect: egui::Rect, text: &str, color: egui::Color32) {
    let font = egui::FontId::proportional(12.0);
    let galley = p.layout_no_wrap(text.to_string(), font, color);
    let h = galley.size().y + 6.0;
    let pw = (galley.size().x + 20.0).min(rect.width()).max(20.0);
    let pr = egui::Rect::from_min_size(
        egui::pos2(rect.center().x - pw / 2.0, rect.center().y - h / 2.0),
        egui::vec2(pw, h),
    );
    let bg = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28);
    p.rect_filled(pr, pr.height() / 2.0, bg);
    p.galley(
        egui::pos2(
            pr.center().x - galley.size().x / 2.0,
            pr.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

/// Manual checkbox (paint only; input via manual routing).
fn paint_check(p: &egui::Painter, rect: egui::Rect, checked: bool, hov: bool) {
    p.rect_filled(
        rect,
        3.0,
        if checked {
            PRIMARY
        } else {
            egui::Color32::TRANSPARENT
        },
    );
    p.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.2, if hov || checked { TXT } else { DIM }),
        egui::StrokeKind::Inside,
    );
    if checked {
        let c = rect.center();
        p.line_segment(
            [c + egui::vec2(-4.5, 0.0), c + egui::vec2(-1.5, 3.0)],
            egui::Stroke::new(1.8, egui::Color32::WHITE),
        );
        p.line_segment(
            [c + egui::vec2(-1.5, 3.0), c + egui::vec2(4.5, -3.5)],
            egui::Stroke::new(1.8, egui::Color32::WHITE),
        );
    }
}

/// Button identity for manual routing (every clickable dialog control).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ButtonId {
    AddPaste,
    AddFolder,
    AddRefresh,
    AddAdv,
    AddAdd,
    AddDownload,
    AddCancel,
    QStart,
    QStop,
    QSchedReset,
    QClose,
    SFolder,
    SChrome,
    SEdge,
    SFirefox,
    SBrave,
    SOpera,
    SClose,
    DResume,
    DClose,
    AbClose,
    TAdd,
    TDismiss,
    ThemeDark,
    ThemeBlack,
    ThemeLight,
    GrabStart,
    GrabQueue,
    GrabClose,
    GrabPreset,
    RuleAdd,
    RuleDel(usize),
    RuleUp(usize),
    RuleDown(usize),
    RuleReset,
    WinAdd,
    WinDel(usize),
    UpdCheck,
    UpdDownload,
}

/// Checkbox identity for manual routing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CheckId {
    CatFold,
    Tray,
    Unlimited,
    SchedOn,
    AddOverwrite,
    GrabDepth,
    WinOn,
    UpdOn,
}

/// Slider identity for manual routing (values live in self fields/items).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SliderId {
    AddConns,
    LimitKB,
    Timeout,
    Retries,
    DefConns,
    QMax,
    DetConns,
    DetRetries,
    DetTimeout,
}

/// Active manual slider drag.
struct SliderCap {
    id: SliderId,
    min: f64,
    max: f64,
    log: bool,
    x0: f32,
    tw: f32,
}

/// Manual push button: reserves space, paints (+hover), returns hit rect.
/// Primary = filled accent; otherwise dark card. Text centered.
fn mbutton(
    ui: &mut egui::Ui,
    man: &ManState,
    label: &str,
    w: f32,
    h: f32,
    primary: bool,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter();
    if primary {
        p.rect_filled(rect, RADIUS, egui::Color32::from_rgb(108, 62, 220));
    } else {
        p.rect_filled(rect, RADIUS, SURFACE);
        if man.hover(&rect) {
            p.rect_filled(rect, RADIUS, SURFACE_LT);
        }
        p.rect_stroke(
            rect,
            RADIUS,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 47, 65)),
            egui::StrokeKind::Inside,
        );
    }
    let g = p.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(13.0),
        if primary { egui::Color32::WHITE } else { TXT },
    );
    p.galley(
        egui::pos2(
            rect.center().x - g.size().x / 2.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        if primary { egui::Color32::WHITE } else { TXT },
    );
    rect
}

/// Manual checkbox row: box + label, full width. Paint only.
fn mcheck(ui: &mut egui::Ui, man: &ManState, label: &str, checked: bool) -> egui::Rect {
    let w = ui.available_width().max(80.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::hover());
    if man.hover(&rect) {
        ui.painter().rect_filled(rect, 4.0, HOVER_BG);
    }
    let p = ui.painter();
    let br = egui::Rect::from_min_size(
        egui::pos2(rect.min.x + 2.0, rect.center().y - 7.0),
        egui::vec2(14.0, 14.0),
    );
    paint_check(&p, br, checked, man.hover(&br.expand(4.0)));
    p.text(
        egui::pos2(br.max.x + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_string(),
        egui::FontId::proportional(12.0),
        TXT,
    );
    rect
}

/// Manual slider: label + track + knob, painted; drag via slider_cap.
fn mslider(
    ui: &mut egui::Ui,
    man: &ManState,
    label: &str,
    minv: f64,
    maxv: f64,
    val: f64,
    log: bool,
) -> egui::Rect {
    let w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::hover());
    let p = ui.painter();
    let frac = if log {
        ((val.max(minv).ln() - minv.ln()) / (maxv.ln() - minv.ln())).clamp(0.0, 1.0)
    } else {
        ((val - minv) / (maxv - minv)).clamp(0.0, 1.0)
    };
    p.text(
        egui::pos2(rect.min.x, rect.min.y + 7.0),
        egui::Align2::LEFT_CENTER,
        label.to_string(),
        egui::FontId::proportional(12.0),
        DIM,
    );
    let (tx0, tw) = slider_track(&rect);
    let ty = rect.min.y + 22.0;
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(tx0, ty - 2.0), egui::vec2(tw, 4.0)),
        2.0,
        SURFACE_LT,
    );
    let kx = tx0 + tw * frac as f32;
    if man.hover(&rect) {
        p.circle_filled(
            egui::pos2(kx, ty),
            9.0,
            egui::Color32::from_rgba_unmultiplied(124, 92, 252, 40),
        );
    }
    p.circle_filled(egui::pos2(kx, ty), 7.0, PRIMARY);
    rect
}

/// Track geometry for a manual slider (same math in layout and process).
fn slider_track(rect: &egui::Rect) -> (f32, f32) {
    let tx0 = rect.min.x + 4.0;
    let tw = (rect.width() - 8.0).max(40.0);
    (tx0, tw)
}

/// Standard text field with an explicit id (so manual clicks can focus
/// it) — returns its rect for hit recording. Typing flows through normal
/// keyboard events once focused.
fn mtext(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    password: bool,
) -> egui::Rect {
    let te = egui::TextEdit::singleline(text)
        .id(egui::Id::new(id))
        .hint_text(hint)
        .password(password)
        .desired_width(width);
    ui.add(te).rect
}

/// Small square icon button (paint only; input via manual routing).
fn micon(ui: &mut egui::Ui, man: &ManState, glyph: &str) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
    if man.hover(&rect) {
        ui.painter().rect_filled(rect, 6.0, SURFACE_LT);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph.to_string(),
        egui::FontId::proportional(16.0),
        DIM,
    );
    rect
}

/// Focus a text field by its explicit id (manual click routing).
fn focus_field(ctx: &egui::Context, id: &str) {
    ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
}

/// Manual mini progress bar (paint only).
fn paint_progress(p: &egui::Painter, center_x: f32, top_y: f32, width: f32, frac: f32) {
    let w = width.max(20.0);
    let bar = egui::Rect::from_min_size(egui::pos2(center_x - w / 2.0, top_y), egui::vec2(w, 6.0));
    p.rect_filled(bar, 3.0, SURFACE);
    let fw = (w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 5.0 } else { 0.0 });
    if fw > 0.0 {
        p.rect_filled(
            egui::Rect::from_min_size(bar.min, egui::vec2(fw, 6.0)),
            3.0,
            PRIMARY,
        );
    }
}

/// Centered single-line text painted into an explicit rect (no cursor use).
/// A plain Label ignores halign for single-line text, so center manually.
fn centered_label_at(p: &egui::Painter, rect: egui::Rect, text: &str, size: f32, color: egui::Color32) {
    let galley = p.layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(size),
        color,
    );
    let gw = galley.size().x.min(rect.width());
    p.galley(
        egui::pos2(
            rect.center().x - gw / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

/// Truncate `text` with an ellipsis so it fits `max_w` (painter-measured).
fn truncate_to_width(
    p: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_w: f32,
) -> String {
    if p
        .layout_no_wrap(text.to_owned(), font.clone(), color)
        .size()
        .x
        <= max_w
    {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s: String = chars[..mid].iter().collect();
        let w = p
            .layout_no_wrap(s.clone() + "…", font.clone(), color)
            .size()
            .x;
        if w <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

/// Name cell painted with ZERO widgets (icon + filename + type via the
/// painter only). egui labels always interact — even hover-only ones —
/// which steals row hover/clicks from the text; painted text cannot.
fn name_cell(p: &egui::Painter, rect: egui::Rect, icon: &str, fname: &str, cat: &str) {
    p.text(
        egui::pos2(rect.min.x + 13.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon.to_owned(),
        egui::FontId::proportional(16.0),
        TXT,
    );
    let tx = rect.min.x + 28.0;
    let max_tw = (rect.max.x - 4.0 - tx).max(20.0);
    let name = truncate_to_width(p, fname, egui::FontId::proportional(12.0), TXT, max_tw);
    p.text(
        egui::pos2(tx, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.0),
        TXT,
    );
    p.text(
        egui::pos2(tx, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        cat.to_owned(),
        egui::FontId::proportional(9.0),
        DIM,
    );
}

/// Default / minimum widths per column.
fn col_default(c: SortCol) -> f32 {
    match c {
        SortCol::Name => 200.0,
        SortCol::Size => SIZE_W,
        SortCol::Status => STATUS_W,
        SortCol::Speed => SPEED_W,
        SortCol::Eta => ETA_W,
        SortCol::Date => DATE_W,
    }
}

fn col_min(c: SortCol) -> f32 {
    match c {
        SortCol::Name => 160.0,
        SortCol::Size => 60.0,
        SortCol::Status => 120.0,
        SortCol::Speed => 60.0,
        SortCol::Eta => 70.0,
        SortCol::Date => 80.0,
    }
}

fn col_w_of(map: &std::collections::HashMap<SortCol, f32>, c: SortCol) -> f32 {
    map.get(&c).copied().unwrap_or(col_default(c))
}

/// Uniform toolbar metrics: every action button shares one height and is
/// vertically centered, with a single gap between buttons.
const TOOL_H: f32 = 52.0;
const TOOL_GAP: f32 = 6.0;

/// Toolbar button: icon above label, 88px wide, uniform height.
/// Manual-input version: reserves space, paints (hover from caller),
/// returns the hit rect for the caller to record. Never reads responses.
fn tool_button(
    ui: &mut egui::Ui,
    man: &ManState,
    icon: &str,
    label: &str,
    enabled: bool,
    tip: &str,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(88.0, TOOL_H), egui::Sense::hover());
    if enabled && man.hover(&rect) {
        ui.painter().rect_filled(rect, 6.0, SURFACE_LT);
    }
    let c = if enabled { TXT } else { DIM };
    let p = ui.painter();
    // Icon and label share the height evenly so all buttons align.
    p.text(
        egui::pos2(rect.center().x, rect.min.y + TOOL_H * 0.32),
        egui::Align2::CENTER_CENTER,
        icon.to_string(),
        egui::FontId::proportional(17.0),
        c,
    );
    p.text(
        egui::pos2(rect.center().x, rect.min.y + TOOL_H * 0.74),
        egui::Align2::CENTER_CENTER,
        label.to_string(),
        egui::FontId::proportional(12.0),
        c,
    );
    if enabled && !tip.is_empty() && man.hover(&rect) {
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            ui.id().with(("toolbar_tip", label)),
            egui::PopupAnchor::Pointer,
        )
        .gap(12.0)
        .show(|ui| {
            ui.label(tip);
        });
    }
    rect
}

/// Add-URL pill: same hit height as the other toolbar buttons, with the
/// pill painted vertically centered, plus gradient download badge.
fn add_url_button(ui: &mut egui::Ui, man: &ManState) -> egui::Rect {
    let (hit, _) = ui.allocate_exact_size(egui::vec2(152.0, TOOL_H), egui::Sense::hover());
    let rect = egui::Rect::from_center_size(hit.center(), egui::vec2(152.0, 32.0));
    let p = ui.painter();
    p.rect_filled(
        rect,
        6.0,
        if man.hover(&hit) { SURFACE_LT } else { SURFACE },
    );
    p.text(
        egui::pos2(rect.min.x + 26.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        MI_ADD_LINK.to_string(),
        egui::FontId::proportional(16.0),
        TXT,
    );
    p.text(
        egui::pos2(rect.min.x + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Add URL".to_string(),
        egui::FontId::proportional(12.0),
        TXT,
    );
    let c = egui::pos2(rect.max.x - 15.0, rect.center().y);
    p.circle_filled(c, 10.0, SECONDARY);
    p.circle_filled(c + egui::vec2(-2.0, -2.0), 8.0, PRIMARY);
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        MI_DOWNLOAD.to_string(),
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
    hit
}

fn status_bar_w(ui: &mut egui::Ui, w: f32, frac: f32, c0: egui::Color32, c1: egui::Color32) {
    let w = w.max(20.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 5.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.5, SURFACE);
    p.rect_stroke(
        rect,
        2.5,
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(200, 200, 210, 25)),
        egui::StrokeKind::Inside,
    );
    if frac > 0.0 {
        let w = (rect.width() * frac.clamp(0.0, 1.0)).max(5.0);
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height()));
        let strips = 12;
        for i in 0..strips {
            let t0 = i as f32 / strips as f32;
            let t1 = (i + 1) as f32 / strips as f32;
            let seg = egui::Rect::from_min_max(
                egui::pos2(fill.min.x + fill.width() * t0, fill.min.y),
                egui::pos2(fill.min.x + fill.width() * t1, fill.max.y),
            );
            let c = egui::Color32::from_rgb(
                (c0.r() as f32 + (c1.r() as f32 - c0.r() as f32) * t0) as u8,
                (c0.g() as f32 + (c1.g() as f32 - c0.g() as f32) * t0) as u8,
                (c0.b() as f32 + (c1.b() as f32 - c0.b() as f32) * t0) as u8,
            );
            p.rect_filled(seg, 0.0, c);
        }
        p.circle_filled(egui::pos2(fill.min.x + 2.5, fill.center().y), 2.5, c0);
        p.circle_filled(egui::pos2(fill.max.x - 2.5, fill.center().y), 2.5, c1);
    }
}

struct ActiveTask {
    shared: Arc<SharedState>,
    handle: tokio::task::JoinHandle<()>,
    last_sample: Instant,
    last_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SortCol {
    Name,
    Size,
    Status,
    Speed,
    Eta,
    Date,
}

impl SortCol {
    fn key(&self) -> &'static str {
        match self {
            SortCol::Name => "name",
            SortCol::Size => "size",
            SortCol::Status => "status",
            SortCol::Speed => "speed",
            SortCol::Eta => "eta",
            SortCol::Date => "date",
        }
    }
    fn from_key(s: &str) -> Option<SortCol> {
        match s {
            "name" => Some(SortCol::Name),
            "size" => Some(SortCol::Size),
            "status" => Some(SortCol::Status),
            "speed" => Some(SortCol::Speed),
            "eta" => Some(SortCol::Eta),
            "date" => Some(SortCol::Date),
            _ => None,
        }
    }
    fn title(&self) -> &'static str {
        match self {
            SortCol::Name => "Name",
            SortCol::Size => "Size",
            SortCol::Status => "Status",
            SortCol::Speed => "Speed",
            SortCol::Eta => "Time Left",
            SortCol::Date => "Date Added",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolId {
    AddUrl,
    StartQueue,
    StopQueue,
    StopAll,
    Queues,
    Settings,
    Grabber,
}

/// Manual hit-test targets. Layout records (kind, rect) every frame;
/// `process_hits` matches pointer state against them in reverse
/// (topmost first) with click consumption. Never touches egui hit-test.
#[derive(Clone)]
enum HitKind {
    Tool(ToolId),
    MenuBtn(u8), // 0 File, 1 Tasks, 2 Tools, 3 View, 4 Help
    MenuItem(u8), // action code (1..14, 20..22, 30..33)
    SideRow(String),
    RowCheck(u64),
    Row(u64),
    HdrCol(SortCol),
    HdrCheck,
    ResizeCol(SortCol),
    CtxItem(RowAct, u64),
    Btn(ButtonId),
    Check(CheckId),
    Slider(SliderId, f64, f64, bool), // id, min, max, log
    Field(&'static str),
    ComboAnchor,
    DetTab(u8), // details Info/Settings tabs
    DetParts,   // details Part-Info collapse toggle
}

/// Active manual divider drag: table column resize (window edges are the
/// OS's job now that the app uses native decorations).
enum ResizeCap {
    Col {
        col: SortCol,
        start_x: f32,
        start_w: f32,
    },
}

#[derive(Clone, Copy)]
enum RowAct {
    Resume,
    Redownload,
    Pause,
    Remove,
    RemoveFile,
    OpenFolder,
    OpenFile,
    CopyUrl,
    Details,
    VerifyChecksum,
}

pub struct CraftApp {
    rt: Arc<tokio::runtime::Runtime>,
    downloads: Vec<DownloadItem>,
    tasks: HashMap<u64, ActiveTask>,
    next_id: u64,
    log: Vec<String>,
    filter: String,
    search: String,
    sort_col: SortCol,
    sort_asc: bool,
    show_add: bool,
    show_queues: bool,
    show_about: bool,
    add_url: String,
    add_dir: String,
    add_file: String,
    add_conns: usize,
    connections_default: usize,
    speed_limit_kbps: u64,
    scheduler_enabled: bool,
    scheduler_time: String,
    scheduler_started: bool,
    clipboard_rx: std::sync::mpsc::Receiver<String>,
    clipboard_toast: Option<String>,
    browser_msg: String,
    pending_browser_install: Option<String>,
    /// mtime of ui.json at our last load/save — a newer stamp means the
    /// standalone Settings window wrote, so we reload (no restart needed).
    last_ui_mtime: std::cell::Cell<Option<std::time::SystemTime>>,
    last_inbox_check: Instant,
    last_prog_write: Instant,
    /// Every queue id ever seen this session (seeded from disk at load).
    /// Lets `save_queue` tell foreign additions (merge) apart from our own
    /// deletions (drop) when another process changed the file meanwhile.
    known_ids: std::collections::HashSet<u64>,
    col_order: Vec<SortCol>,
    col_w: std::collections::HashMap<SortCol, f32>,
    // ---- power features ----
    max_concurrent: usize,
    /// Pop the standalone "Download complete" dialog when a download finishes.
    /// Its own "Don't show this dialog again" checkbox clears this.
    show_complete: bool,
    queue_running: bool,
    had_active: bool,
    timeout_secs: u64,
    max_retries: u32,
    user_agent: String,
    proxy_url: String,
    antivirus_cmd: String,
    finish_action: String,
    category_folders: bool,
    close_to_tray: bool,
    theme: String,
    theme_applied: String,
    tray: Option<tray_icon::TrayIcon>,
    details_id: Option<u64>,
    details_tab: u8, // 0 info, 1 settings
    probing: bool,
    probe_size: Option<u64>,
    probe_name: Option<String>,
    probe_done_for: String,
    probe_rx: Option<std::sync::mpsc::Receiver<(Option<u64>, Option<String>)>>,
    add_adv: bool,
    add_checksum: String,
    add_overwrite: bool,
    verify_jobs: Vec<(u64, std::sync::mpsc::Receiver<anyhow::Result<String>>)>,
    // ---- P3: site grabber ----
    show_grabber: bool,
    grab_url: String,
    grab_preset: usize,
    grab_exts: String,
    grab_depth: bool,
    grab_busy: bool,
    grab_results: Vec<crate::grabber::GrabLink>,
    grab_notice: String,
    grab_rx: Option<std::sync::mpsc::Receiver<Result<Vec<crate::grabber::GrabLink>, String>>>,
    // ---- P3: folder rules + rename templates ----
    folder_rules: Vec<crate::model::FolderRule>,
    rename_template: String,
    // ---- P3: speed windows + update checker ----
    speed_windows: Vec<crate::model::SpeedWindow>,
    speed_windows_on: bool,
    speed_limit_live: std::sync::Arc<std::sync::atomic::AtomicU64>,
    update_check_on: bool,
    update_status: String,
    update_busy: bool,
    update_rx: Option<std::sync::mpsc::Receiver<(String, Option<String>)>>,
    /// Portable-update asset URL from the last check (P5), if any.
    update_asset: Option<String>,
    update_auto_done: bool,
    // ---- P5: per-download speed history (EWMA input + Details graph) ----
    // Last ≤60 half-second samples per download id, pruned with tasks.
    speed_hist: std::collections::HashMap<u64, std::collections::VecDeque<f64>>,
    // TEMPORARY input diagnostics (removed after the hover investigation):
    // ring buffer of recent pointer/selection/action states, flushed to
    // input.log so pointer delivery can be verified without watching.
    diag_lines: Vec<String>,
    diag_last: Instant,
    diag_acts: u64,
    diag_std_hover: u64,
    diag_std_click: u64,
    diag_hits_n: usize,
    // ---- manual pointer routing (bypasses egui hit-testing) ----
    man: ManState,
    hits: Vec<(HitKind, egui::Rect)>,
    man_hover_dbg: String,
    ctx_menu: Option<(u64, egui::Pos2)>,
    drop_menu: Option<u8>,
    finish_open: bool,
    resize_cap: Option<ResizeCap>,
    slider_cap: Option<SliderCap>,
    // Add-dialog advanced fields (kept on self so manual buttons in
    // process_hits can commit without dialog locals).
    add_referer: String,
    add_cookie: String,
    add_user: String,
    add_pass: String,
    /// Preferred quality from the extension menu (used by the size probe so
    /// the Add dialog can show the chosen stream's size, and honored by the
    /// engine at download time).
    add_quality: String,
    // Details-dialog checksum staging (validated live; see Details tab).
    det_sum: String,
    det_for: Option<u64>,
    det_parts_open: bool,
    // Autofocus request for the Add dialog URL field.
    add_focus_req: bool,
    // Recent-folders menu (the path field's caret): open flag + caret rect, so
    // the outside-click test and the render agree.
    add_recent_open: bool,
    add_caret: Option<egui::Rect>,
}

impl CraftApp {
    pub fn new(rt: Arc<tokio::runtime::Runtime>) -> Self {
        let dl_dir = dirs::download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\Downloads".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || clipboard_watch(tx));
        let mut app = Self {
            rt,
            downloads: Vec::new(),
            tasks: HashMap::new(),
            next_id: 1,
            log: vec!["Download Craft ready.".to_string()],
            filter: "All".to_string(),
            search: String::new(),
            sort_col: SortCol::Date,
            sort_asc: false,
            show_add: false,
            show_queues: false,
            show_about: false,
            add_url: String::new(),
            add_dir: dl_dir,
            add_file: String::new(),
            add_conns: 8,
            connections_default: 8,
            speed_limit_kbps: 0,
            scheduler_enabled: false,
            scheduler_time: "02:00".to_string(),
            scheduler_started: false,
            clipboard_rx: rx,
            clipboard_toast: None,
            browser_msg: String::new(),
            pending_browser_install: None,
            last_ui_mtime: std::cell::Cell::new(None),
            last_inbox_check: Instant::now(),
            last_prog_write: Instant::now(),
            known_ids: std::collections::HashSet::new(),
            col_order: vec![
                SortCol::Name,
                SortCol::Size,
                SortCol::Status,
                SortCol::Speed,
                SortCol::Eta,
                SortCol::Date,
            ],
            col_w: Default::default(),
            max_concurrent: 3,
            show_complete: true,
            queue_running: false,
            had_active: false,
            timeout_secs: 60,
            max_retries: 5,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36".to_string(),
            proxy_url: String::new(),
            antivirus_cmd: String::new(),
            finish_action: "nothing".to_string(),
            category_folders: true,
            close_to_tray: false,
            theme: "dark".to_string(),
            theme_applied: String::new(),
            tray: None,
            details_id: None,
            details_tab: 0,
            probing: false,
            probe_size: None,
            probe_name: None,
            probe_done_for: String::new(),
            probe_rx: None,
            add_adv: false,
            add_checksum: String::new(),
            add_overwrite: false,
            verify_jobs: Vec::new(),
            show_grabber: false,
            grab_url: String::new(),
            grab_preset: 0,
            grab_exts: crate::grabber::preset_exts("Mixed"),
            grab_depth: false,
            grab_busy: false,
            grab_results: Vec::new(),
            grab_notice: String::new(),
            grab_rx: None,
            folder_rules: crate::model::default_folder_rules(),
            rename_template: "{name}".to_string(),
            speed_windows: vec![crate::model::SpeedWindow {
                start: "02:00".to_string(),
                end: "06:00".to_string(),
                limit_kbps: 0,
            }],
            speed_windows_on: false,
            speed_limit_live: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
            update_check_on: false,
            update_status: String::new(),
            update_busy: false,
            update_rx: None,
            update_asset: None,
            update_auto_done: false,
            speed_hist: std::collections::HashMap::new(),
            diag_lines: Vec::new(),
            diag_last: Instant::now(),
            diag_acts: 0,
            diag_std_hover: 0,
            diag_std_click: 0,
            diag_hits_n: 0,
            man: ManState::default(),
            hits: Vec::new(),
            man_hover_dbg: "-".to_string(),
            ctx_menu: None,
            drop_menu: None,
            finish_open: false,
            resize_cap: None,
            slider_cap: None,
            add_referer: String::new(),
            add_cookie: String::new(),
            add_user: String::new(),
            add_pass: String::new(),
            add_quality: String::new(),
            det_sum: String::new(),
            det_for: None,
            det_parts_open: true,
            add_focus_req: false,
            add_recent_open: false,
            add_caret: None,
        };
        app.load_queue();
        app.load_ui_state();
        app.last_ui_mtime.set(
            Self::ui_state_path()
                .and_then(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok()),
        );
        // Zero-config browser integration: the native host always points
        // at this binary (silent unless something needed fixing).
        for b in ["chrome", "edge", "brave", "opera", "vivaldi", "firefox"] {
            match ensure_native_host(b) {
                Ok(true) => app.push_log(format!("Browser host ready for {b}.")),
                Err(e) => app.push_log(format!("Browser host ({b}) unavailable: {e}")),
                Ok(false) => {}
            }
        }
        app.poll_inbox();
        app.merge_queue_from_disk();
        app.check_wake();
        crate::model::touch_heartbeat();
        app
    }

    fn push_log(&mut self, s: impl Into<String>) {
        self.log.push(format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            s.into()
        ));
        if self.log.len() > 300 {
            self.log.drain(..self.log.len() - 300);
        }
    }

    fn queue_path() -> Option<std::path::PathBuf> {
        crate::model::config_dir().map(|p| p.join("queue.json"))
    }

    /// Publish live per-download numbers for standalone progress windows
    /// (`progress.json`, several times a second while anything is active;
    /// removed when idle).
    fn write_progress_snapshot(&mut self) {
        let Some(path) = crate::model::config_dir().map(|p| p.join("progress.json")) else {
            return;
        };
        if self.tasks.is_empty() {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            return;
        }
        if self.last_prog_write.elapsed().as_millis() < 100 {
            return;
        }
        self.last_prog_write = Instant::now();
        let mut items = serde_json::Map::new();
        for (id, task) in self.tasks.iter() {
            let speed = self
                .downloads
                .iter()
                .find(|d| d.id == *id)
                .map(|d| d.speed_bps)
                .unwrap_or(0.0);
            let totals = task.shared.part_total.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let dones = task.shared.part_done.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let parts: Vec<[u64; 3]> = totals
                .iter()
                .zip(dones.iter())
                .map(|((s, e), d)| [*s, *e, *d])
                .collect();
            items.insert(
                id.to_string(),
                serde_json::json!({
                    "downloaded": task.shared.downloaded.load(Ordering::SeqCst),
                    "total": if task.shared.has_total.load(Ordering::SeqCst) {
                        Some(task.shared.total.load(Ordering::SeqCst))
                    } else {
                        None
                    },
                    "speed": speed,
                    "resumable": task.shared.resumable.load(Ordering::SeqCst),
                    "parts": parts,
                }),
            );
        }
        if let Ok(json) = serde_json::to_string_pretty(&serde_json::json!({
            "items": serde_json::Value::Object(items)
        })) {
            Self::atomic_write(&path, json.as_bytes());
        }
    }

    /// Run pause/resume/cancel orders left by standalone progress windows.
    fn check_cmd(&mut self) {
        let path = match crate::model::config_dir() {
            Some(p) => p.join("cmd.json"),
            None => return,
        };
        let _guard = crate::model::fs_lock("cmd");
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let _ = std::fs::remove_file(&path); // claim first: at-most-once
        let cmds: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
        for c in cmds {
            let id = c.get("id").and_then(|x| x.as_u64()).unwrap_or(u64::MAX);
            let act = c.get("action").and_then(|x| x.as_str()).unwrap_or("");
            match act {
                "pause" => self.pause(id),
                "resume" => self.resume(id),
                "cancel" => self.cancel_task(id),
                "pause_all" => self.stop_all(),
                "resume_all" => self.start_queue(),
                "limit" => {
                    if let Some(k) = c.get("kbps").and_then(|x| x.as_u64()) {
                        self.speed_limit_kbps = k;
                        self.refresh_speed_limit();
                        self.save_ui_state();
                        self.push_log(format!("Speed limit set to {k} KB/s from a progress window."));
                    }
                }
                "finish" => {
                    if let Some(v) = c.get("value").and_then(|x| x.as_str()) {
                        if ["nothing", "shutdown", "sleep", "hibernate"].contains(&v) {
                            self.finish_action = v.to_string();
                            self.save_ui_state();
                            self.push_log(format!("Finish action set to {v} from a progress window."));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Pop a standalone IDM-style progress window for one download.
    fn spawn_progress(id: u64) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--progress").arg(id.to_string());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000008);
        }
        let _ = cmd.spawn();
    }

    /// Pop the standalone "Download complete" dialog for one finished download.
    fn spawn_complete(id: u64) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--complete").arg(id.to_string());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000008);
        }
        let _ = cmd.spawn();
    }
    fn ui_state_path() -> Option<std::path::PathBuf> {
        crate::model::config_dir().map(|p| p.join("ui.json"))
    }

    /// Launch the standalone Settings window (separate OS process).
    fn open_settings_window() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--settings");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000008);
        }
        let _ = cmd.spawn();
    }

    /// Reload ui.json when the standalone Settings window rewrote it.
    /// Skipped while the Add dialog is open (never clobber typing).
    fn watch_ui_state(&mut self) {
        let Some(path) = Self::ui_state_path() else {
            return;
        };
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return;
        };
        let changed = self.last_ui_mtime.get().map(|t| mtime > t).unwrap_or(false);
        self.last_ui_mtime.set(Some(mtime));
        if changed && !self.show_add {
            self.load_ui_state();
            self.refresh_speed_limit();
            self.push_log("Settings reloaded from disk.");
        }
    }

    fn save_ui_state(&self) {
        if let Some(path) = Self::ui_state_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let order: Vec<&str> = self.col_order.iter().map(|c| c.key()).collect();
            let widths: std::collections::HashMap<&str, f32> = self
                .col_w
                .iter()
                .map(|(c, w)| (c.key(), *w))
                .collect();
            let v = serde_json::json!({
                "columns": order,
                "widths": widths,
                "max_concurrent": self.max_concurrent,
                "show_complete": self.show_complete,
                "timeout_secs": self.timeout_secs,
                "max_retries": self.max_retries,
                "user_agent": self.user_agent,
                "proxy_url": self.proxy_url,
                "antivirus_cmd": self.antivirus_cmd,
                "finish_action": self.finish_action,
                "category_folders": self.category_folders,
                "close_to_tray": self.close_to_tray,
                "theme": self.theme,
                "connections_default": self.connections_default,
                "speed_limit_kbps": self.speed_limit_kbps,
                "save_dir": self.add_dir,
                "folder_rules": self.folder_rules,
                "rename_template": self.rename_template,
                "speed_windows": self.speed_windows,
                "speed_windows_on": self.speed_windows_on,
                "update_check_on": self.update_check_on,
            });
            if let Ok(json) = serde_json::to_string_pretty(&v) {
                Self::atomic_write(&path, json.as_bytes());
                if let Ok(mtime) =
                    std::fs::metadata(&path).and_then(|m| m.modified())
                {
                    self.last_ui_mtime.set(Some(mtime));
                }
            }
        }
    }

    fn load_ui_state(&mut self) {
        if let Some(path) = Self::ui_state_path() {
            if let Ok(data) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    if let Some(arr) = v.get("columns").and_then(|c| c.as_array()) {
                        let mut order = Vec::new();
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                if let Some(c) = SortCol::from_key(s) {
                                    if !order.contains(&c) {
                                        order.push(c);
                                    }
                                }
                            }
                        }
                        for c in [
                            SortCol::Name,
                            SortCol::Size,
                            SortCol::Status,
                            SortCol::Speed,
                            SortCol::Eta,
                            SortCol::Date,
                        ] {
                            if !order.contains(&c) {
                                order.push(c);
                            }
                        }
                        self.col_order = order;
                    }
                    if let Some(w) = v.get("widths").and_then(|x| x.as_object()) {
                        for (k, val) in w {
                            if let (Some(c), Some(f)) =
                                (SortCol::from_key(k), val.as_f64())
                            {
                                self.col_w.insert(c, (f as f32).clamp(col_min(c), 900.0));
                            }
                        }
                    }
                    let g = |k: &str| v.get(k);
                    if let Some(n) = g("max_concurrent").and_then(|x| x.as_u64()) {
                        self.max_concurrent = (n as usize).clamp(1, 16);
                    }
                    if let Some(b) = g("show_complete").and_then(|x| x.as_bool()) {
                        self.show_complete = b;
                    }
                    if let Some(n) = g("timeout_secs").and_then(|x| x.as_u64()) {
                        self.timeout_secs = n.clamp(5, 600);
                    }
                    if let Some(n) = g("max_retries").and_then(|x| x.as_u64()) {
                        self.max_retries = (n as u32).clamp(1, 20);
                    }
                    if let Some(s) = g("user_agent").and_then(|x| x.as_str()) {
                        if !s.is_empty() {
                            self.user_agent = s.to_string();
                        }
                    }
                    if let Some(s) = g("proxy_url").and_then(|x| x.as_str()) {
                        self.proxy_url = s.to_string();
                    }
                    if let Some(s) = g("antivirus_cmd").and_then(|x| x.as_str()) {
                        self.antivirus_cmd = s.to_string();
                    }
                    if let Some(s) = g("finish_action").and_then(|x| x.as_str()) {
                        self.finish_action = s.to_string();
                    }
                    if let Some(b) = g("category_folders").and_then(|x| x.as_bool()) {
                        self.category_folders = b;
                    }
                    if let Some(b) = g("close_to_tray").and_then(|x| x.as_bool()) {
                        self.close_to_tray = b;
                    }
                    if let Some(s) = g("theme").and_then(|x| x.as_str()) {
                        self.theme = s.to_string();
                    }
                    if let Some(n) = g("connections_default").and_then(|x| x.as_u64()) {
                        self.connections_default = (n as usize).clamp(1, 16);
                    }
                    if let Some(n) = g("speed_limit_kbps").and_then(|x| x.as_u64()) {
                        self.speed_limit_kbps = n;
                    }
                    if let Some(s) = g("save_dir").and_then(|x| x.as_str()) {
                        if !s.is_empty() {
                            self.add_dir = s.to_string();
                        }
                    }
                    if let Some(arr) = g("folder_rules").and_then(|x| x.as_array()) {
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
                            self.folder_rules = rules;
                        }
                    }
                    if let Some(s) = g("rename_template").and_then(|x| x.as_str()) {
                        if !s.trim().is_empty() {
                            self.rename_template = s.to_string();
                        }
                    }
                    if let Some(arr) = g("speed_windows").and_then(|x| x.as_array()) {
                        let wins: Vec<crate::model::SpeedWindow> = arr
                            .iter()
                            .filter_map(|r| {
                                Some(crate::model::SpeedWindow {
                                    start: r.get("start")?.as_str()?.to_string(),
                                    end: r.get("end")?.as_str()?.to_string(),
                                    limit_kbps: r.get("limit_kbps")?.as_u64()?,
                                })
                            })
                            .take(6)
                            .collect();
                        if !wins.is_empty() {
                            self.speed_windows = wins;
                        }
                    }
                    if let Some(b) = g("speed_windows_on").and_then(|x| x.as_bool()) {
                        self.speed_windows_on = b;
                    }
                    if let Some(b) = g("update_check_on").and_then(|x| x.as_bool()) {
                        self.update_check_on = b;
                    }
                }
            }
        }
    }

    /// Leftover chunk files for a destination (single + segmented parts).
    fn part_files(dest: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = vec![dest.with_extension("dcpart")];
        for i in 0..16 {
            out.push(dest.with_extension(format!("dcpart{i}")));
        }
        out
    }

    fn redownload(&mut self, id: u64) {
        self.cancel_task(id);
        if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
            let dest = d.dest_path();
            // Replace semantics: drop the previous file so the engine does
            // not auto-rename to `name (1).ext`.
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            for p in Self::part_files(&dest) {
                let _ = std::fs::remove_file(p);
            }
        }
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            d.downloaded_bytes = 0;
            d.total_bytes = None;
            d.speed_bps = 0.0;
            d.status = Status::Queued;
        }
        self.save_queue();
        self.start(id);
    }

    /// Re-queue every failed item and pump the queue. Real action behind
    /// the Tasks-menu / toolbar entry (no placeholder controls).
    fn retry_all_failed(&mut self) {
        let mut n = 0;
        for d in self.downloads.iter_mut() {
            if matches!(d.status, Status::Failed(_)) {
                d.status = Status::Queued;
                n += 1;
            }
        }
        if n > 0 {
            self.queue_running = true;
            self.fill_next();
            self.push_log(format!("Re-queued {n} failed item(s)."));
            self.save_queue();
        } else {
            self.push_log("Nothing failed — nothing to retry.");
        }
    }

    /// Menu/dropdown action codes shared by the menubar and popups.
    /// 1 add, 2 folder, 4/5/6/7 tasks, 8 queues, 9 settings, 10 about,
    /// 11/12 import/export, 13 retry-failed, 14 exit, 20/21/22 themes.
    fn do_menu_act(&mut self, ctx: &egui::Context, act: u8) {
        self.diag_acts += 1;
        match act {
            1 => {
                self.add_url.clear();
                self.add_file.clear();
                self.add_checksum.clear();
                self.add_quality.clear();
                self.show_add = true;
            }
            2 => {
                let _ = std::process::Command::new("explorer")
                    .arg(&self.add_dir)
                    .spawn();
            }
            4 => {
                for id in self.selected_ids() {
                    self.resume(id);
                }
            }
            5 => {
                for id in self.selected_ids() {
                    self.pause(id);
                }
            }
            6 => {
                for id in self.selected_ids() {
                    self.remove(id);
                }
            }
            7 => {
                let ids: Vec<u64> = self
                    .downloads
                    .iter()
                    .filter(|d| matches!(d.status, Status::Completed))
                    .map(|d| d.id)
                    .collect();
                for id in ids {
                    self.remove(id);
                }
            }
            8 => self.show_queues = true,
            9 => Self::open_settings_window(),
            10 => self.show_about = true,
            11 => self.import_list(),
            12 => self.export_list(),
            13 => self.retry_all_failed(),
            14 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            20 | 21 | 22 => {
                self.theme = match act {
                    20 => "dark",
                    21 => "black",
                    _ => "light",
                }
                .to_string();
                self.save_ui_state();
            }
            _ => {}
        }
    }

    /// Dropdown items per menubar menu. `(label, act)`; `("", 0)` renders
    /// a separator row.
    fn menu_items(menu: u8) -> Vec<(String, u8)> {
        match menu {
            0 => vec![
                ("Add URL".into(), 1),
                ("Open download folder".into(), 2),
                (String::new(), 0),
                ("Import URL list (.txt)…".into(), 11),
                ("Export list (.txt)…".into(), 12),
                (String::new(), 0),
                ("Exit".into(), 14),
            ],
            1 => vec![
                ("Start selected".into(), 4),
                ("Pause selected".into(), 5),
                ("Retry all failed".into(), 13),
                ("Remove selected".into(), 6),
                ("Clear finished".into(), 7),
            ],
            2 => vec![("Open Queues".into(), 8), ("Settings".into(), 9)],
            3 => vec![("Dark".into(), 20), ("Black".into(), 21), ("Light".into(), 22)],
            _ => vec![("About".into(), 10)],
        }
    }

    /// Row action dispatcher (driven by process_hits).
    fn do_row_act(&mut self, act: RowAct, id: u64) {
        self.diag_acts += 1;
        match act {
            RowAct::Resume => self.resume(id),
            RowAct::Pause => self.pause(id),
            RowAct::Details => {
                self.details_id = Some(id);
                self.details_tab = 0;
            }
            RowAct::Redownload => self.redownload(id),
            RowAct::Remove => self.remove(id),
            RowAct::RemoveFile => self.remove_file(id),
            RowAct::OpenFolder => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    open_path(&d.dest_path(), true);
                }
            }
            RowAct::OpenFile => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    open_path(&d.dest_path(), false);
                }
            }
            RowAct::CopyUrl => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(d.url.clone());
                    }
                    self.push_log("URL copied to clipboard.");
                }
            }
            RowAct::VerifyChecksum => self.verify_checksum(id),
        }
    }

    /// Row context-menu item definitions: (label, action).
    fn ctx_items() -> Vec<(String, Option<RowAct>)> {
        use RowAct::*;
        vec![
            ("Details".into(), Some(Details)),
            ("".into(), None),
            ("Resume / Retry".into(), Some(Resume)),
            ("Pause".into(), Some(Pause)),
            ("Redownload".into(), Some(Redownload)),
            ("Verify checksum".into(), Some(VerifyChecksum)),
            ("".into(), None),
            ("Copy URL".into(), Some(CopyUrl)),
            ("Open file location".into(), Some(OpenFolder)),
            ("Open file".into(), Some(OpenFile)),
            ("".into(), None),
            ("Remove".into(), Some(Remove)),
            ("Remove + delete file".into(), Some(RemoveFile)),
        ]
    }

    /// On-demand integrity check for a finished file with a stored SHA-256.
    /// Hashing runs on a worker thread; the result is collected in `update()`
    /// so the GUI never blocks.
    fn verify_checksum(&mut self, id: u64) {
        let (dest, expected) = match self.downloads.iter().find(|d| d.id == id) {
            Some(d) => (d.dest_path(), d.checksum_sha256.clone()),
            None => return,
        };
        if expected.trim().is_empty() {
            self.push_log(format!(
                "Verify #{id}: no SHA-256 stored for this item (set one in Details → Settings)."
            ));
            return;
        }
        if !matches!(
            self.downloads.iter().find(|d| d.id == id).map(|d| &d.status),
            Some(Status::Completed)
        ) {
            self.push_log(format!("Verify #{id}: only finished files can be verified."));
            return;
        }
        if self.verify_jobs.iter().any(|(j, _)| *j == id) {
            self.push_log(format!("Verify #{id}: already running."));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::model::sha256_of_file(&dest));
        });
        self.verify_jobs.push((id, rx));
        self.push_log(format!("Verify #{id}: hashing…"));
    }

    /// Collect finished checksum jobs (called every frame).
    fn poll_verify_jobs(&mut self) {
        let mut done: Vec<(u64, anyhow::Result<String>)> = Vec::new();
        self.verify_jobs.retain(|(id, rx)| match rx.try_recv() {
            Ok(r) => {
                done.push((*id, r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        for (id, r) in done {
            let expected = self
                .downloads
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.checksum_sha256.clone())
                .unwrap_or_default();
            match r {
                Ok(actual)
                    if actual.eq_ignore_ascii_case(
                        crate::model::normalize_sha256(&expected).as_str(),
                    ) =>
                {
                    self.push_log(format!("Verify #{id}: OK (SHA-256 matches)."));
                    notify("Download Craft — checksum OK", &format!("#{id} verified"));
                }
                Ok(actual) => {
                    let short = if actual.len() > 16 { &actual[..16] } else { &actual[..] };
                    self.push_log(format!("Verify #{id}: MISMATCH (file starts {short}…)."));
                    notify("Download Craft — checksum mismatch", &format!("#{id} does not match"));
                }
                Err(e) => self.push_log(format!("Verify #{id}: failed ({e:#}).")),
            }
        }
    }

    fn remove_file(&mut self, id: u64) {
        if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
            let dest = d.dest_path();
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            for p in Self::part_files(&dest) {
                let _ = std::fs::remove_file(p);
            }
        }
        self.remove(id);
    }

    /// Final save folder for a file (optionally into a category subfolder).
    fn save_dir_for(&self, filename: &str) -> String {
        if self.category_folders {
            // Ordered user rules first (P3.2), category fallback second.
            if let Some(rule) = self.folder_rules.iter().find(|r| r.matches(filename)) {
                if !rule.folder.trim().is_empty() {
                    let mut p = std::path::PathBuf::from(&self.add_dir);
                    p.push(rule.folder.trim());
                    return p.to_string_lossy().to_string();
                }
            }
            let cat = category_of(filename);
            let mut p = std::path::PathBuf::from(&self.add_dir);
            p.push(cat);
            p.to_string_lossy().to_string()
        } else {
            self.add_dir.clone()
        }
    }

    /// Probe a URL for size + server filename (used by the Add dialog).
    fn start_probe(&mut self, url: String) {
        if !is_download_url(&url) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.probe_rx = Some(rx);
        self.probing = true;
        self.probe_size = None;
        self.probe_name = None;
        let ua = if self.user_agent.trim().is_empty() {
            format!("download_craft/{}", env!("CARGO_PKG_VERSION"))
        } else {
            self.user_agent.clone()
        };
        let opts = EngineOpts {
            user_agent: ua,
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            quality_pref: self.add_quality.trim().to_string(),
            ..Default::default()
        };
        self.rt.spawn(async move {
            let out = match opts.build_client() {
                Ok(client) => {
                    let (mut total, _, ctype, mut disp) =
                        crate::engine::probe(&client, &opts, &url).await;
                    // Page URLs (YouTube watch / generic video pages) answer
                    // HEAD with HTML and no length → resolve the media stream
                    // first so the dialog shows the real size + title (IDM-style).
                    let is_page = ctype
                        .as_deref()
                        .map(|c| c.contains("text/html"))
                        .unwrap_or(false)
                        || url.contains("youtube.com")
                        || url.contains("youtu.be");
                    if total.is_none() && is_page {
                        let shared =
                            std::sync::Arc::new(crate::engine::SharedState::new());
                        if let Some(hit) = crate::engine::resolve_media(&client, &opts, &url, &shared).await
                        {
                            let (t2, _, _, _) =
                                crate::engine::probe(&client, &opts, &hit.url).await;
                            if total.is_none() {
                                total = t2;
                            }
                            // Pair audio adds to the finished size when the
                            // menu choice merges (IDM shows the merged total).
                            if let Some(pair) = hit.pair_url.filter(|p| p.starts_with("http")) {
                                let (ta, _, _, _) =
                                    crate::engine::probe(&client, &opts, &pair).await;
                                if let (Some(a), Some(b)) = (total, ta) {
                                    total = Some(a.saturating_add(b));
                                } else {
                                    total = total.or(ta);
                                }
                            }
                            if disp.is_none() && !hit.title.trim().is_empty() {
                                disp = Some(crate::engine::sanitize_filename(&hit.title));
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
    /// Start a site-grabber crawl on the Tokio runtime (P3.1).
    fn start_grab(&mut self) {
        let url = self.grab_url.trim().to_string();
        if !is_download_url(&url) {
            self.grab_notice = "Need an http(s) page URL.".to_string();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.grab_rx = Some(rx);
        self.grab_busy = true;
        self.grab_results.clear();
        self.grab_notice.clear();
        let exts = crate::grabber::parse_ext_list(&self.grab_exts);
        let depth_1 = self.grab_depth;
        let ua = if self.user_agent.trim().is_empty() {
            format!("download_craft/{}", env!("CARGO_PKG_VERSION"))
        } else {
            self.user_agent.clone()
        };
        let opts = EngineOpts {
            user_agent: ua,
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            ..Default::default()
        };
        self.rt.spawn(async move {
            let out = match opts.build_client() {
                Ok(client) => crate::grabber::grab_site(&client, &opts, &url, &exts, depth_1, crate::grabber::MAX_PAGES_DEFAULT)
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(out);
        });
    }

    /// Queue all grabber results through the same dedupe + dest-clash
    /// refresh path as every other capture (P4): re-grabs refresh stale
    /// links instead of colliding, and the queue starts when running.
    fn queue_grab_results(&mut self) {
        if self.grab_results.is_empty() {
            self.grab_notice = "Grab a page first.".to_string();
            return;
        }
        let mut n = 0;
        let results = std::mem::take(&mut self.grab_results);
        for l in results {
            let fname = crate::model::apply_rename_template(&self.rename_template, &l.name, &l.url);
            if self.queue_url(l.url, Some(fname), false, None) {
                n += 1;
            }
        }
        if self.queue_running {
            self.fill_next();
        }
        self.push_log(format!("Grabber queued {n} link(s)."));
        self.grab_notice.clear();
    }

    /// Recompute the live scheduler cap from speed windows (P3.4).
    /// u64::MAX = no window active (per-download caps apply as usual).
    fn refresh_speed_limit(&self) {        let v = if self.speed_windows_on {
            let now = chrono::Local::now();
            let now_min = (now.format("%H").to_string().parse::<u16>().unwrap_or(0)) * 60
                + now.format("%M").to_string().parse::<u16>().unwrap_or(0);
            self.speed_windows
                .iter()
                .find(|w| w.active_at(now_min))
                .map(|w| w.limit_kbps * 1024)
                .unwrap_or(u64::MAX)
        } else {
            u64::MAX
        };
        self.speed_limit_live.store(v, Ordering::SeqCst);
    }

    /// One-line status for the Speed windows section.
    fn speed_window_status(&self) -> String {
        if !self.speed_windows_on {
            return "Windows off — global limit applies.".to_string();
        }
        let now = chrono::Local::now();
        let now_min = now.format("%H").to_string().parse::<u16>().unwrap_or(0) * 60
            + now.format("%M").to_string().parse::<u16>().unwrap_or(0);
        match self.speed_windows.iter().find(|w| w.active_at(now_min)) {
            Some(w) if w.limit_kbps == 0 => {
                format!("Active now: {}–{} full speed.", w.start, w.end)
            }
            Some(w) => format!("Active now: {}–{} at {} KB/s.", w.start, w.end, w.limit_kbps),
            None => "No window active right now.".to_string(),
        }
    }

    /// One-shot opt-in update check against GitHub releases (P3.4, P5 asset).
    fn start_update_check(&mut self) {
        if self.update_busy {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_busy = true;
        self.update_status.clear();
        self.update_asset = None;
        self.rt.spawn(async move {
            let _ = tx.send(update_check_async().await);
        });
    }

    /// Queue the portable update asset as a normal download (P5).
    /// Never self-replaces: the .zip lands in Downloads and the log +
    /// notification tell the user where to unzip it.
    fn download_update_asset(&mut self) {
        let Some(url) = self.update_asset.clone() else {
            self.push_log("No update asset known — check for updates first.");
            return;
        };
        let fname = url
            .split(['?', '#'])
            .next()
            .unwrap_or(&url)
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "download_craft_update.zip".to_string());
        if self.queue_url(url, Some(fname.clone()), true, None) {
            self.push_log(format!("Downloading update: {fname}"));
            notify("Download Craft — update", &format!("Downloading {fname}"));
        } else {
            self.push_log("Update already queued.");
        }
    }

    /// Queue one URL programmatically (used by inbox + import).
    /// Returns true when the URL was newly queued or an existing same-file
    /// entry was refreshed (P4 grab-all counts on this).
    fn queue_url(&mut self, url: String, filename: Option<String>, autostart: bool, size: Option<u64>) -> bool {
        if !is_download_url(&url) {
            return false;
        }
        // Same URL already tracked. A live row is left alone; a dead row
        // (failed, or finished with its file gone) is refreshed so a
        // re-capture is never silently dropped.
        if let Some(pos) = self.downloads.iter().position(|d| d.url == url) {
            let dead = match &self.downloads[pos].status {
                Status::Failed(_) => true,
                Status::Completed => !self.downloads[pos].dest_path().exists(),
                _ => false,
            };
            if !dead {
                return false;
            }
            let id = self.downloads[pos].id;
            let fname = self.downloads[pos].filename.clone();
            if let Some(s) = size.filter(|s| *s > 0) {
                self.downloads[pos].total_bytes = Some(s);
            }
            self.downloads[pos].downloaded_bytes = 0;
            self.downloads[pos].speed_bps = 0.0;
            self.downloads[pos].status = Status::Queued;
            let dp = self.downloads[pos].dest_path();
            crate::model::drop_resume_state(&dp);
            self.push_log(format!("Re-queued: {fname}"));
            if autostart {
                self.start(id);
            }
            self.save_queue();
            return true;
        }
        let fname = filename
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| filename_from_url(&url));
        let fname = crate::model::apply_rename_template(&self.rename_template, &fname, &url);
        let dir = self.save_dir_for(&fname);
        let dest = std::path::Path::new(&dir).join(&fname);
        // Same file, fresh URL (re-capture): refresh in place instead of a
        // colliding duplicate (two tasks on one dest fail loudly and can
        // corrupt resume state across different URLs).
        if let Some(pos) = crate::model::find_dest_clash(&self.downloads, &dest) {
            let id = self.downloads[pos].id;
            if self.downloads[pos].url != url {
                self.downloads[pos].url = url;
                self.downloads[pos].downloaded_bytes = 0;
                crate::model::drop_resume_state(&dest);
            }
            self.downloads[pos].total_bytes = size.filter(|s| *s > 0);
            self.downloads[pos].speed_bps = 0.0;
            self.downloads[pos].status = Status::Queued;
            self.push_log(format!("Refreshed link: {fname}"));
            if autostart {
                self.start(id);
            }
            self.save_queue();
            return true;
        }
        let id = self.alloc_id();
        let mut item = DownloadItem::new(
            id,
            url,
            fname,
            dir,
            self.connections_default,
        );
        // Browser-measured size survives queueing (see confirm::commit).
        item.total_bytes = size.filter(|s| *s > 0);
        self.downloads.push(item);
        if autostart {
            self.start(id);
        }
        self.save_queue();
        true
    }

    fn import_list(&mut self) {
        let file = rfd::FileDialog::new()
            .add_filter("Text", &["txt", "list", "urls"])
            .set_title("Import URL list")
            .pick_file();
        let Some(path) = file else { return };
        let Ok(data) = std::fs::read_to_string(&path) else {
            self.push_log("Import failed: could not read that file.");
            return;
        };
        let mut n = 0;
        for line in data.lines() {
            let t = line.trim().trim_matches(|c| c == '"' || c == '\'');
            if is_download_url(t) && !self.downloads.iter().any(|d| d.url == t) {
                self.queue_url(t.to_string(), None, false, None);
                n += 1;
            }
        }
        if n > 0 {
            self.queue_running = true;
            self.fill_next();
        }
        self.push_log(format!("Imported {n} URL(s) from list."));    }

    fn export_list(&mut self) {
        let file = rfd::FileDialog::new()
            .add_filter("Text", &["txt"])
            .set_file_name("download_craft_urls.txt")
            .set_title("Export URL list")
            .save_file();
        let Some(path) = file else { return };
        let body: String = self
            .downloads
            .iter()
            .map(|d| d.url.clone())
            .collect::<Vec<_>>()
            .join("\n");
        match std::fs::write(&path, body) {
            Ok(()) => self.push_log(format!("Exported to {}.", path.display())),
            Err(e) => self.push_log(format!("Export failed: {e}")),
        }
    }

    /// Atomically write `data` to `path` (temp + rename).
    fn atomic_write(path: &std::path::Path, data: &[u8]) {
        let _ = crate::model::atomic_write(path, data);
    }

    fn save_queue(&mut self) {
        let _guard = crate::model::fs_lock("queue");
        let Some(path) = Self::queue_path() else {
            return;
        };
        let mut items = self.downloads.clone();
        for it in &mut items {
            it.speed_bps = 0.0;
        }
        // Union with foreign additions made since our last sync (confirm /
        // basket / CLI). Our own deletions stay deleted: ids we have ever
        // seen but no longer hold are dropped, unknown ids are merged in.
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(disk) = serde_json::from_str::<Vec<DownloadItem>>(&data) {
                for it in disk {
                    if items.iter().any(|d| d.id == it.id || d.url == it.url) {
                        continue;
                    }
                    if self.known_ids.contains(&it.id) {
                        continue; // deleted here; do not resurrect
                    }
                    let mut it = it;
                    it.speed_bps = 0.0;
                    it.selected = false;
                    items.push(it);
                }
            }
        }
        for it in &items {
            self.known_ids.insert(it.id);
        }
        if let Some(m) = items.iter().map(|d| d.id).max() {
            self.next_id = self.next_id.max(m + 1);
        }
        if let Ok(json) = serde_json::to_string_pretty(&items) {
            Self::atomic_write(&path, json.as_bytes());
        }
    }

    /// Fresh id that cannot collide with other processes' additions: refresh
    /// from disk under the queue lock, then take max+1.
    fn alloc_id(&mut self) -> u64 {
        let _guard = crate::model::fs_lock("queue");
        if let Some(path) = Self::queue_path() {
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(disk) = serde_json::from_str::<Vec<DownloadItem>>(&data) {
                    for it in &disk {
                        self.known_ids.insert(it.id);
                        self.next_id = self.next_id.max(it.id + 1);
                    }
                }
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.known_ids.insert(id);
        id
    }

    /// Parse queue items tolerantly: one bad entry (schema drift, partial
    /// write) must not nuke the whole queue. Returns (good, had_bad).
    fn parse_queue_items(data: &str) -> (Vec<DownloadItem>, bool) {
        if let Ok(items) = serde_json::from_str::<Vec<DownloadItem>>(data) {
            return (items, false);
        }
        let mut good = Vec::new();
        let mut bad = false;
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(data) {
            for v in arr {
                match serde_json::from_value::<DownloadItem>(v) {
                    Ok(it) => good.push(it),
                    Err(_) => bad = true,
                }
            }
        } else {
            bad = true;
        }
        (good, bad)
    }

    fn quarantine_queue_file(data: &str) {
        if let Some(dir) = crate::model::config_dir() {
            let name = format!("queue.corrupt-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S"));
            let _ = std::fs::write(dir.join(name), data);
        }
    }

    fn load_queue(&mut self) {
        if let Some(path) = Self::queue_path() {
            if let Ok(data) = std::fs::read_to_string(path) {
                let (items, had_bad) = Self::parse_queue_items(&data);
                if had_bad {
                    Self::quarantine_queue_file(&data);
                    self.push_log("Queue file had unreadable entries — kept the good ones, quarantined the rest.");
                }
                for mut it in items {
                        if it.status.is_active() {
                            it.status = Status::Queued;
                        }
                        // drop phantom sizes from failed zero-byte transfers
                        if it.downloaded_bytes == 0 {
                            it.total_bytes = None;
                        }
                        // past zero-byte "completions" with no file were phantom successes
                        if matches!(it.status, Status::Completed)
                            && it.downloaded_bytes == 0
                            && !it.dest_path().exists()
                        {
                            it.status =
                                Status::Failed("empty result — resume to retry".into());
                        }
                        it.speed_bps = 0.0;
                        it.selected = false;
                        self.next_id = self.next_id.max(it.id + 1);
                        self.known_ids.insert(it.id);
                        self.downloads.push(it);
                    }
                    self.push_log(format!("Loaded {} item(s).", self.downloads.len()));
                }
            }
        }

    fn start(&mut self, id: u64) {
        let idx = match self.downloads.iter().position(|d| d.id == id) {
            Some(i) => i,
            None => return,
        };
        if self.tasks.contains_key(&id) {
            return;
        }
        // IDM-style: a download that begins gets its own progress window, no
        // matter which path started it (Add dialog, capture popup, Redownload,
        // Resume, Start Queue, retry-all). The window claims a per-download
        // slot, so a start while one is already open never stacks duplicates,
        // while a window the user closed comes straight back on Retry.
        Self::spawn_progress(id);
        let item = self.downloads[idx].clone();
        let shared = Arc::new(SharedState::new());
        if let Some(t) = item.total_bytes {
            shared.total.store(t, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
        shared
            .downloaded
            .store(item.downloaded_bytes, Ordering::SeqCst);
        let url = item.url.clone();
        let dest = item.dest_path();
        let conns = item.connections;
        let opts = EngineOpts {
            speed_limit_bps: self.speed_limit_kbps * 1024,
            timeout_secs: item.eff_timeout(self.timeout_secs),
            max_retries: item.eff_retries(self.max_retries),
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", env!("CARGO_PKG_VERSION"))
            } else {
                self.user_agent.clone()
            },
            referer: item.referer.clone(),
            cookie: item.cookie.clone(),
            username: item.username.clone(),
            password: item.password.clone(),
            proxy_url: self.proxy_url.clone(),
            allow_rename: true,
            expected_sha256: item.checksum_sha256.clone(),
            overwrite_existing: item.overwrite_existing,
            quality_pref: item.quality_pref.clone(),
            global_limit: Some(self.speed_limit_live.clone()),
        };
        let shared2 = shared.clone();
        // Adaptive pairs (video-only + audio) merge via ffmpeg on completion.
        let pair = if item.pair_url.starts_with("http") {
            Some(item.pair_url.clone())
        } else {
            None
        };
        let handle = self.rt.spawn(async move {
            let r = match pair {
                Some(a) => {
                    crate::engine::download_adaptive(url, Some(a), dest, conns, opts, shared2.clone()).await
                }
                None => download(url, dest, conns, opts, shared2.clone()).await,
            };
            if let Err(e) = r {
                shared2.set_error(format!("{e:#}"));
                eprintln!("download failed: {e:#}");
            }
        });
        self.tasks.insert(
            id,
            ActiveTask {
                shared,
                handle,
                last_sample: Instant::now(),
                last_bytes: self.downloads[idx].downloaded_bytes,
            },
        );
        self.downloads[idx].status = Status::Downloading;
        self.downloads[idx].speed_bps = 0.0;
        self.had_active = true;
        self.push_log(format!("Started: {}", item.filename));
        self.save_queue();
    }

    /// Start queued items while under the concurrent limit.
    /// Failed items are NOT picked up here: retrying them in a hot loop
    /// spams failures forever (a failed YouTube bot-check would log
    /// endlessly). The user retries explicitly via Resume/Retry or
    /// Tasks → Retry all failed.
    fn fill_next(&mut self) {
        if !self.queue_running {
            return;
        }
        loop {
            let active = self.tasks.len();
            if active >= self.max_concurrent.max(1) {
                return;
            }
            let next = self
                .downloads
                .iter()
                .find(|d| matches!(d.status, Status::Queued))
                .map(|d| d.id);
            match next {
                Some(id) => self.start(id),
                None => return,
            }
        }
    }

    fn pause(&mut self, id: u64) {
        if let Some(t) = self.tasks.get(&id) {
            t.shared.paused.store(true, Ordering::SeqCst);
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                d.status = Status::Paused;
            }
            self.push_log(format!("Paused #{id}"));
            self.save_queue();
        } else if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            // Taskless (queued) items pause directly so the button never
            // looks dead; the pump skips Paused.
            if matches!(d.status, Status::Queued) {
                d.status = Status::Paused;
                self.push_log(format!("Paused #{id}"));
                self.save_queue();
            }
        }
    }

    fn resume(&mut self, id: u64) {
        if let Some(t) = self.tasks.get(&id) {
            t.shared.paused.store(false, Ordering::SeqCst);
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                d.status = Status::Downloading;
            }
            // The window may have been closed while the transfer sat paused:
            // reopening is cheap and a live one is left alone by its slot.
            Self::spawn_progress(id);
            self.push_log(format!("Resumed #{id}"));
            self.save_queue();
        } else if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
            if matches!(d.status, Status::Paused | Status::Queued | Status::Failed(_)) {
                if let Some(dd) = self.downloads.iter_mut().find(|d| d.id == id) {
                    if matches!(dd.status, Status::Failed(_)) {
                        dd.status = Status::Queued;
                    }
                }
                self.start(id);
            }
        }
    }

    fn cancel_task(&mut self, id: u64) {
        if let Some(t) = self.tasks.remove(&id) {
            t.shared.cancelled.store(true, Ordering::SeqCst);
            t.handle.abort();
        }
        // Reflect the stop immediately (the finish handler would only get
        // here on its next poll): a cancelled transfer rests Paused with
        // its partial state kept for resume.
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            if !matches!(d.status, Status::Completed) {
                d.status = Status::Paused;
            }
        }
        self.save_queue();
    }

    fn remove(&mut self, id: u64) {
        self.cancel_task(id);
        self.downloads.retain(|d| d.id != id);
        self.push_log(format!("Removed #{id}"));
        self.save_queue();
    }

    fn selected_ids(&self) -> Vec<u64> {
        self.downloads.iter().filter(|d| d.selected).map(|d| d.id).collect()
    }

    fn start_queue(&mut self) {
        self.queue_running = true;
        // Resume paused ones directly, but never above the concurrent limit:
        // paused items with live tasks count, the rest stay queued for
        // fill_next (previously ALL paused resumed at once, ignoring the cap).
        let cap = self.max_concurrent.max(1);
        let ids: Vec<u64> = self
            .downloads
            .iter()
            .filter(|d| matches!(d.status, Status::Paused))
            .map(|d| d.id)
            .collect();
        for id in ids {
            if self.tasks.len() >= cap {
                break;
            }
            self.resume(id);
        }
        self.fill_next();
        self.push_log("Queue started.");
    }

    /// Stop starting new downloads; active transfers run to completion.
    fn stop_queue(&mut self) {
        self.queue_running = false;
        self.push_log("Queue stopped (active downloads finish).");
    }

    /// Pause everything right now and stop the queue pump.
    fn stop_all(&mut self) {
        self.queue_running = false;
        let ids: Vec<u64> = self.tasks.keys().copied().collect();
        for id in ids {
            self.pause(id);
        }
        self.push_log("All downloads paused.");
    }

    fn poll_tasks(&mut self) {
        let mut finished = Vec::new();
        let mut retry_notes = Vec::new();
        for (id, task) in self.tasks.iter_mut() {
            let dl = task.shared.downloaded.load(Ordering::SeqCst);
            let resumable = task.shared.resumable.load(Ordering::SeqCst);
            if let Ok(mut g) = task.shared.resolved_name.lock() {
                if let Some(name) = g.take() {
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.id == *id) {
                        if d.status != Status::Completed {
                            d.filename = name;
                        }
                    }
                }
            }
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == *id) {
                // Displayed progress is strictly monotonic: engine counters
                // reset on retry/fallback legs (segment drop → resume from a
                // lower offset), but the bar must never jump backwards
                // (e.g. 35% -> 20% -> 31%). A changed total means a new leg —
                // adopt both fresh values; otherwise keep the max.
                let new_total = if task.shared.has_total.load(Ordering::SeqCst) {
                    Some(task.shared.total.load(Ordering::SeqCst))
                } else {
                    None
                };
                if new_total != d.total_bytes {
                    // Fresh link / quality fallback: the old bytes belong to
                    // deleted work files, so restart the bar instead of
                    // showing phantom progress — and say so in the log.
                    if d.downloaded_bytes > 0 && dl < d.downloaded_bytes {
                        retry_notes.push(format!(
                            "Retrying {} with a fresh link — restarting progress.",
                            d.filename
                        ));
                    }
                    d.total_bytes = new_total;
                    d.downloaded_bytes = dl;
                } else {
                    d.downloaded_bytes = d.downloaded_bytes.max(dl);
                }
                // Clamp to [0, total] so a stale counter can never exceed 100%.
                if let Some(t) = d.total_bytes {
                    if d.downloaded_bytes > t {
                        d.downloaded_bytes = t;
                    }
                }
                d.resumable = resumable;
                let el = task.last_sample.elapsed().as_secs_f64();
                if el >= 0.25 {
                    // Speed from the MONOTONIC displayed value (never the raw
                    // engine counter): a leg reset would otherwise show a
                    // negative spike followed by an inflated catch-up burst.
                    // P5: EWMA smoothing (α=0.55) so the readout and ETA stop
                    // flickering on bursty CDNs while still reacting quickly
                    // (a quarter-second cadence reads as live); raw samples
                    // feed the graph.
                    let delta = d.downloaded_bytes.saturating_sub(task.last_bytes) as f64;
                    let raw = if el > 0.0 { delta / el } else { 0.0 };
                    d.speed_bps = if d.speed_bps <= 0.0 {
                        raw
                    } else {
                        0.55 * raw + 0.45 * d.speed_bps
                    };
                    let hist = self.speed_hist.entry(*id).or_default();
                    hist.push_back(raw);
                    while hist.len() > 120 {
                        hist.pop_front();
                    }
                    task.last_sample = Instant::now();
                    task.last_bytes = d.downloaded_bytes;
                }
            }
            if task.handle.is_finished() {
                finished.push(*id);
            }
        }
        for note in retry_notes {
            self.push_log(note);
        }
        let mut repair_restart: Vec<u64> = Vec::new();
        for id in finished {
            if let Some(task) = self.tasks.remove(&id) {
                let cancelled = task.shared.cancelled.load(Ordering::SeqCst);
                let dl = task.shared.downloaded.load(Ordering::SeqCst);
                let was_resumable = task.shared.resumable.load(Ordering::SeqCst);
                let has_total = task.shared.has_total.load(Ordering::SeqCst);
                let total = task.shared.total.load(Ordering::SeqCst);
                let mut msg: Option<String> = None;
                let mut finished_info: Option<(String, std::path::PathBuf)> = None;
                // Set when the checksum repair below re-queues this id.
                let mut repaired_here = false;
                if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                    d.downloaded_bytes = dl;
                    d.resumable = was_resumable;
                    if has_total {
                        d.total_bytes = Some(total);
                    }
                    d.speed_bps = 0.0;
                    // Bytes the engine *fetched* are not proof the file landed:
                    // a re-fetched/overlapping segment can push the counter to
                    // the total while the assembled file is still short, and a
                    // failed/renamed write leaves nothing behind. "Finished"
                    // must mean the bytes are actually on disk.
                    // Merged (video+audio) deliveries are exempt: muxing
                    // rewrites the container, so sizes are not comparable.
                    let landed = std::fs::metadata(d.dest_path())
                        .map(|m| m.len())
                        .unwrap_or(0);
                    let adaptive = d.pair_url.starts_with("http");
                    let complete = match d.total_bytes {
                        Some(t) if adaptive => t > 0 && dl >= t,
                        Some(t) => t > 0 && dl >= t && landed >= t,
                        // unknown size: only a clean end WITH bytes counts
                        None => !cancelled && dl > 0,
                    };
                    // Snapshot what follow-up actions need (borrow ends at block end).
                    // A pre-existing file only counts when the size was never
                    // known AND this run actually wrote bytes (dl > 0): a
                    // shortfall against a KNOWN total must stay Failed, or a
                    // truncated retry over an old finished file masks as success.
                    // (Covers empty files too.)
                    let file_done = !cancelled
                        && !complete
                        && d.total_bytes.is_none()
                        && dl > 0
                        && d.dest_path().exists();
                    if cancelled {
                        d.status = Status::Paused;
                    } else if complete || file_done {
                        d.status = Status::Completed;
                        msg = Some(format!("Finished: {}", d.filename));
                        finished_info = Some((d.filename.clone(), d.dest_path()));
                    } else {
                        let reason = task
                            .shared
                            .error
                            .lock()
                            .map(|mut g| g.take())
                            .unwrap_or_default()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| {
                                // Include landed vs expected bytes so a cut
                                // connection (e.g. throttled FB CDN) tells the
                                // user exactly where resume continues from.
                                match d.total_bytes {
                                    Some(t) if t > 0 && dl >= t => format!(
                                        "transfer reported {} but only {} is on disk — resume to retry",
                                        crate::model::format_bytes(dl),
                                        crate::model::format_bytes(landed)
                                    ),
                                    Some(t) if t > 0 => format!(
                                        "connection ended early (got {} of {}) — resume to retry",
                                        crate::model::format_bytes(dl),
                                        crate::model::format_bytes(t)
                                    ),
                                    _ => "connection ended early — resume to retry".to_string(),
                                }
                            });
                        // P5 checksum auto-repair: one automatic from-scratch
                        // redownload per checksum. A mismatch means the bytes
                        // on disk are wrong, so resume state is dropped and
                        // progress restarts; a second mismatch fails loudly
                        // instead of looping forever.
                        let mut repaired = false;
                        if reason.contains("checksum mismatch")
                            && !d.checksum_sha256.trim().is_empty()
                            && !d.checksum_repaired
                        {
                            d.checksum_repaired = true;
                            let dp = d.dest_path();
                            crate::model::drop_resume_state(&dp);
                            d.downloaded_bytes = 0;
                            d.total_bytes = None;
                            d.speed_bps = 0.0;
                            d.status = Status::Queued;
                            msg = Some(format!(
                                "Checksum mismatch on {} — re-downloading once to repair.",
                                d.filename
                            ));
                            repaired = true;
                        }
                        if !repaired {
                            d.status = Status::Failed(reason.clone());
                            msg = Some(format!("Failed #{id}: {reason}"));
                        }
                        // Never keep a phantom size: a failed transfer with no
                        // bytes (e.g. a walled video page) must not display
                        // the probe's bogus total.
                        if dl == 0 {
                            d.total_bytes = None;
                        }
                        if !repaired {
                            notify(
                                "Download Craft — download failed",
                                &format!("#{id}: {reason}"),
                            );
                        } else {
                            repaired_here = true;
                        }
                    }
                }
                if repaired_here {
                    repair_restart.push(id);
                }
                if let Some((name, path)) = finished_info {
                    self.on_download_finished(&name, &path);
                    // IDM pops its "Download complete" dialog for a finished
                    // transfer (the dialog's own checkbox switches it off).
                    if self.show_complete {
                        Self::spawn_complete(id);
                    }
                }
                if let Some(m) = msg {
                    self.push_log(m);
                }
                self.save_queue();
            }
        }
        // Checksum repairs restart immediately (the job was already running;
        // this is an internal retry, not a queue jump).
        for id in repair_restart {
            if !self.tasks.contains_key(&id) {
                self.start(id);
            }
        }
        // P5: drop speed history for rows that no longer exist.
        if self.speed_hist.len() > self.downloads.len() + 4 {
            let live: std::collections::HashSet<u64> =
                self.downloads.iter().map(|d| d.id).collect();
            self.speed_hist.retain(|id, _| live.contains(id));
        }
        self.fill_next();
        self.maybe_finish_action();
    }

    /// Called with an immutable borrow-safe snapshot after a download completes.
    fn on_download_finished(&mut self, name: &str, path: &std::path::Path) {
        notify("Download Craft — download finished", name);
        if !self.antivirus_cmd.trim().is_empty() {
            let cmd = self.antivirus_cmd.replace("{file}", &path.to_string_lossy());
            self.push_log(format!("Antivirus scan: {cmd}"));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("cmd")
                    .arg("/C")
                    .arg(format!("start /min \"\" {cmd}"))
                    .creation_flags(0x08000000)
                    .spawn();
            }
            #[cfg(not(windows))]
            {
                let _ = std::process::Command::new("sh").arg("-c").arg(&cmd).spawn();
            }
        }
    }

    /// Runs the configured end-of-queue action once everything drained.
    /// Only actively-workable items block it (Queued/Downloading). Paused
    /// or failed leftovers no longer veto silently — they get one log line
    /// so the user knows why the action still fired.
    fn maybe_finish_action(&mut self) {
        if !self.had_active || !self.queue_running {
            return;
        }
        if !self.tasks.is_empty() {
            return;
        }
        let pending = self.downloads.iter().any(|d| {
            matches!(d.status, Status::Queued | Status::Downloading)
        });
        if pending {
            return;
        }
        let leftovers = self
            .downloads
            .iter()
            .filter(|d| matches!(d.status, Status::Paused | Status::Failed(_)))
            .count();
        self.queue_running = false;
        self.had_active = false;
        if leftovers > 0 {
            self.push_log(format!(
                "Queue drained with {leftovers} paused/failed item(s) left — finish action still runs."
            ));
        }
        match self.finish_action.as_str() {
            "shutdown" => {
                self.push_log("Queue done — shutting down.");
                let _ = std::process::Command::new("shutdown")
                    .args(["/s", "/t", "15", "/c", "Download Craft: downloads finished"])
                    .spawn();
            }
            "sleep" => {
                self.push_log("Queue done — sleeping.");
                let _ = std::process::Command::new("rundll32.exe")
                    .args(["powrprof.dll,SetSuspendState", "0,1,0"])
                    .spawn();
            }
            "hibernate" => {
                self.push_log("Queue done — hibernating.");
                let _ = std::process::Command::new("shutdown").args(["/h"]).spawn();
            }
            _ => {
                self.push_log("Queue finished.");
            }
        }
        notify("Download Craft — queue finished", "All downloads completed.");
        self.save_queue();
    }

    fn check_scheduler(&mut self) {
        if !self.scheduler_enabled || self.scheduler_started {
            return;
        }
        let now = chrono::Local::now().format("%H:%M").to_string();
        if now == self.scheduler_time.trim() {
            self.scheduler_started = true;
            self.start_queue();
            self.push_log("Scheduler: queue started.");
        }
    }

    /// Import URLs queued by the browser extension's native host.
    /// Unlike auto-starting, the first new item opens the Add-download
    /// dialog prefilled (IDM-style), so the user confirms folder/quality.
    /// Returns true when the window should be brought to the front.
    /// Leftover items stay in the inbox for later polls.
    fn poll_inbox(&mut self) -> bool {
        const MAX_INBOX: usize = 200;
        const CONFIRM_GRACE_SECS: i64 = 10;
        let path = match crate::native_host::inbox_path() {
            Some(p) => p,
            None => return false,
        };
        let _guard = crate::model::fs_lock("inbox");
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return false,
        };
        // Never destroy the inbox on parse failure (disk-full truncation,
        // foreign bytes) — leave it for the next poll/owner.
        let items: Vec<crate::native_host::InboxItem> = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => return false,
        };
        if items.is_empty() {
            let _ = std::fs::remove_file(&path);
            return false;
        }
        // Standalone Download File Info dialogs (spawned per capture, IDM
        // style) claim their inbox entry on open. Entries younger than
        // CONFIRM_GRACE_SECS are left alone so the dialog wins the race;
        // only entries it never claimed (crashed spawn, old version) fall
        // through to this in-canvas prompt.
        let now_ts = chrono::Local::now().timestamp();
        let age_secs = |rfc: &str| -> i64 {
            chrono::DateTime::parse_from_rfc3339(rfc)
                .map(|dt| now_ts - dt.timestamp())
                .unwrap_or(i64::MAX)
        };
        let mut keep: Vec<crate::native_host::InboxItem> = Vec::new();
        let mut next: Option<(String, Option<String>, String, String, Option<u64>, Option<String>)> = None;
        for i in items.into_iter() {
            if next.is_none()
                && is_download_url(&i.url)
                && !self.downloads.iter().any(|d| d.url == i.url)
                && age_secs(&i.received_at) >= CONFIRM_GRACE_SECS
            {
                next = Some((
                    i.url.clone(),
                    i.filename.clone(),
                    i.referer
                        .clone()
                        .or_else(|| i.page_url.clone())
                        .unwrap_or_default(),
                    i.cookie.clone().unwrap_or_default(),
                    i.size,
                    i.quality.clone(),
                ));
            } else if is_download_url(&i.url)
                && !self.downloads.iter().any(|d| d.url == i.url)
            {
                // Young (dialog likely opening) or a later old entry: keep.
                keep.push(i);
            }
            // Invalid URLs and already-queued duplicates are dropped.
        }
        // Cap the inbox (newest first by received_at) so a stuck producer
        // can't grow it without bound.
        if keep.len() > MAX_INBOX {
            keep.sort_by(|a, b| b.received_at.cmp(&a.received_at));
            keep.truncate(MAX_INBOX);
        }
        let write_keep = |keep: &Vec<crate::native_host::InboxItem>| {
            if keep.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if let Ok(json) = serde_json::to_string_pretty(&keep) {
                Self::atomic_write(&path, json.as_bytes());
            }
        };
        let Some((url, filename, referer, cookie, size, quality)) = next else {
            // Nothing actionable now: keep young entries, drop the file only
            // when fully drained.
            write_keep(&keep);
            return false;
        };
        write_keep(&keep);
        if self.show_add {
            return false; // user is typing; retry on a later poll
        }
        self.add_url = url.clone();
        self.add_file = filename
            .filter(|f| !f.trim().is_empty())
            .map(|f| crate::model::apply_rename_template(&self.rename_template, &f, &url))
            .unwrap_or_else(|| filename_from_url(&url));
        self.add_referer = referer;
        self.add_cookie = cookie;
        self.add_checksum.clear();
        self.add_quality = quality.filter(|s| !s.trim().is_empty()).unwrap_or_default();
        // Browser-measured size rides along: show it without a server-side
        // re-probe (which can 403 off-session on expiring CDN links).
        match size.filter(|s| *s > 0) {
            Some(s) => {
                self.probe_size = Some(s);
                self.probe_done_for = url.clone();
            }
            None => self.probe_done_for.clear(), // force a fresh size/name probe
        }
        self.show_add = true;
        self.push_log("Browser extension sent a download — confirm in Add download.");
        true
    }

    /// Merge queue entries written by other processes (standalone Download
    /// File Info dialogs, CLI) into this window's in-memory list.
    fn merge_queue_from_disk(&mut self) {
        let path = match Self::queue_path() {
            Some(p) => p,
            None => return,
        };
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let (items, _) = Self::parse_queue_items(&data);
        for mut it in items {
            if self
                .downloads
                .iter()
                .any(|d| d.id == it.id || d.url == it.url)
            {
                continue;
            }
            // Foreign entry for a file we already track (fresh capture):
            // refresh the local entry instead of a colliding duplicate.
            if let Some(pos) =
                crate::model::find_dest_clash(&self.downloads, &it.dest_path())
            {
                let local = &mut self.downloads[pos];
                if local.url != it.url {
                    local.url = std::mem::take(&mut it.url);
                    local.downloaded_bytes = 0;
                    let dp = local.dest_path();
                    crate::model::drop_resume_state(&dp);
                }
                local.total_bytes = it.total_bytes;
                local.speed_bps = 0.0;
                local.status = Status::Queued;
                continue;
            }
            if it.status.is_active() {
                it.status = Status::Queued;
            }
            it.speed_bps = 0.0;
            it.selected = false;
            self.next_id = self.next_id.max(it.id + 1);
            self.known_ids.insert(it.id);
            self.downloads.push(it);
        }
    }

    /// Consume `%APPDATA%/download_craft/wake.json` (`{"start":[ids]}`):
    /// start IDs queued by a standalone dialog's "Start Download".
    fn check_wake(&mut self) {
        let path = match crate::model::config_dir() {
            Some(p) => p.join("wake.json"),
            None => return,
        };
        let _guard = crate::model::fs_lock("wake");
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let _ = std::fs::remove_file(&path); // claim first: at-most-once
        let ids: Vec<u64> = serde_json::from_str::<serde_json::Value>(&data)
            .ok()
            .and_then(|v| v.get("start").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        self.merge_queue_from_disk();
        for id in ids {
            let ready = self.downloads.iter().any(|d| {
                d.id == id && matches!(d.status, Status::Queued) && !self.tasks.contains_key(&id)
            });
            if ready {
                self.queue_running = true;
                self.start(id);
            }
        }
    }

    fn matches_filter(d: &DownloadItem, filter: &str) -> bool {
        match filter {
            "All" => true,
            // Legacy names (kept for saved state compat) + FluxDown-style names.
            "Finished" | "Completed" => matches!(d.status, Status::Completed),
            "Unfinished" | "Incomplete" => !matches!(d.status, Status::Completed),
            "Failed" => matches!(d.status, Status::Failed(_)),
            "Paused" => matches!(d.status, Status::Paused),
            "Downloading" => matches!(d.status, Status::Downloading),
            "Queued" => matches!(d.status, Status::Queued),
            cat => d.category() == cat,
        }
    }

    fn visible_ids(&self) -> Vec<u64> {
        let q = self.search.trim().to_lowercase();
        let mut ids: Vec<u64> = self
            .downloads
            .iter()
            .filter(|d| Self::matches_filter(d, &self.filter))
            .filter(|d| q.is_empty() || d.filename.to_lowercase().contains(&q))
            .map(|d| d.id)
            .collect();
        let asc = self.sort_asc;
        let col = self.sort_col;
        ids.sort_by(|a, b| {
            let da = self.downloads.iter().find(|d| d.id == *a).unwrap();
            let db = self.downloads.iter().find(|d| d.id == *b).unwrap();
            let ord = match col {
                SortCol::Name => da.filename.to_lowercase().cmp(&db.filename.to_lowercase()),
                SortCol::Size => da.total_bytes.cmp(&db.total_bytes),
                SortCol::Status => format!("{:?}", da.status).cmp(&format!("{:?}", db.status)),
                SortCol::Speed => da.speed_bps.partial_cmp(&db.speed_bps).unwrap_or(std::cmp::Ordering::Equal),
                SortCol::Eta => da.downloaded_bytes.cmp(&db.downloaded_bytes),
                SortCol::Date => da.created_at.cmp(&db.created_at),
            };
            if asc { ord } else { ord.reverse() }
        });
        ids
    }
}

/// Open a file (or reveal it in Explorer when `reveal`).
pub(crate) fn open_path(path: &std::path::Path, reveal: bool) {
    if reveal {
        if path.exists() {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,\"{}\"", path.display()))
                .spawn();
        } else if let Some(parent) = path.parent() {
            let _ = std::process::Command::new("explorer").arg(parent).spawn();
        }
    } else if path.exists() {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();
    }
}

/// Write the native-messaging manifest + launcher and register it (HKCU, no admin).
/// Zero-config: the unpacked extension carries a pinned key, so its ID is
/// always this value — nothing to paste.
pub(crate) const EXTENSION_ID: &str = "pgdhmfgefjnfijmlheboncdmfhcnoikk";

pub(crate) fn install_native_host(browser: &str) -> Result<String, String> {
    let base = crate::model::config_dir()
        .map(|p| p.join("native-host"))
        .ok_or_else(|| "no config dir".to_string())?;
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bat = base.join("download_craft_host.bat");
    // NOTE: `@echo off` is mandatory — cmd echoes every command to stdout
    // by default, which corrupts the length-prefixed native-messaging
    // protocol and makes Chrome report the host as unreachable.
    std::fs::write(
        &bat,
        format!("@echo off\r\n\"{}\" --native-host %*\r\n", exe.display()),
    )
    .map_err(|e| e.to_string())?;

    // Native host name (must match the browser extension's HOST constant).
    const HOST_NAME: &str = "com.download_craft";
    let manifest_path = base.join(format!("{browser}.json"));
    let manifest = match browser {
        "firefox" => serde_json::json!({
            "name": HOST_NAME,
            "description": "Download Craft native host",
            "path": bat.to_string_lossy(),
            "type": "stdio",
            "allowed_extensions": ["send-to-download-craft@example"]
        }),
        _ => serde_json::json!({
            "name": HOST_NAME,
            "description": "Download Craft native host",
            "path": bat.to_string_lossy(),
            "type": "stdio",
            "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")]
        }),
    };
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let key = native_host_key(browser);
    let out = std::process::Command::new("reg")
        .args(["add", key, "/ve", "/d", &manifest_path.to_string_lossy(), "/f"])
        .output()
        .map_err(|e| format!("reg failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    // Best-effort cleanup of the legacy host registration.
    let legacy_key = legacy_host_key(browser);
    let _ = std::process::Command::new("reg")
        .args(["delete", legacy_key, "/f"])
        .output();
    Ok(manifest_path.to_string_lossy().to_string())
}

/// Registry key for a Chromium-family / Firefox native host (HKCU, no admin).
/// Opera / Brave / Vivaldi share the Chromium layout with vendor keys.
fn native_host_key(browser: &str) -> &'static str {
    match browser {
        "chrome" => r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.download_craft",
        "edge" => r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.download_craft",
        "brave" => r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.download_craft",
        "opera" => r"HKCU\Software\Opera Software\NativeMessagingHosts\com.download_craft",
        "vivaldi" => r"HKCU\Software\Vivaldi\NativeMessagingHosts\com.download_craft",
        _ => r"HKCU\Software\Mozilla\NativeMessagingHosts\com.download_craft",
    }
}

fn legacy_host_key(browser: &str) -> &'static str {
    match browser {
        "chrome" => r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.idm_rust",
        "edge" => r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.idm_rust",
        "brave" => r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.idm_rust",
        "opera" => r"HKCU\Software\Opera Software\NativeMessagingHosts\com.idm_rust",
        "vivaldi" => r"HKCU\Software\Vivaldi\NativeMessagingHosts\com.idm_rust",
        _ => r"HKCU\Software\Mozilla\NativeMessagingHosts\com.idm_rust",
    }
}
/// IDM-style zero-config: verify the native host is registered to THIS
/// binary on every launch, reinstalling silently when missing or stale
/// (first run, portable exe moved or renamed, etc.).
/// Returns Ok(true) when something was (re)installed.
/// Best-effort: failures are reported to the caller, never fatal.
fn ensure_native_host(browser: &str) -> Result<bool, String> {
    let base = crate::model::config_dir()
        .map(|p| p.join("native-host"))
        .ok_or_else(|| "no config dir".to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_s = exe.to_string_lossy().into_owned();
    let manifest_path = base.join(format!("{browser}.json"));
    let manifest_s = manifest_path.to_string_lossy().into_owned();
    // Fresh = manifest exists and its launcher targets this exact binary.
    let fresh = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("path")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
        .map(|bat| {
            std::fs::read_to_string(&bat)
                .map(|c| c.contains(exe_s.as_str()))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let key = native_host_key(browser);
    let registered = std::process::Command::new("reg")
        .args(["query", key, "/ve"])
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).contains(manifest_s.as_str())
        })
        .unwrap_or(false);
    if fresh && registered {
        return Ok(false);
    }
    install_native_host(browser)?;
    Ok(true)
}

/// Opt-in update check (P3.4, extended P5): compare the running version
/// against the latest GitHub release tag, and locate the portable Windows
/// asset when one is newer. Network failures become a status string —
/// never a popup, never an auto-download.
async fn update_check_async() -> (String, Option<String>) {
    let current = env!("CARGO_PKG_VERSION");
    let client = match reqwest::Client::builder()
        .user_agent(format!("download_craft/{current}"))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return (format!("Update check failed: {e:#}"), None),
    };
    let resp = client
        .get("https://api.github.com/repos/hungry-detective/downloadcraft/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return (format!("Update check failed (network): {e:#}"), None),
    };
    if !resp.status().is_success() {
        return (format!("Update check failed: HTTP {}", resp.status()), None);
    }
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return (format!("Update check failed (parse): {e:#}"), None),
    };
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
    if tag.is_empty() {
        return ("Update check failed: no releases found.".to_string(), None);
    }
    if crate::model::is_newer_version(current, tag) {
        let url = v
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or("https://github.com/hungry-detective/downloadcraft/releases");
        (
            format!("Update available: {tag} (you have v{current}) — {url}"),
            crate::model::pick_windows_asset(&v),
        )
    } else {
        (format!("Up to date (v{current})."), None)
    }
}

/// Best-effort desktop toast (ignored when unsupported).
fn notify(title: &str, body: &str) {    let _ = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show();
}

/// Build the system tray icon (uses the bundled app icon).
fn build_tray() -> Option<tray_icon::TrayIcon> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let icon = tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()?;
    let menu = tray_icon::menu::Menu::new();
    let show = tray_icon::menu::MenuItem::with_id("tray-show", "Show Download Craft", true, None);
    let exit = tray_icon::menu::MenuItem::with_id("tray-exit", "Exit", true, None);
    menu.append_items(&[&show, &exit]).ok()?;
    tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Download Craft")
        .with_icon(icon)
        .build()
        .ok()
}

fn clipboard_watch(tx: std::sync::mpsc::Sender<String>) {
    let mut last = String::new();
    let mut cb = arboard::Clipboard::new().ok();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let text = cb
            .as_mut()
            .and_then(|c| c.get_text().ok())
            .unwrap_or_default();
        let t = text.trim().to_string();
        if t != last && is_download_url(&t) {
            last = t.clone();
            let _ = tx.send(t);
        }
    }
}

// Column widths.
const CHECK_W: f32 = 32.0;
const SIZE_W: f32 = 90.0;
const STATUS_W: f32 = 130.0;
const SPEED_W: f32 = 90.0;
const ETA_W: f32 = 100.0;
const DATE_W: f32 = 110.0;

impl eframe::App for CraftApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Manual pointer routing: snapshot input, then match layout rects
        // recorded below (process_hits runs after all layout, topmost first).
        self.man.begin(ctx);
        self.hits.clear();
        while let Ok(url) = self.clipboard_rx.try_recv() {
            if !self.downloads.iter().any(|d| d.url == url)
                && self.clipboard_toast.as_deref() != Some(url.as_str())
            {
                self.clipboard_toast = Some(url);
            }
        }

        self.poll_tasks();
        self.poll_verify_jobs();
        self.write_progress_snapshot();
        self.check_scheduler();
        if self.last_inbox_check.elapsed().as_secs() >= 2 {
            self.last_inbox_check = Instant::now();
            crate::model::touch_heartbeat();
            self.watch_ui_state();
            self.merge_queue_from_disk();
            self.check_wake();
            self.check_cmd();
            self.refresh_speed_limit();
            // Opt-in update check, once per session (silent unless newer).
            if self.update_check_on && !self.update_auto_done {
                self.update_auto_done = true;
                self.start_update_check();
            }
            if let Some(rx) = self.update_rx.as_ref() {
                let mut got = None;
                while let Ok(m) = rx.try_recv() {
                    got = Some(m);
                }
                if let Some((m, asset)) = got {
                    self.update_busy = false;
                    self.update_status = m.clone();
                    self.update_asset = asset;
                    if m.starts_with("Update available") {
                        self.push_log(m);
                    }
                }
            }
            if self.poll_inbox() {
                // Extension sent something: pop the app up with the
                // prefilled Add dialog.
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        // Repaint often while a transfer is live so the speed column and the
        // progress windows move in real time; idle, a few times a second is
        // plenty (pointer input repaints immediately on its own).
        let tick_ms = if self.tasks.is_empty() { 250 } else { 100 };
        ctx.request_repaint_after(std::time::Duration::from_millis(tick_ms));

        // theme (re-applied when the user switches it)
        if self.theme_applied != self.theme {
            ctx.set_visuals(visuals_for(&self.theme));
            apply_widget_theme(ctx);
            self.theme_applied = self.theme.clone();
        }

        // system tray
        if self.close_to_tray && self.tray.is_none() {
            self.tray = build_tray();
        }
        if !self.close_to_tray {
            self.tray = None;
        }
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if ev.id() == "tray-show" {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if ev.id() == "tray-exit" {
                self.close_to_tray = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if matches!(
                ev,
                tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                }
            ) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        // close button => minimize to tray instead of quitting (only if tray lives)
        if self.close_to_tray
            && self.tray.is_some()
            && ctx.input(|i| i.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // Native OS window chrome (title bar, min/max/close, drag, resize,
        // snap, taskbar) — a custom-painted titlebar can never match it and
        // is the fastest way to look unprofessional.

        // ================= menu bar + search =================
        // Flat desktop menu bar: manual text buttons (no backgrounds),
        // dropdowns render at the end of update (topmost).
        egui::TopBottomPanel::top("menubar")
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 4,
                bottom: 4,
            }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (m, label) in
                        ["File", "Tasks", "Tools", "View", "Help"].iter().enumerate()
                    {
                        let g = ui.painter().layout_no_wrap(
                            label.to_string(),
                            egui::FontId::proportional(12.5),
                            TXT,
                        );
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(g.size().x + 20.0, 26.0),
                            egui::Sense::hover(),
                        );
                        let hov = self.man.hover(&rect);
                        let open = self.drop_menu == Some(m as u8);
                        if hov || open {
                            ui.painter().rect_filled(rect, 4.0, SURFACE_LT);
                        }
                        ui.painter().galley(
                            egui::pos2(
                                rect.center().x - g.size().x / 2.0,
                                rect.center().y - g.size().y / 2.0,
                            ),
                            g,
                            TXT,
                        );
                        self.hits.push((HitKind::MenuBtn(m as u8), rect));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(SURFACE)
                        .corner_radius(18.0)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 5,
                            bottom: 5,
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                mi(ui, MI_SEARCH, 16.0, DIM);
                                let r = mtext(ui, "search", &mut self.search, "Search in the List", 210.0, false);
                                self.hits.push((HitKind::Field("search"), r));
                            });
                        });
                });
            });
        });
        // Menu/dropdown actions run from process_hits (see do_menu_act).


        // ================= left sidebar (collapsible tree) =================
        egui::SidePanel::left("cats")
            .resizable(false)
            .exact_width(200.0)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                // Status counts (computed once per frame; cheap for typical lists).
                let n_all = self.downloads.len();
                let mut n_done = 0usize;
                let mut n_active = 0usize;
                let mut n_paused = 0usize;
                let mut n_failed = 0usize;
                let mut n_queued = 0usize;
                let mut cat_n: std::collections::HashMap<&str, usize> =
                    std::collections::HashMap::new();
                for d in &self.downloads {
                    *cat_n.entry(d.category()).or_default() += 1;
                    match &d.status {
                        Status::Completed => n_done += 1,
                        Status::Downloading => n_active += 1,
                        Status::Paused => n_paused += 1,
                        Status::Failed(_) => n_failed += 1,
                        Status::Queued => n_queued += 1,
                    }
                }
                let n_incomplete = n_all - n_done;
                let cur = self.filter.clone();
                // (filter key, icon, label, count, running-dot)
                // Labels follow the AB-style sidebar; keys stay internal.
                let cats: [(&str, &str, &str); 6] = [
                    ("Image", MI_IMAGE, "Pictures"),
                    ("Video", MI_MOVIE, "Videos"),
                    ("Music", MI_MUSIC, "Music"),
                    ("Document", MI_DOC, "Documents"),
                    ("Compressed", MI_ARCHIVE, "Compressed"),
                    ("Apps", MI_GRID, "Apps"),
                ];
                ui.add_space(8.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    // Manual rows: paint (+record hit); process_hits
                    // applies selection. `cur` snapshot avoids borrow issues.
                    let nav = |ui: &mut egui::Ui,
                               hits: &mut Vec<(HitKind, egui::Rect)>,
                               man: &ManState,
                               cur: &str,
                               key: &str,
                               icon: &str,
                               label: &str,
                               n: Option<usize>,
                               dot: bool| {
                        let r = side_nav_row(ui, man, cur == key, icon, label, n, dot);
                        hits.push((HitKind::SideRow(key.to_string()), r));
                    };
                    nav(
                        ui,
                        &mut self.hits,
                        &self.man,
                        &cur,
                        "All",
                        MI_FOLDER,
                        "All",
                        Some(n_all),
                        false,
                    );
                    ui.add_space(6.0);
                    side_section(ui, "CATEGORIES");
                    for (key, icon, label) in cats {
                        let n = cat_n.get(key).copied().unwrap_or(0);
                        nav(
                            ui,
                            &mut self.hits,
                            &self.man,
                            &cur,
                            key,
                            icon,
                            label,
                            Some(n),
                            false,
                        );
                    }
                    ui.add_space(8.0);
                    side_section(ui, "STATUS");
                    let statuses: [(&str, &str, &str, usize, bool); 5] = [
                        ("Downloading", MI_DOWNLOAD, "Downloading", n_active, n_active > 0),
                        ("Queued", MI_QUEUE, "Queued", n_queued, false),
                        ("Paused", MI_STOP, "Paused", n_paused, false),
                        ("Completed", MI_CHECK, "Finished", n_done, false),
                        ("Failed", MI_REFRESH, "Failed", n_failed, false),
                    ];
                    for (key, icon, label, n, dot) in statuses {
                        nav(
                            ui,
                            &mut self.hits,
                            &self.man,
                            &cur,
                            key,
                            icon,
                            label,
                            Some(n),
                            dot,
                        );
                    }
                    // Legacy "Unfinished" view kept reachable (same as Incomplete).
                    let r = side_nav_row(
                        ui,
                        &self.man,
                        cur == "Unfinished" || cur == "Incomplete",
                        MI_FOLDER,
                        "Unfinished",
                        Some(n_incomplete),
                        false,
                    );
                    self.hits.push((HitKind::SideRow("Unfinished".to_string()), r));
                    ui.add_space(8.0);
                    side_section(ui, "MANAGE");
                    nav(
                        ui,
                        &mut self.hits,
                        &self.man,
                        &cur,
                        "Queues",
                        MI_QUEUE,
                        "Queues",
                        None,
                        false,
                    );
                    ui.add_space(4.0);
                    ui.small(format!("{n_active} active · {n_all} total"));
                });
            });

        // ================= bottom status bar =================
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let total_speed: f64 = self.downloads.iter().map(|d| d.speed_bps).sum();
                let active = self.tasks.len();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    mi(ui, MI_CLOUD_DL, 15.0, DIM);
                    ui.label(egui::RichText::new(if total_speed > 0.0 {
                        format_speed(total_speed)
                    } else {
                        String::new()
                    })
                    .color(DIM));
                    ui.add_space(10.0);
                    mi(ui, MI_MENU, 15.0, DIM);
                    ui.label(egui::RichText::new(format!("{active}")).color(DIM));
                });
            });
        });

        // ================= main area (toolbar + list, beside the sidebar) =================
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CENTRAL_BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 12,
                bottom: 12,
            }))
            .show(ctx, |ui| {
                // toolbar: uniform-height buttons, one shared gap.
                // Manual routing: paint (+record hit); process_hits acts.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TOOL_GAP;
                    let r = add_url_button(ui, &self.man);
                    self.hits.push((HitKind::Tool(ToolId::AddUrl), r));
                    ui.add_space(10.0);
                    let busy = !self.tasks.is_empty();
                    let tools = [
                        (ToolId::StartQueue, MI_PLAY, "Start Queue", true, "Start queued downloads"),
                        (ToolId::StopQueue, MI_STOP, "Stop Queue", busy, "Stop starting new downloads (active ones finish)"),
                        (ToolId::StopAll, MI_PAUSE, "Pause All", busy, "Pause every active download now"),
                    ];
                    for (id, icon, label, enabled, tip) in tools {
                        let r = tool_button(ui, &self.man, icon, label, enabled, tip);
                        self.hits.push((HitKind::Tool(id), r));
                    }
                    ui.add_space(10.0);
                    let r = tool_button(ui, &self.man, MI_QUEUE, "Open Queues", true, "Queue status, scheduler and log");
                    self.hits.push((HitKind::Tool(ToolId::Queues), r));
                    let r = tool_button(ui, &self.man, MI_CLOUD_DL, "Site Grabber", true, "Collect file links from a web page");
                    self.hits.push((HitKind::Tool(ToolId::Grabber), r));
                    let r = tool_button(ui, &self.man, MI_SETTINGS, "Settings", true, "Open Settings in its own window");
                    self.hits.push((HitKind::Tool(ToolId::Settings), r));
                });
                ui.add_space(8.0);
                // clipboard toast: dismissible banner card, never loose text
                if let Some(url) = self.clipboard_toast.clone() {
                    egui::Frame::new()
                        .fill(SURFACE)
                        .stroke(egui::Stroke::new(1.0_f32, PRIMARY))
                        .corner_radius(RADIUS)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 8,
                            top: 8,
                            bottom: 8,
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                mi(ui, MI_ADD_LINK, 16.0, PRIMARY);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.label(
                                        egui::RichText::new("Link copied from clipboard").strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            url.chars().take(90).collect::<String>(),
                                        )
                                        .small()
                                        .color(DIM),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let r = mbutton(ui, &self.man, "×", 34.0, 30.0, false);
                                        self.hits.push((HitKind::Btn(ButtonId::TDismiss), r));
                                        let r = mbutton(ui, &self.man, "Add URL", 90.0, 30.0, false);
                                        self.hits.push((HitKind::Btn(ButtonId::TAdd), r));
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                }
                let ids = self.visible_ids();
                if ids.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(
                                egui::RichText::new(MI_CLOUD_DL).size(48.0).color(SURFACE_LT),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("No downloads yet").size(14.0).color(DIM),
                            );
                            ui.label(
                                egui::RichText::new("Click Add URL to start downloading.")
                                    .size(12.0)
                                    .color(DIM),
                            );
                        });
                    });
                } else {
                    // ---- fully manual table: explicit geometry for header
                    // and rows + manual pointer routing (no egui hit-test) ----
                    const ROW_H: f32 = 46.0;
                    const HDR_H: f32 = 26.0;
                    const GAP: f32 = 8.0;
                    let cols = self.col_order.clone();
                    let avail_w = ui.available_width();
                    let others_w: f32 = cols
                        .iter()
                        .filter(|c| **c != SortCol::Name)
                        .map(|c| col_w_of(&self.col_w, *c))
                        .sum();
                    let gaps_w = GAP * (cols.len().saturating_sub(1) as f32);
                    let name_w = (avail_w - CHECK_W - others_w - gaps_w).max(160.0);
                    let col_ws: Vec<f32> = cols
                        .iter()
                        .map(|c| {
                            if *c == SortCol::Name {
                                name_w
                            } else {
                                col_w_of(&self.col_w, *c)
                            }
                        })
                        .collect();
                    let total_w: f32 =
                        CHECK_W + col_ws.iter().sum::<f32>() + gaps_w;
                    // Select-all state, snapshotted before the header.
                    let all_sel = !ids.is_empty()
                        && ids.iter().all(|id| {
                            self.downloads
                                .iter()
                                .find(|d| d.id == *id)
                                .map(|d| d.selected)
                                .unwrap_or(false)
                        });
                    // ---- header bar (fixed, scrolls horizontally with body) ----
                    egui::Frame::new()
                        .fill(HEADER_BG)
                        .corner_radius(RADIUS)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.horizontal(|ui| {
                                let (cr, _) = ui.allocate_exact_size(
                                    egui::vec2(CHECK_W, HDR_H),
                                    egui::Sense::hover(),
                                );
                                paint_check(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        cr.center(),
                                        egui::vec2(14.0, 14.0),
                                    ),
                                    all_sel,
                                    self.man.hover(&cr),
                                );
                                self.hits.push((HitKind::HdrCheck, cr));
                                for (ci, col) in cols.iter().enumerate() {
                                    let w = col_ws[ci];
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(w, HDR_H),
                                        egui::Sense::hover(),
                                    );
                                    let arrow = if self.sort_col == *col {
                                        if self.sort_asc { " ▲" } else { " ▼" }
                                    } else {
                                        ""
                                    };
                                    let txt = format!("↕ {}{}", col.title(), arrow);
                                    let g = ui.painter().layout_no_wrap(
                                        txt,
                                        egui::FontId::proportional(12.0),
                                        DIM,
                                    );
                                    ui.painter().galley(
                                        egui::pos2(
                                            r.center().x - g.size().x / 2.0,
                                            r.center().y - g.size().y / 2.0,
                                        ),
                                        g,
                                        DIM,
                                    );
                                    self.hits.push((HitKind::HdrCol(*col), r));
                                    if ci + 1 < cols.len() {
                                        let (zr, _) = ui.allocate_exact_size(
                                            egui::vec2(GAP, HDR_H),
                                            egui::Sense::hover(),
                                        );
                                        self.hits.push((HitKind::ResizeCol(*col), zr));
                                    }
                                }
                            });
                        });
                    // ---- body rows: manual geometry inside scroll content.
                    // Row screen rects come straight from layout, so manual
                    // hit-testing stays exact under scrolling ----
                    let body_h = (ui.available_height() - 40.0).max(80.0);
                    egui::ScrollArea::horizontal()
                        .id_salt("dl_h")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("dl_v")
                                .max_height(body_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for id in &ids {
                                        let id = *id;
                                        let (sel, fname, cat, size_s, prog, spd, eta, date, st) = {
                                            let d = self
                                                .downloads
                                                .iter()
                                                .find(|d| d.id == id)
                                                .unwrap();
                                            (
                                                d.selected,
                                                d.filename.clone(),
                                                d.category().to_string(),
                                                d.total_bytes
                                                    .map(format_bytes)
                                                    .unwrap_or_else(|| "--".into()),
                                                d.progress(),
                                                d.speed_bps,
                                                format_eta(d),
                                                relative_time(&d.created_at),
                                                d.status.clone(),
                                            )
                                        };
                                        // Row spacer = hit + paint rect.
                                        let (rrect, _) = ui.allocate_exact_size(
                                            egui::vec2(total_w, ROW_H),
                                            egui::Sense::hover(),
                                        );
                                        if sel {
                                            ui.painter().rect_filled(
                                                rrect.expand(2.0),
                                                RADIUS,
                                                SEL_BG,
                                            );
                                            ui.painter().rect_stroke(
                                                rrect.expand(2.0),
                                                RADIUS,
                                                egui::Stroke::new(1.0_f32, SEL_BD),
                                                egui::StrokeKind::Inside,
                                            );
                                        } else if self.man.hover(&rrect) {
                                            ui.painter().rect_filled(
                                                rrect.expand(2.0),
                                                RADIUS,
                                                HOVER_BG,
                                            );
                                        }
                                        self.hits.push((HitKind::Row(id), rrect));
                                        // Manual select checkbox.
                                        let cb = egui::Rect::from_center_size(
                                            egui::pos2(
                                                rrect.min.x + CHECK_W / 2.0,
                                                rrect.center().y,
                                            ),
                                            egui::vec2(14.0, 14.0),
                                        );
                                        paint_check(
                                            ui.painter(),
                                            cb,
                                            sel,
                                            self.man.hover(&cb.expand(4.0)),
                                        );
                                        self.hits.push((
                                            HitKind::RowCheck(id),
                                            cb.expand(6.0),
                                        ));
                                // Cells painted at explicit sub-rects: pure
                                // paint, no widgets, no cursor use — the row
                                // spacer above already advanced the cursor.
                                let p = ui.painter();
                                let mut x = rrect.min.x + CHECK_W;
                                for (ci, col) in cols.iter().enumerate() {
                                    let w = col_ws[ci];
                                    let crect = egui::Rect::from_min_size(
                                        egui::pos2(x, rrect.min.y),
                                        egui::vec2(w, ROW_H),
                                    );
                                    match col {
                                        SortCol::Name => {
                                            name_cell(
                                                &p,
                                                crect,
                                                file_icon(&cat),
                                                &fname,
                                                &cat,
                                            );
                                        }
                                        SortCol::Size => {
                                            centered_label_at(&p, crect, &size_s, 12.0, TXT);
                                        }
                                        SortCol::Status => match &st {
                                            Status::Downloading => {
                                                status_pill_at(
                                                    &p,
                                                    egui::Rect::from_min_size(
                                                        crect.min,
                                                        egui::vec2(w, 24.0),
                                                    ),
                                                    &format!(
                                                        "{}%",
                                                        (prog * 100.0) as u32
                                                    ),
                                                    PRIMARY,
                                                );
                                                paint_progress(
                                                    &p,
                                                    crect.center().x,
                                                    crect.min.y + 27.0,
                                                    w,
                                                    prog,
                                                );
                                            }
                                            Status::Queued => {
                                                centered_label_at(&p, crect, "Added", 12.0, TXT);
                                            }
                                            Status::Paused => {
                                                status_pill_at(
                                                    &p,
                                                    crect,
                                                    &format!(
                                                        "{}% Paused",
                                                        (prog * 100.0) as u32
                                                    ),
                                                    WARNING,
                                                );
                                            }
                                            Status::Completed => {
                                                status_pill_at(&p, crect, "Finished", SUCCESS);
                                            }
                                            Status::Failed(_) => {
                                                status_pill_at(&p, crect, "Failed", ERROR);
                                            }
                                        },
                                        SortCol::Speed => {
                                            let spd_s = if spd > 0.0 {
                                                format_speed(spd)
                                            } else {
                                                "-".to_string()
                                            };
                                            centered_label_at(&p, crect, &spd_s, 12.0, TXT);
                                        }
                                        SortCol::Eta => {
                                            let eta_s: &str =
                                                if eta == "-" { "" } else { &eta };
                                            centered_label_at(&p, crect, eta_s, 12.0, DIM);
                                        }
                                        SortCol::Date => {
                                            centered_label_at(&p, crect, &date, 12.0, DIM);
                                        }
                                    }
                                    x += w;
                                    if ci + 1 < cols.len() {
                                        x += GAP;
                                    }
                                }
                            }
                        });
                    });
                }
            });

        // ================= browser install result =================
        // (row/menu actions dispatch from process_hits at the end of update)
        if let Some(which) = self.pending_browser_install.take() {
            match install_native_host(&which) {
                Ok(manifest) => {
                    self.push_log(format!("Native host installed for {which}."));
                    self.browser_msg = format!("Installed {which} host ✓ ({manifest})");
                }
                Err(e) => self.browser_msg = format!("Install failed: {e}"),
            }
        }

        // ================= modal backdrop (dim only — never intercepts clicks) =================
        if self.show_add
            || self.show_queues
            || self.show_grabber
            || self.show_about
            || self.details_id.is_some()
        {
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::new("modal_dim"),
            ))
            .rect_filled(
                ctx.screen_rect(),
                0.0,
                egui::Color32::from_black_alpha(160),
            );
        }

        // ================= dialogs =================
        if self.show_add {
            if self.add_focus_req {
                // Focus lands without any click (hit-test independent).
                self.add_focus_req = false;
                focus_field(ctx, "add_url");
            }
            // Esc backs out of the menu first, then the dialog (a modal this
            // tall can otherwise be hard to dismiss).
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                if self.add_recent_open {
                    self.add_recent_open = false;
                } else {
                    self.show_add = false;
                    self.add_focus_req = false;
                }
            }
            // auto-probe once per URL
            if !self.add_url.trim().is_empty()
                && self.add_url.trim() != self.probe_done_for
                && !self.probing
            {
                self.probe_done_for = self.add_url.trim().to_string();
                self.start_probe(self.add_url.trim().to_string());
            }
            // collect probe result
            let mut got_probe = None;
            if let Some(rx) = self.probe_rx.as_ref() {
                while let Ok(r) = rx.try_recv() {
                    got_probe = Some(r);
                }
            }
            if let Some((sz, nm)) = got_probe {
                self.probing = false;
                self.probe_size = sz;
                if let Some(n) = nm {
                    self.probe_name = Some(n.clone());
                    if self.add_file.trim().is_empty() {
                        self.add_file = crate::model::apply_rename_template(
                            &self.rename_template,
                            &n,
                            &self.add_url,
                        );
                    }
                }
            }
            let psize = self.probe_size;
            let pbusy = self.probing;
            let mut open = self.show_add;
            // In-canvas centered modal (a separate OS window proved unstable
            // here, so the dialog lives in the main canvas like before).
            // The concept is a fixed-size dialog; only the advanced panel makes
            // it taller (the concept draws that collapsed).
            let adv_h = if self.add_adv { DM_PANEL_H + 14.0 } else { 0.0 };
            let card_h = (DM_H + adv_h).min((ctx.screen_rect().height() * 0.94).max(300.0));
            egui::Window::new("Add download")
                .id(egui::Id::new("add_download_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .title_bar(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .fixed_size(egui::vec2(DM_W, card_h))
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    // Everything is painted from the card origin: the concept is
                    // a pixel layout, not a flow layout.
                    let card = ui.max_rect();
                    let o = card.min;
                    let p = ui.painter().clone();
                    let at = |x: f32, y: f32, w: f32, h: f32| {
                        egui::Rect::from_min_size(egui::pos2(o.x + x, o.y + y), egui::vec2(w, h))
                    };
                    // ---- card ----
                    p.rect(
                        card,
                        8.0,
                        DM_CARD,
                        egui::Stroke::new(1.0, DM_LINE),
                        egui::StrokeKind::Inside,
                    );
                    // ---- header: mark + title + window controls ----
                    dm_mark(&p, at(15.0, 11.0, 18.0, 18.0));
                    p.text(
                        egui::pos2(o.x + 41.0, o.y + 20.0),
                        egui::Align2::LEFT_CENTER,
                        "Add download",
                        crate::fonts::semi(13.0),
                        DM_TXT,
                    );
                    p.line_segment(
                        [
                            egui::pos2(o.x + 1.0, o.y + 39.5),
                            egui::pos2(o.x + DM_W - 1.0, o.y + 39.5),
                        ],
                        egui::Stroke::new(1.0, DM_LINE),
                    );
                    // The dialog lives in the main canvas, so minimise/restore
                    // drive the real OS window and the X closes the dialog.
                    for (cx, icon, ctl) in [
                        (485.0f32, DmIcon::Minimize, 0u8),
                        (513.0, DmIcon::Maximize, 1),
                        (541.0, DmIcon::Close, 2),
                    ] {
                        let r = at(cx - 8.0, 12.0, 16.0, 16.0);
                        dm_icon_btn(&p, &self.man, r, icon, 12.0, 2.0, DM_DIM);
                        if self.man.clicked(&r) {
                            match ctl {
                                0 => ui
                                    .ctx()
                                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                                1 => {
                                    let max = ui
                                        .ctx()
                                        .input(|i| i.viewport().maximized.unwrap_or(false));
                                    ui.ctx()
                                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                                }
                                _ => {
                                    self.show_add = false;
                                    self.add_focus_req = false;
                                    self.add_recent_open = false;
                                }
                            }
                        }
                    }
                    // ---- row 1: URL + clipboard ----
                    let url_box = at(DM_X, DM_ROW1, DM_W_IN, DM_BOX_H);
                    let url_btn = egui::Rect::from_min_size(
                        egui::pos2(url_box.max.x - 33.0, url_box.min.y + 5.0),
                        egui::Vec2::splat(28.0),
                    );
                    dm_field(
                        ui,
                        "add_url",
                        url_box,
                        url_btn.min.x - 6.0,
                        &mut self.add_url,
                        "https://…",
                        false,
                    );
                    self.hits.push((HitKind::Field("add_url"), url_box));
                    dm_icon_btn(&p, &self.man, url_btn, DmIcon::Clipboard, 14.0, 1.5, DM_DIM);
                    self.hits.push((HitKind::Btn(ButtonId::AddPaste), url_btn));
                    // ---- row 2: save folder + carets | size chip ----
                    let path_box = at(DM_X, DM_ROW2, DM_PATH_W, DM_BOX_H);
                    let caret_btn = egui::Rect::from_min_size(
                        egui::pos2(path_box.max.x - 33.0, path_box.min.y + 5.0),
                        egui::Vec2::splat(28.0),
                    );
                    let folder_btn = egui::Rect::from_min_size(
                        egui::pos2(caret_btn.min.x - 28.0, caret_btn.min.y),
                        egui::Vec2::splat(28.0),
                    );
                    dm_field(
                        ui,
                        "add_dir",
                        path_box,
                        folder_btn.min.x - 6.0,
                        &mut self.add_dir,
                        "Save folder",
                        false,
                    );
                    self.hits.push((HitKind::Field("add_dir"), path_box));
                    dm_icon_btn(&p, &self.man, folder_btn, DmIcon::Folder, 14.0, 1.5, DM_DIM);
                    self.hits.push((HitKind::Btn(ButtonId::AddFolder), folder_btn));
                    dm_icon_btn(&p, &self.man, caret_btn, DmIcon::Chevron, 10.0, 2.0, DM_DIM);
                    self.add_caret = Some(caret_btn);
                    if self.man.clicked(&caret_btn) {
                        self.add_recent_open = !self.add_recent_open;
                    }
                    // size chip: grid mark, size, and a tick once the probe is in
                    let chip = at(DM_X + DM_PATH_W + 8.0, DM_ROW2, DM_SIZE_W, DM_BOX_H);
                    dm_box(&p, chip);
                    dm_icon(
                        &p,
                        egui::Rect::from_center_size(
                            egui::pos2(chip.min.x + 21.0, chip.center().y),
                            egui::Vec2::splat(16.0),
                        ),
                        DmIcon::Grid,
                        DM_ACC,
                        0.0,
                    );
                    let known = !pbusy && psize.filter(|s| *s > 0).is_some();
                    let sz_txt: String = if pbusy {
                        "…".to_string()
                    } else {
                        psize
                            .filter(|s| *s > 0)
                            .map(format_bytes)
                            .unwrap_or_else(|| "—".to_string())
                    };
                    let g = p.layout_no_wrap(sz_txt, crate::fonts::reg(11.0), DM_TXT);
                    let tw = g.size().x;
                    let tx = chip.min.x + 37.0;
                    p.galley(
                        egui::pos2(tx, chip.center().y - g.size().y * 0.5),
                        g,
                        DM_TXT,
                    );
                    if known {
                        dm_icon(
                            &p,
                            egui::Rect::from_min_size(
                                egui::pos2(tx + tw + 8.0, chip.min.y + 13.0),
                                egui::Vec2::splat(12.0),
                            ),
                            DmIcon::Check,
                            DM_OK,
                            2.5,
                        );
                    }
                    // ---- row 3: filename + refresh + advanced ----
                    let file_box = at(DM_X, DM_ROW3, DM_FILE_W, DM_BOX_H);
                    let refr_btn = egui::Rect::from_min_size(
                        egui::pos2(file_box.max.x + 8.0, file_box.min.y),
                        egui::Vec2::splat(38.0),
                    );
                    let gear_btn = egui::Rect::from_min_size(
                        egui::pos2(refr_btn.max.x + 8.0, refr_btn.min.y),
                        egui::Vec2::splat(38.0),
                    );
                    dm_field(
                        ui,
                        "add_file",
                        file_box,
                        file_box.max.x - 13.0,
                        &mut self.add_file,
                        "Filename",
                        false,
                    );
                    self.hits.push((HitKind::Field("add_file"), file_box));
                    dm_icon_btn(&p, &self.man, refr_btn, DmIcon::Refresh, 14.0, 1.5, DM_DIM);
                    self.hits.push((HitKind::Btn(ButtonId::AddRefresh), refr_btn));
                    dm_icon_btn(&p, &self.man, gear_btn, DmIcon::Settings, 14.0, 1.5, DM_DIM);
                    self.hits.push((HitKind::Btn(ButtonId::AddAdv), gear_btn));
                    // ---- advanced panel (gear): same tokens, placeholders
                    // instead of labels, per the concept's field style ----
                    if self.add_adv {
                        let panel = at(DM_X, DM_ACT, DM_W_IN, DM_PANEL_H);
                        dm_box(&p, panel);
                        let cx = panel.min.x + 13.0;
                        let cw = panel.width() - 26.0;
                        let mut cy = panel.min.y + 13.0;
                        let row = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, 30.0));
                        p.text(
                            egui::pos2(row.min.x, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Connections",
                            crate::fonts::reg(11.0),
                            DM_DIM,
                        );
                        let srect = egui::Rect::from_min_size(
                            egui::pos2(row.min.x + 92.0, row.min.y + 5.0),
                            egui::vec2(row.width() - 144.0, 20.0),
                        );
                        let frac =
                            (((self.add_conns as f64) - 1.0) / 15.0).clamp(0.0, 1.0) as f32;
                        dm_slider(&p, &self.man, srect, frac);
                        self.hits.push((
                            HitKind::Slider(SliderId::AddConns, 1.0, 16.0, false),
                            srect,
                        ));
                        p.text(
                            egui::pos2(row.max.x, row.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("{}", self.add_conns),
                            crate::fonts::reg(11.0),
                            DM_TXT,
                        );
                        cy += 38.0;
                        let half = (cw - 8.0) / 2.0;
                        let r_ref = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(half, DM_BOX_H));
                        let r_cok = egui::Rect::from_min_size(
                            egui::pos2(cx + half + 8.0, cy),
                            egui::vec2(half, DM_BOX_H),
                        );
                        dm_field(ui, "add_ref", r_ref, r_ref.max.x - 13.0, &mut self.add_referer, "Referer (optional)", false);
                        self.hits.push((HitKind::Field("add_ref"), r_ref));
                        dm_field(ui, "add_cookie", r_cok, r_cok.max.x - 13.0, &mut self.add_cookie, "Cookie (optional)", false);
                        self.hits.push((HitKind::Field("add_cookie"), r_cok));
                        cy += DM_BOX_H + 8.0;
                        let r_usr = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(half, DM_BOX_H));
                        let r_pwd = egui::Rect::from_min_size(
                            egui::pos2(cx + half + 8.0, cy),
                            egui::vec2(half, DM_BOX_H),
                        );
                        dm_field(ui, "add_user", r_usr, r_usr.max.x - 13.0, &mut self.add_user, "User (optional)", false);
                        self.hits.push((HitKind::Field("add_user"), r_usr));
                        dm_field(ui, "add_pass", r_pwd, r_pwd.max.x - 13.0, &mut self.add_pass, "Password", true);
                        self.hits.push((HitKind::Field("add_pass"), r_pwd));
                        cy += DM_BOX_H + 8.0;
                        let r_sum = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, DM_BOX_H));
                        dm_field(ui, "add_sum", r_sum, r_sum.max.x - 13.0, &mut self.add_checksum, "SHA-256 checksum (optional, 64 hex)", false);
                        self.hits.push((HitKind::Field("add_sum"), r_sum));
                        cy += DM_BOX_H + 8.0;
                        let ow = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, 22.0));
                        let cbx = egui::Rect::from_min_size(
                            egui::pos2(ow.min.x, ow.center().y - 8.0),
                            egui::Vec2::splat(16.0),
                        );
                        if self.add_overwrite {
                            p.rect_filled(cbx, 4.0, DM_ACC);
                            dm_icon(
                                &p,
                                egui::Rect::from_center_size(cbx.center(), egui::Vec2::splat(12.0)),
                                DmIcon::Check,
                                egui::Color32::WHITE,
                                2.5,
                            );
                        } else {
                            p.rect(
                                cbx,
                                4.0,
                                DM_CARD,
                                egui::Stroke::new(1.0, DM_BOX_BD),
                                egui::StrokeKind::Inside,
                            );
                        }
                        p.text(
                            egui::pos2(cbx.max.x + 8.0, ow.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Overwrite file if it already exists",
                            crate::fonts::reg(11.0),
                            DM_DIM,
                        );
                        self.hits.push((HitKind::Check(CheckId::AddOverwrite), ow));
                    }
                    // ---- actions (manual hit table, as before) ----
                    let act_y = DM_ACT + adv_h;
                    let add_btn = at(DM_X, act_y, 100.0, 36.0);
                    dm_btn_flat(&p, &self.man, add_btn, "Add");
                    self.hits.push((HitKind::Btn(ButtonId::AddAdd), add_btn));
                    let dl_btn = egui::Rect::from_min_size(
                        egui::pos2(add_btn.max.x + 8.0, add_btn.min.y),
                        egui::vec2(140.0, 36.0),
                    );
                    dm_gradient(&p, dl_btn, DM_RAD, DM_ACC, DM_ACC2);
                    if self.man.hover(&dl_btn) {
                        p.rect_filled(dl_btn, DM_RAD, egui::Color32::from_white_alpha(22));
                    }
                    p.text(
                        dl_btn.center(),
                        egui::Align2::CENTER_CENTER,
                        "Download",
                        crate::fonts::semi(13.0),
                        egui::Color32::WHITE,
                    );
                    self.hits.push((HitKind::Btn(ButtonId::AddDownload), dl_btn));
                    let cancel_btn = egui::Rect::from_min_size(
                        egui::pos2(o.x + DM_W - 17.0 - 100.0, add_btn.min.y),
                        egui::vec2(100.0, 36.0),
                    );
                    dm_btn_flat(&p, &self.man, cancel_btn, "Cancel");
                    self.hits.push((HitKind::Btn(ButtonId::AddCancel), cancel_btn));
                });
            // The egui X button can't be relied on: keep the flag unless
            // our own Cancel/Add/Download closed it.
            self.show_add = open && self.show_add;
            if self.add_dir.trim().is_empty() {
                self.add_dir = dirs::download_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "C:\\Downloads".to_string());
            }
            // ---- recent-folders menu (the path field's caret) ----
            // A click outside the caret and outside the menu closes it. The
            // caret rect is this frame's; the menu rect is derived, never
            // stored, so the two can't drift apart.
            if self.add_recent_open && ctx.input(|i| i.pointer.any_click()) {
                let pos = ctx.input(|i| i.pointer.interact_pos());
                let rows = self.add_recent_dirs().len();
                let menu = self.add_caret.map(|c| dm_menu_rect(c, rows));
                let inside = |r: Option<egui::Rect>| match (r, pos) {
                    (Some(r), Some(q)) => r.contains(q),
                    _ => false,
                };
                if !inside(self.add_caret) && !inside(menu) {
                    self.add_recent_open = false;
                }
            }
            if self.add_recent_open {
                match self.add_caret {
                    None => self.add_recent_open = false,
                    Some(caret) => {
                        let anchors = self.add_recent_dirs();
                        let mut pick: Option<String> = None;
                        egui::Area::new(egui::Id::new("add_recent_menu"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(dm_menu_rect(caret, anchors.len()).min)
                            .show(ctx, |ui| {
                                egui::Frame::new()
                                    .fill(DM_CARD)
                                    .stroke(egui::Stroke::new(1.0, DM_LINE))
                                    .corner_radius(DM_RAD)
                                    .inner_margin(egui::Margin::same(6))
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        for d in &anchors {
                                            let resp = ui
                                                .add_sized(
                                                    egui::vec2(308.0, 24.0),
                                                    egui::Button::new(
                                                        egui::RichText::new(elide_left(d, 46))
                                                            .size(11.0)
                                                            .color(DM_TXT),
                                                    )
                                                    .fill(egui::Color32::TRANSPARENT)
                                                    .stroke(egui::Stroke::NONE),
                                                )
                                                .on_hover_text(d);
                                            if resp.clicked() {
                                                pick = Some(d.clone());
                                            }
                                        }
                                    });
                            });
                        if let Some(d) = pick {
                            self.add_dir = d;
                            self.add_recent_open = false;
                        }
                    }
                }
            }
        }
        if !self.show_add {
            self.add_recent_open = false;
            self.add_caret = None;
        }

        // ================= site grabber (P3.1) =================
        if self.show_grabber {
            // Collect finished crawl without holding borrows across UI code.
            let mut got_grab: Option<Result<Vec<crate::grabber::GrabLink>, String>> = None;
            if let Some(rx) = self.grab_rx.as_ref() {
                while let Ok(r) = rx.try_recv() {
                    got_grab = Some(r);
                }
            }
            if let Some(r) = got_grab {
                self.grab_busy = false;
                match r {
                    Ok(links) => {
                        self.grab_results = links;
                        self.grab_notice.clear();
                    }
                    Err(e) => self.grab_notice = e,
                }
            }
            if self.grab_busy {
                ctx.request_repaint();
            }
            let mut open = self.show_grabber;
            let grab_w = modal_width(ctx, 560.0);
            egui::Window::new("Site Grabber")
                .id(egui::Id::new("grab_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .default_width(grab_w)
                .min_width(grab_w.min(320.0))
                .max_width(grab_w)
                .max_height((ctx.screen_rect().height() * 0.86).clamp(300.0, 900.0))
                .frame(modal_frame())
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    ui.horizontal(|ui| {
                        let r = mtext(ui, "grab_url", &mut self.grab_url, "https://gallery…", f32::INFINITY, false);
                        self.hits.push((HitKind::Field("grab_url"), r));
                    });
                    ui.horizontal(|ui| {
                        const PRESETS: &[&str] =
                            &["Mixed", "Video", "Audio", "Images", "Documents", "Archives"];
                        let cur = PRESETS[self.grab_preset.min(PRESETS.len() - 1)];
                        let r = mbutton(ui, &self.man, &format!("Preset: {cur}"), 170.0, 32.0, false);
                        self.hits.push((HitKind::Btn(ButtonId::GrabPreset), r));
                        let r = mtext(ui, "grab_exts", &mut self.grab_exts, "exts: mp4,zip,…", 220.0, false);
                        self.hits.push((HitKind::Field("grab_exts"), r));
                    });
                    ui.horizontal(|ui| {
                        let r = mcheck(ui, &self.man, "Include linked pages (same site)", self.grab_depth);
                        self.hits.push((HitKind::Check(CheckId::GrabDepth), r));
                    });
                    if !self.grab_notice.is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(255, 150, 120), &self.grab_notice);
                    }
                    if self.grab_busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Crawling…");
                        });
                    } else if !self.grab_results.is_empty() {
                        let n = self.grab_results.len();
                        ui.label(format!("{n} file link(s) found:"));
                        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                            for l in self.grab_results.iter().take(40) {
                                ui.monospace(&l.name);
                            }
                            if n > 40 {
                                ui.weak(format!("…and {} more", n - 40));
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        let r = mbutton(ui, &self.man, "Find Links", 110.0, 36.0, false);
                        self.hits.push((HitKind::Btn(ButtonId::GrabStart), r));
                        let r = mbutton(ui, &self.man, "Queue All", 130.0, 36.0, true);
                        self.hits.push((HitKind::Btn(ButtonId::GrabQueue), r));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let r = mbutton(ui, &self.man, "Close", 110.0, 36.0, false);
                                self.hits.push((HitKind::Btn(ButtonId::GrabClose), r));
                            },
                        );
                    });
                });
            self.show_grabber = open && self.show_grabber;
        }

        if self.show_queues {
            let mut open = true;
            let n_items = self.downloads.len();
            let n_active = self.tasks.len();
            let sched_fired = self.scheduler_started;
            let log: Vec<String> = self.log.iter().rev().take(60).cloned().collect();
            let q_w = modal_width(ctx, 540.0);
            egui::Window::new("Queues")
                .id(egui::Id::new("queues_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .default_width(q_w)
                .min_width(q_w.min(320.0))
                .max_width(q_w)
                .max_height((ctx.screen_rect().height() * 0.86).clamp(300.0, 900.0))
                .frame(modal_frame())
                .show(ctx, |ui| {
                ui.label(format!("{n_items} item(s) in queue, {n_active} active."));
                ui.horizontal(|ui| {
                    let r = mbutton(ui, &self.man, "Start Queue", 130.0, 32.0, false);
                    self.hits.push((HitKind::Btn(ButtonId::QStart), r));
                    let r = mbutton(ui, &self.man, "Stop Queue", 130.0, 32.0, false);
                    self.hits.push((HitKind::Btn(ButtonId::QStop), r));
                });
                ui.label(
                    egui::RichText::new(format!(
                        "Max simultaneous downloads: {}",
                        self.max_concurrent
                    ))
                    .color(DIM),
                );
                let r = mslider(
                    ui,
                    &self.man,
                    "",
                    1.0,
                    16.0,
                    self.slider_get(SliderId::QMax),
                    false,
                );
                self.hits.push((
                    HitKind::Slider(SliderId::QMax, 1.0, 16.0, false),
                    r,
                ));
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("When queue finishes:");
                    let cur = match self.finish_action.as_str() {
                        "shutdown" => "Shut down",
                        "sleep" => "Sleep",
                        "hibernate" => "Hibernate",
                        _ => "Do nothing",
                    };
                    let g = ui.painter().layout_no_wrap(
                        cur.to_string(),
                        egui::FontId::proportional(12.5),
                        TXT,
                    );
                    let (r, _) = ui.allocate_exact_size(
                        egui::vec2((g.size().x + 40.0).max(140.0), 28.0),
                        egui::Sense::hover(),
                    );
                    if self.man.hover(&r) {
                        ui.painter().rect_filled(r, 6.0, SURFACE_LT);
                    }
                    ui.painter().rect_stroke(
                        r,
                        6.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 47, 65)),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().galley(
                        egui::pos2(r.min.x + 10.0, r.center().y - g.size().y / 2.0),
                        g,
                        TXT,
                    );
                    // dropdown arrow
                    ui.painter().text(
                        egui::pos2(r.max.x - 14.0, r.center().y),
                        egui::Align2::CENTER_CENTER,
                        "▾".to_string(),
                        egui::FontId::proportional(12.0),
                        DIM,
                    );
                    self.hits.push((HitKind::ComboAnchor, r));
                });
                ui.separator();
                let r = mcheck(
                    ui,
                    &self.man,
                    "Start queue automatically at (HH:MM):",
                    self.scheduler_enabled,
                );
                self.hits.push((HitKind::Check(CheckId::SchedOn), r));
                let r = mtext(ui, "sched_time", &mut self.scheduler_time, "HH:MM", 90.0, false);
                self.hits.push((HitKind::Field("sched_time"), r));
                if !sched_fired {
                    ui.small("Waiting for scheduled time…");
                } else {
                    ui.small("Scheduler already fired this session.");
                    let r = mbutton(ui, &self.man, "Reset Scheduler", 140.0, 30.0, false);
                    self.hits.push((HitKind::Btn(ButtonId::QSchedReset), r));
                }
                ui.separator();
                ui.collapsing("Log", |ui| {
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        for line in &log {
                            ui.monospace(line);
                        }
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let r = mbutton(ui, &self.man, "Close", 110.0, 36.0, false);
                            self.hits.push((HitKind::Btn(ButtonId::QClose), r));
                        },
                    );
                });
            });
            let was_open = self.show_queues;
            self.show_queues = open;
            if was_open && !open {
                self.save_ui_state();
            }
        }

        if let Some(did) = self.details_id {
            let snap = self.downloads.iter().find(|d| d.id == did).cloned();
            match snap {
                None => self.details_id = None,
                Some(item) => {
                    let active = self.tasks.contains_key(&did);
                    let paused = matches!(item.status, Status::Paused);
                    let (ranges, dones) = self
                        .tasks
                        .get(&did)
                        .map(|t| {
                            // never panic on a poisoned lock (engine task died holding it)
                            let r = t
                                .shared
                                .part_total
                                .lock()
                                .map(|g| g.clone())
                                .unwrap_or_default();
                            let d = t
                                .shared
                                .part_done
                                .lock()
                                .map(|g| g.clone())
                                .unwrap_or_default();
                            (r, d)
                        })
                        .unwrap_or_default();
                    let pct = (item.progress() * 100.0) as u32;
                    let mut open = true;
                    let tab = self.details_tab;
                    let title = if matches!(item.status, Status::Downloading | Status::Paused) {
                        format!("{pct}%-{}", item.filename)
                    } else {
                        item.filename.clone()
                    };
                    let det_w = modal_width(ctx, 520.0);
                    egui::Window::new(title)
                        .id(egui::Id::new("details_modal"))
                        .order(egui::Order::Foreground)
                        .open(&mut open)
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .pivot(egui::Align2::CENTER_CENTER)
                        .collapsible(false)
                        .resizable(false)
                        .default_width(det_w)
                        .min_width(det_w.min(320.0))
                        .max_width(det_w)
                        .max_height((ctx.screen_rect().height() * 0.86).clamp(300.0, 900.0))
                        .frame(modal_frame())
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                for (t, label) in [(0u8, "Info"), (1u8, "Settings")] {
                                    let g = ui.painter().layout_no_wrap(
                                        label.to_string(),
                                        egui::FontId::proportional(12.5),
                                        if tab == t { TXT } else { DIM },
                                    );
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(g.size().x + 24.0, 26.0),
                                        egui::Sense::hover(),
                                    );
                                    if tab == t {
                                        ui.painter().rect_filled(r, 4.0, SURFACE_LT);
                                    } else if self.man.hover(&r) {
                                        ui.painter().rect_filled(r, 4.0, HOVER_BG);
                                    }
                                    ui.painter().galley(
                                        egui::pos2(
                                            r.center().x - g.size().x / 2.0,
                                            r.center().y - g.size().y / 2.0,
                                        ),
                                        g,
                                        if tab == t { TXT } else { DIM },
                                    );
                                    self.hits.push((HitKind::DetTab(t), r));
                                }
                            });
                            ui.separator();
                            if tab == 0 {
                                egui::Grid::new("dinfo")
                                    .num_columns(2)
                                    .spacing([24.0, 6.0])
                                    .show(ui, |ui| {
                                        let row = |ui: &mut egui::Ui, k: &str, v: String| {
                                            ui.label(egui::RichText::new(k).color(DIM));
                                            ui.label(egui::RichText::new(v).size(13.0));
                                            ui.end_row();
                                        };
                                        row(ui, "Name:", item.filename.clone());
                                        row(
                                            ui,
                                            "Status:",
                                            match item.status {
                                                Status::Queued => "Queued".to_string(),
                                                Status::Downloading => "Downloading".to_string(),
                                                Status::Paused => "Paused".to_string(),
                                                Status::Completed => "Finished".to_string(),
                                                Status::Failed(_) => "Failed".to_string(),
                                            },
                                        );
                                        row(
                                            ui,
                                            "Size:",
                                            item.total_bytes
                                                .map(format_bytes)
                                                .unwrap_or_else(|| "?".into()),
                                        );
                                        row(ui, "Downloaded:", format_bytes(item.downloaded_bytes));
                                        row(ui, "Speed:", format_speed(item.speed_bps));
                                        row(
                                            ui,
                                            "Remaining Time:",
                                            if format_eta(&item) == "-" {
                                                "-".to_string()
                                            } else {
                                                format_eta(&item)
                                            },
                                        );
                                        ui.label(egui::RichText::new("Resume Support:").color(DIM));
                                        ui.label(
                                            egui::RichText::new(if item.resumable {
                                                "Yes"
                                            } else {
                                                "No"
                                            })
                                            .color(if item.resumable {
                                                SUCCESS
                                            } else {
                                                ERROR
                                            }),
                                        );
                                        ui.end_row();
                                        ui.label(egui::RichText::new("Integrity (SHA-256):").color(DIM));
                                        ui.label(
                                            egui::RichText::new(if item.checksum_sha256.trim().is_empty() {
                                                "not set".to_string()
                                            } else {
                                                format!("{}…", &item.checksum_sha256.chars().take(16).collect::<String>())
                                            })
                                            .size(13.0),
                                        );
                                        ui.end_row();
                                    });
                                ui.add_space(8.0);
                                status_bar_w(
                                    ui,
                                    ui.available_width(),
                                    item.progress(),
                                    PRIMARY,
                                    SECONDARY,
                                );
                                ui.add_space(8.0);
                                // P5 speed history: raw quarter-second samples
                                // as a mini area chart (EWMA readout above it).
                                let hist: Vec<f64> = self
                                    .speed_hist
                                    .get(&did)
                                    .map(|h| h.iter().copied().collect())
                                    .unwrap_or_default();
                                if hist.len() > 1 {
                                    let peak = hist.iter().copied().fold(0.0_f64, f64::max).max(1.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Speed history (peak {}):",
                                            format_speed(peak)
                                        ))
                                        .color(DIM),
                                    );
                                    let (grect, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 40.0),
                                        egui::Sense::hover(),
                                    );
                                    let gp = ui.painter();
                                    gp.rect_filled(grect, 4.0, SURFACE);
                                    let n = hist.len();
                                    let pts: Vec<egui::Pos2> = hist
                                        .iter()
                                        .enumerate()
                                        .map(|(i, v)| {
                                            let x = grect.min.x
                                                + grect.width() * (i as f32 / (n.max(2) - 1) as f32);
                                            let y = grect.max.y
                                                - 3.0
                                                - (grect.height() - 6.0)
                                                    * (*v as f32 / peak as f32).clamp(0.0, 1.0);
                                            egui::pos2(x, y)
                                        })
                                        .collect();
                                    let mut poly = pts.clone();
                                    poly.push(egui::pos2(grect.max.x, grect.max.y - 1.0));
                                    poly.push(egui::pos2(grect.min.x, grect.max.y - 1.0));
                                    gp.add(egui::Shape::convex_polygon(
                                        poly,
                                        egui::Color32::from_rgba_unmultiplied(124, 92, 252, 40),
                                        egui::Stroke::new(0.0, egui::Color32::TRANSPARENT),
                                    ));
                                    gp.add(egui::Shape::line(pts, egui::Stroke::new(1.5, PRIMARY)));
                                    ui.add_space(8.0);
                                }
                                ui.horizontal(|ui| {
                                    if active {
                                        let r = mbutton(
                                            ui,
                                            &self.man,
                                            if paused { "Resume" } else { "Pause" },
                                            110.0,
                                            32.0,
                                            false,
                                        );
                                        self.hits.push((HitKind::Btn(ButtonId::DResume), r));
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let r = mbutton(ui, &self.man, "Close", 110.0, 32.0, false);
                                            self.hits.push((HitKind::Btn(ButtonId::DClose), r));
                                        },
                                    );
                                });
                                ui.add_space(4.0);
                                // Part details (manual collapse toggle).
                                let r = mbutton(ui, &self.man, if self.det_parts_open { "Hide Part Details" } else { "Show Part Details" }, 150.0, 28.0, false);
                                self.hits.push((HitKind::DetParts, r));
                                if self.det_parts_open {
                                    if ranges.is_empty() {
                                        ui.small("No part data yet.");
                                    } else {
                                        // colored strip
                                        let total: u64 =
                                            ranges.iter().map(|(s, e)| e.saturating_sub(*s) + 1).sum();
                                        let w = ui.available_width();
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(w, 12.0),
                                            egui::Sense::hover(),
                                        );
                                        let p = ui.painter();
                                        p.rect_filled(rect, 5.0, SURFACE);
                                        if total > 0 {
                                            let mut x = rect.min.x;
                                            for (i, (s, e)) in ranges.iter().enumerate() {
                                                let part_len = e.saturating_sub(*s) + 1;
                                                let seg_w = rect.width() * part_len as f32 / total as f32;
                                                let done = dones.get(i).copied().unwrap_or(0);
                                                let frac = (done as f64 / part_len.max(1) as f64)
                                                    .clamp(0.0, 1.0);
                                                let c = if frac >= 1.0 {
                                                    SUCCESS
                                                } else if active && done > 0 {
                                                    WARNING
                                                } else {
                                                    egui::Color32::from_rgb(0x3A, 0x3B, 0x44)
                                                };
                                                let seg = egui::Rect::from_min_size(
                                                    egui::pos2(x + 1.0, rect.min.y),
                                                    egui::vec2((seg_w - 2.0).max(2.0), rect.height()),
                                                );
                                                p.rect_filled(seg, 3.0, c);
                                                x += seg_w;
                                            }
                                        }
                                        // per-part table (scrolls past ~8 rows so
                                        // the dialog never outgrows the screen)
                                        egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
                                        egui::Grid::new("parts")
                                            .num_columns(4)
                                            .spacing([16.0, 4.0])
                                            .striped(true)
                                            .show(ui, |ui| {
                                                ui.label(egui::RichText::new("#").color(DIM));
                                                ui.label(egui::RichText::new("Status").color(DIM));
                                                ui.label(
                                                    egui::RichText::new("Downloaded").color(DIM),
                                                );
                                                ui.label(egui::RichText::new("Total").color(DIM));
                                                ui.end_row();
                                                for (i, (s, e)) in ranges.iter().enumerate() {
                                                    let part_len = e.saturating_sub(*s) + 1;
                                                    let done = dones.get(i).copied().unwrap_or(0);
                                                    let st = if done >= part_len {
                                                        "Complete"
                                                    } else if active && done > 0 {
                                                        "Receiving Data"
                                                    } else if active {
                                                        "Waiting"
                                                    } else {
                                                        "Paused"
                                                    };
                                                    ui.label(format!("{}", i + 1));
                                                    ui.label(st);
                                                    ui.label(format_bytes(done));
                                                    ui.label(format_bytes(part_len));
                                                    ui.end_row();
                                                }
                                            });
                                        });
                                    }
                                }
                            } else {
                                // Live-apply settings (no Apply button): edits
                                // write straight through; network options take
                                // effect on restart.
                                if self.det_for != Some(did) {
                                    self.det_for = Some(did);
                                    self.det_sum = item.checksum_sha256.clone();
                                }
                                ui.label("Save folder:");
                                ui.monospace(&item.save_dir);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Connections: {}",
                                        item.connections
                                    ))
                                    .color(DIM),
                                );
                                let r = mslider(
                                    ui,
                                    &self.man,
                                    "",
                                    1.0,
                                    16.0,
                                    self.slider_get(SliderId::DetConns),
                                    false,
                                );
                                self.hits.push((
                                    HitKind::Slider(SliderId::DetConns, 1.0, 16.0, false),
                                    r,
                                ));
                                // P5 per-download overrides (0 = global).
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Retries: {}",
                                        item.max_retries
                                            .map(|n| n.to_string())
                                            .unwrap_or_else(|| format!(
                                                "global ({})",
                                                self.max_retries
                                            ))
                                    ))
                                    .color(DIM),
                                );
                                let r = mslider(
                                    ui,
                                    &self.man,
                                    "",
                                    0.0,
                                    20.0,
                                    self.slider_get(SliderId::DetRetries),
                                    false,
                                );
                                self.hits.push((
                                    HitKind::Slider(SliderId::DetRetries, 0.0, 20.0, false),
                                    r,
                                ));
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Timeout: {}",
                                        item.timeout_secs
                                            .map(|n| format!("{n}s"))
                                            .unwrap_or_else(|| format!(
                                                "global ({}s)",
                                                self.timeout_secs
                                            ))
                                    ))
                                    .color(DIM),
                                );
                                let r = mslider(
                                    ui,
                                    &self.man,
                                    "",
                                    0.0,
                                    600.0,
                                    self.slider_get(SliderId::DetTimeout),
                                    false,
                                );
                                self.hits.push((
                                    HitKind::Slider(SliderId::DetTimeout, 0.0, 600.0, false),
                                    r,
                                ));
                                ui.small("Retries/timeout apply on restart (0 = global).");
                                ui.label("Referer:");
                                if let Some(d) =
                                    self.downloads.iter_mut().find(|d| d.id == did)
                                {
                                    let r = mtext(ui, "det_ref", &mut d.referer, "", f32::INFINITY, false);
                                    // (hit recorded after borrow ends)
                                    let rr = r;
                                    self.hits.push((HitKind::Field("det_ref"), rr));
                                }
                                ui.label("Cookie:");
                                if let Some(d) =
                                    self.downloads.iter_mut().find(|d| d.id == did)
                                {
                                    let r = mtext(ui, "det_cookie", &mut d.cookie, "", f32::INFINITY, false);
                                    let rr = r;
                                    self.hits.push((HitKind::Field("det_cookie"), rr));
                                }
                                ui.horizontal(|ui| {
                                    ui.label("User:");
                                    if let Some(d) =
                                        self.downloads.iter_mut().find(|d| d.id == did)
                                    {
                                        let r = mtext(ui, "det_user", &mut d.username, "", 130.0, false);
                                        let rr = r;
                                        self.hits.push((HitKind::Field("det_user"), rr));
                                    }
                                    ui.label("Pass:");
                                    if let Some(d) =
                                        self.downloads.iter_mut().find(|d| d.id == did)
                                    {
                                        let r = mtext(ui, "det_pass", &mut d.password, "", 130.0, true);
                                        let rr = r;
                                        self.hits.push((HitKind::Field("det_pass"), rr));
                                    }
                                });
                                ui.small("Applies immediately (network options on restart).");
                                ui.label("SHA-256 (optional, 64 hex):");
                                let r = mtext(ui, "det_sum", &mut self.det_sum, "", f32::INFINITY, false);
                                self.hits.push((HitKind::Field("det_sum"), r));
                                // Commit a valid checksum live; invalid stays
                                // local (never clobbers the stored one).
                                let norm = crate::model::normalize_sha256(&self.det_sum);
                                if norm.is_empty() || crate::model::is_valid_sha256(&norm) {
                                    if let Some(d) =
                                        self.downloads.iter_mut().find(|d| d.id == did)
                                    {
                                        if d.checksum_sha256 != norm {
                                            d.checksum_sha256 = norm;
                                            self.save_queue();
                                        }
                                    }
                                } else if !self.det_sum.trim().is_empty() {
                                    ui.small("Invalid SHA-256 (want 64 hex chars) — not saved.");
                                }
                            }
                        });
                    self.details_tab = tab;
                    if !open {
                        self.details_id = None;
                        self.det_for = None;
                    }
                }
            }
        }

        if self.show_about {
            let ab_w = modal_width(ctx, 400.0);
            egui::Window::new("About")
                .id(egui::Id::new("about_modal"))
                .order(egui::Order::Foreground)
                .open(&mut self.show_about)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .default_width(ab_w)
                .min_width(ab_w.min(320.0))
                .max_width(ab_w)
                .max_height((ctx.screen_rect().height() * 0.86).clamp(280.0, 900.0))
                .frame(modal_frame())
                .show(ctx, |ui| {
                ui.heading("Download Craft");
                ui.small(format!("Version {} — check this to confirm your build.", env!("CARGO_PKG_VERSION")));
                ui.label("A lightweight download manager written in Rust + egui. MIT licensed — free for open-source, closed-source, and commercial use.");
                ui.small("Multi-connection segmented downloads, queue, scheduler, browser extension host.");
                ui.separator();
                ui.small("Interface design inspired by modern download managers.");
                ui.small("Icons: Material Icons font (Apache-2.0, © Google). See NOTICE.md.");
                // P5 portable updater: check is opt-in, the asset downloads
                // as a normal queued file (never a silent self-replace —
                // Windows locks the running exe).
                ui.separator();
                ui.horizontal(|ui| {
                    let r = mbutton(
                        ui,
                        &self.man,
                        if self.update_busy { "Checking…" } else { "Check for updates" },
                        150.0,
                        32.0,
                        false,
                    );
                    self.hits.push((HitKind::Btn(ButtonId::UpdCheck), r));
                    if self.update_asset.is_some() && !self.update_busy {
                        let r = mbutton(ui, &self.man, "Download update", 150.0, 32.0, true);
                        self.hits.push((HitKind::Btn(ButtonId::UpdDownload), r));
                    }
                });
                if !self.update_status.trim().is_empty() {
                    ui.small(self.update_status.clone());
                }
                if self.update_asset.is_some() {
                    ui.small("Portable .zip lands in your downloads — unzip over the app folder to upgrade.");
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let r = mbutton(ui, &self.man, "Close", 110.0, 36.0, false);
                            self.hits.push((HitKind::Btn(ButtonId::AbClose), r));
                        },
                    );
                });
            });
        }

        // ================= manual popups (topmost) + input processing ====
        // Dropdown (menubar) and row context menu render here so their hit
        // rects are recorded last (processed first = topmost wins).
        if let Some(m) = self.drop_menu {
            self.render_drop_menu(ctx, m);
        }
        if let Some((id, pos)) = self.ctx_menu {
            self.render_ctx_menu(ctx, id, pos);
        }
        self.process_hits(ctx);
        self.diag_hits_n = self.hits.len();
    }
}

impl CraftApp {
    /// Menubar dropdown items for `render_popup`. (label, MenuItem act.)
    /// The View menu marks the active theme.
    fn drop_rows(&self, menu: u8) -> Vec<(String, Option<HitKind>)> {
        let mut out = Vec::new();
        for (label, act) in Self::menu_items(menu) {
            if label.is_empty() {
                out.push((String::new(), None));
                continue;
            }
            let mut text = label.clone();
            if menu == 3 {
                let key = match act {
                    20 => "dark",
                    21 => "black",
                    _ => "light",
                };
                if self.theme == key {
                    text = format!("✓ {text}");
                }
            }
            out.push((text, Some(HitKind::MenuItem(act))));
        }
        out
    }

    /// Row context-menu rows for `render_popup`.
    fn ctx_rows(id: u64) -> Vec<(String, Option<HitKind>)> {
        Self::ctx_items()
            .into_iter()
            .map(|(label, act)| {
                (
                    label,
                    act.map(|a| HitKind::CtxItem(a, id)),
                )
            })
            .collect()
    }

    fn frac_to_val(frac: f64, min: f64, max: f64, log: bool) -> f64 {
        let f = frac.clamp(0.0, 1.0);
        if log {
            (min.ln() + f * (max.ln() - min.ln())).exp()
        } else {
            min + f * (max - min)
        }
    }

    fn slider_get(&self, id: SliderId) -> f64 {
        match id {
            SliderId::AddConns => self.add_conns as f64,
            SliderId::LimitKB => self.speed_limit_kbps.max(50) as f64,
            SliderId::Timeout => self.timeout_secs as f64,
            SliderId::Retries => self.max_retries as f64,
            SliderId::DefConns => self.connections_default as f64,
            SliderId::QMax => self.max_concurrent as f64,
            SliderId::DetConns => self
                .details_id
                .and_then(|did| self.downloads.iter().find(|d| d.id == did))
                .map(|d| d.connections as f64)
                .unwrap_or(8.0),
            // 0 = follow the global setting (P5 per-download overrides).
            SliderId::DetRetries => self
                .details_id
                .and_then(|did| self.downloads.iter().find(|d| d.id == did))
                .map(|d| d.max_retries.unwrap_or(0) as f64)
                .unwrap_or(0.0),
            SliderId::DetTimeout => self
                .details_id
                .and_then(|did| self.downloads.iter().find(|d| d.id == did))
                .map(|d| d.timeout_secs.unwrap_or(0) as f64)
                .unwrap_or(0.0),
        }
    }

    fn slider_set(&mut self, id: SliderId, v: f64) {
        match id {
            SliderId::AddConns => self.add_conns = (v as usize).clamp(1, 16),
            SliderId::LimitKB => self.speed_limit_kbps = (v as u64).clamp(50, 20000),
            SliderId::Timeout => self.timeout_secs = (v as u64).clamp(5, 600),
            SliderId::Retries => self.max_retries = (v as u32).clamp(1, 20),
            SliderId::DefConns => self.connections_default = (v as usize).clamp(1, 16),
            SliderId::QMax => {
                self.max_concurrent = (v as usize).clamp(1, 16);
                self.fill_next();
            }
            SliderId::DetConns => {
                if let Some(did) = self.details_id {
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.id == did) {
                        d.connections = (v as usize).clamp(1, 16);
                    }
                }
            }
            SliderId::DetRetries => {
                if let Some(did) = self.details_id {
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.id == did) {
                        let n = v.round() as u32;
                        d.max_retries = if n == 0 { None } else { Some(n.clamp(1, 20)) };
                        self.save_queue();
                    }
                }
            }
            SliderId::DetTimeout => {
                if let Some(did) = self.details_id {
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.id == did) {
                        let n = (v.round() as u64) / 5 * 5; // 5 s steps
                        d.timeout_secs = if n == 0 { None } else { Some(n.clamp(5, 600)) };
                        self.save_queue();
                    }
                }
            }
        }
    }

    /// Manual dialog/toast button dispatcher (driven by process_hits).
    /// Validate + queue the Add dialog contents (shared by Add/Download).
    fn commit_add(&mut self, download: bool) {
        if !is_download_url(self.add_url.trim()) {
            self.show_add = true;
            self.push_log("Add download: invalid URL (need http/https).");
            return;
        }
        let csum = crate::model::normalize_sha256(&self.add_checksum);
        if !csum.is_empty() && !crate::model::is_valid_sha256(&csum) {
            self.show_add = true;
            self.push_log("Add download: invalid SHA-256 (want 64 hex chars).");
            return;
        }
        // Same URL already tracked: a live row (queued/downloading/paused, or
        // finished with its file present) blocks a duplicate, but a DEAD row —
        // failed, or "Completed" with the file gone — is refreshed in place
        // below so a stale entry never dead-ends the user.
        let existing = self
            .downloads
            .iter()
            .position(|d| d.url == self.add_url.trim());
        if let Some(pos) = existing {
            let dead = match &self.downloads[pos].status {
                Status::Failed(_) => true,
                Status::Completed => !self.downloads[pos].dest_path().exists(),
                _ => false,
            };
            if !dead {
                self.show_add = true;
                self.push_log("Add download: that URL is already queued.");
                return;
            }
        }
        if self.add_file.trim().is_empty() {
            self.add_file = filename_from_url(&self.add_url);
        }
        if self.add_dir.trim().is_empty() {
            self.add_dir = dirs::download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "C:\\Downloads".to_string());
        }
        let final_dir = self.save_dir_for(&self.add_file);
        let id = match existing {
            Some(pos) => {
                // Refresh the dead row in place (same id): drop any stale
                // resume state so a mixed-URL resume can't corrupt the file.
                let id = self.downloads[pos].id;
                let dp = self.downloads[pos].dest_path();
                crate::model::drop_resume_state(&dp);
                id
            }
            None => self.alloc_id(),
        };
        let mut item = DownloadItem::new(
            id,
            self.add_url.trim().to_string(),
            self.add_file.trim().to_string(),
            final_dir,
            self.add_conns,
        );
        item.referer = self.add_referer.clone();
        item.cookie = self.add_cookie.clone();
        item.username = self.add_user.clone();
        item.password = self.add_pass.clone();
        item.checksum_sha256 = csum;
        item.overwrite_existing = self.add_overwrite;
        item.quality_pref = self.add_quality.trim().to_string();
        // Probed (or browser-measured) size survives queueing so the engine
        // and progress UI know the total even if a re-probe would 403.
        item.total_bytes = self.probe_size.filter(|s| *s > 0);
        if !self.add_overwrite {
            let dest = item.dest_path();
            if dest.exists() {
                self.push_log(format!(
                    "Note: {} exists — will save as a numbered copy unless overwritten.",
                    item.filename
                ));
            }
        }
        match existing {
            Some(pos) => {
                self.downloads[pos] = item;
                self.push_log(format!("Re-queued #{id}"));
            }
            None => {
                self.downloads.push(item);
                self.push_log(format!("Queued #{id}"));
            }
        }
        self.save_queue();
        self.show_add = false;
        self.add_focus_req = false;
        self.add_url.clear();
        self.add_file.clear();
        self.add_checksum.clear();
        self.add_quality.clear();
        if download {
            self.queue_running = true;
            self.start(id);
        }
    }

    /// Candidate save folders for the path field's caret menu: the standard
    /// user folders first, then whatever the dialog is currently pointed at.
    fn add_recent_dirs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in [
            dirs::download_dir(),
            dirs::desktop_dir(),
            dirs::document_dir(),
            dirs::picture_dir(),
            dirs::video_dir(),
            dirs::audio_dir(),
        ]
        .into_iter()
        .flatten()
        {
            let s = d.to_string_lossy().to_string();
            if !out.contains(&s) {
                out.push(s);
            }
        }
        for s in [self.add_dir.trim().to_string()] {
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    fn do_button(&mut self, b: ButtonId) {
        self.diag_acts += 1;
        match b {
            ButtonId::AddPaste => {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(t) = cb.get_text() {
                        let t = t.trim().to_string();
                        if is_download_url(&t) {
                            self.add_url = t;
                        }
                    }
                }
            }
            ButtonId::AddFolder => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.add_dir = p.to_string_lossy().to_string();
                }
            }
            ButtonId::AddRefresh => {
                if !self.add_url.trim().is_empty() {
                    self.probe_done_for = self.add_url.trim().to_string();
                    self.start_probe(self.add_url.trim().to_string());
                }
            }
            ButtonId::AddAdv => self.add_adv = !self.add_adv,
            ButtonId::AddAdd => self.commit_add(false),
            ButtonId::AddDownload => self.commit_add(true),
            ButtonId::AddCancel => {
                self.show_add = false;
                self.add_focus_req = false;
            }
            ButtonId::GrabStart => self.start_grab(),
            ButtonId::GrabQueue => self.queue_grab_results(),
            ButtonId::GrabClose => {
                self.show_grabber = false;
            }
            ButtonId::GrabPreset => {
                self.grab_preset = (self.grab_preset + 1) % 6;
                const PRESETS: &[&str] =
                    &["Mixed", "Video", "Audio", "Images", "Documents", "Archives"];
                self.grab_exts = crate::grabber::preset_exts(PRESETS[self.grab_preset]);
            }
            ButtonId::RuleAdd => {
                if self.folder_rules.len() < 8 {
                    self.folder_rules.push(crate::model::FolderRule {
                        exts: String::new(),
                        folder: String::new(),
                    });
                    self.save_ui_state();
                }
            }
            ButtonId::RuleReset => {
                self.folder_rules = crate::model::default_folder_rules();
                self.save_ui_state();
            }
            ButtonId::RuleDel(i) => {
                if i < self.folder_rules.len() {
                    self.folder_rules.remove(i);
                    self.save_ui_state();
                }
            }
            ButtonId::RuleUp(i) => {
                if i > 0 && i < self.folder_rules.len() {
                    self.folder_rules.swap(i - 1, i);
                    self.save_ui_state();
                }
            }
            ButtonId::RuleDown(i) => {
                if i + 1 < self.folder_rules.len() {
                    self.folder_rules.swap(i, i + 1);
                    self.save_ui_state();
                }
            }
            ButtonId::WinAdd => {
                if self.speed_windows.len() < 6 {
                    self.speed_windows.push(crate::model::SpeedWindow {
                        start: "02:00".to_string(),
                        end: "06:00".to_string(),
                        limit_kbps: 0,
                    });
                    self.save_ui_state();
                }
            }
            ButtonId::WinDel(i) => {
                if i < self.speed_windows.len() {
                    self.speed_windows.remove(i);
                    self.save_ui_state();
                }
            }
            ButtonId::UpdCheck => self.start_update_check(),
            ButtonId::UpdDownload => self.download_update_asset(),
            ButtonId::QStart => self.start_queue(),
            ButtonId::QStop => self.stop_queue(),
            ButtonId::QSchedReset => self.scheduler_started = false,
            ButtonId::QClose => {
                self.show_queues = false;
                self.save_ui_state();
            }
            ButtonId::SFolder => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.add_dir = p.to_string_lossy().to_string();
                    self.save_ui_state();
                }
            }
            ButtonId::SChrome => self.pending_browser_install = Some("chrome".to_string()),
            ButtonId::SEdge => self.pending_browser_install = Some("edge".to_string()),
            ButtonId::SFirefox => self.pending_browser_install = Some("firefox".to_string()),
            ButtonId::SBrave => self.pending_browser_install = Some("brave".to_string()),
            ButtonId::SOpera => self.pending_browser_install = Some("opera".to_string()),
            ButtonId::SClose => {
                // Legacy in-app Settings close (Settings now lives in its own
                // window; kept so the match stays exhaustive).
                if self.add_dir.trim().is_empty() {
                    self.add_dir = dirs::download_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "C:\\Downloads".to_string());
                }
                self.save_ui_state();
            }
            ButtonId::ThemeDark => {
                self.theme = "dark".to_string();
                self.save_ui_state();
            }
            ButtonId::ThemeBlack => {
                self.theme = "black".to_string();
                self.save_ui_state();
            }
            ButtonId::ThemeLight => {
                self.theme = "light".to_string();
                self.save_ui_state();
            }
            ButtonId::DResume => {
                if let Some(did) = self.details_id {
                    let paused = matches!(
                        self.downloads.iter().find(|d| d.id == did).map(|d| &d.status),
                        Some(Status::Paused)
                    );
                    if paused {
                        self.resume(did);
                    } else {
                        self.pause(did);
                    }
                }
            }
            ButtonId::DClose => self.details_id = None,
            ButtonId::AbClose => self.show_about = false,
            ButtonId::TAdd => {
                if let Some(url) = self.clipboard_toast.clone() {
                    self.add_url = url.clone();
                    self.add_file = filename_from_url(&url);
                    self.add_checksum.clear();
                    self.add_quality.clear();
                    self.clipboard_toast = None;
                    self.add_focus_req = true;
                    self.show_add = true;
                }
            }
            ButtonId::TDismiss => self.clipboard_toast = None,
        }
    }

    /// Generic manual popup list. Records item hits; clicks dispatch from
    /// process_hits. Empty labels render as separators.
    fn render_popup(
        &mut self,
        ctx: &egui::Context,
        area_id: &str,
        pos: egui::Pos2,
        width: f32,
        rows: Vec<(String, Option<HitKind>)>,
    ) {
        const H: f32 = 28.0;
        let h: f32 = rows
            .iter()
            .map(|(l, _)| if l.is_empty() { 9.0 } else { H })
            .sum::<f32>()
            + 12.0;
        let scr = ctx.screen_rect();
        let pos = egui::pos2(
            pos.x.min(scr.max.x - width - 8.0).max(8.0),
            pos.y.min(scr.max.y - h - 8.0).max(8.0),
        );
        egui::Area::new(egui::Id::new(area_id))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(20, 21, 29))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(45, 47, 65),
                    ))
                    .corner_radius(RADIUS)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (label, hit) in rows {
                            if label.is_empty() {
                                ui.add_space(3.0);
                                ui.separator();
                                ui.add_space(3.0);
                                continue;
                            }
                            let (r, _) = ui.allocate_exact_size(
                                egui::vec2(width - 12.0, H),
                                egui::Sense::hover(),
                            );
                            if self.man.hover(&r) {
                                ui.painter().rect_filled(r, 4.0, SURFACE_LT);
                            }
                            ui.painter().text(
                                egui::pos2(r.min.x + 10.0, r.center().y),
                                egui::Align2::LEFT_CENTER,
                                label,
                                egui::FontId::proportional(12.5),
                                TXT,
                            );
                            if let Some(h) = hit {
                                self.hits.push((h, r));
                            }
                        }
                    });
            });
    }

    fn render_drop_menu(&mut self, ctx: &egui::Context, menu: u8) {
        // Position below the menubar button (recorded this frame).
        let pos = self
            .hits
            .iter()
            .find(|(k, _)| matches!(k, HitKind::MenuBtn(m) if *m == menu))
            .map(|(_, r)| egui::pos2(r.min.x, r.max.y + 4.0))
            .unwrap_or(egui::pos2(100.0, 100.0));
        let rows = self.drop_rows(menu);
        self.render_popup(ctx, "drop_menu", pos, 240.0, rows);
    }

    fn render_ctx_menu(&mut self, ctx: &egui::Context, id: u64, pos: egui::Pos2) {
        let rows = Self::ctx_rows(id);
        self.render_popup(ctx, "ctx_menu", pos, 230.0, rows);
    }

    /// Match recorded hit rects (reverse = topmost first) against manual
    /// pointer state and dispatch. The single place anything "clicks".
    fn process_hits(&mut self, ctx: &egui::Context) {
        // Active divider drag continuation (movement, not clicks).
        // Active divider drag continues here (movement, not clicks).
        if let Some(cap) = self.resize_cap.take() {
            match cap {
                ResizeCap::Col { col, start_x, start_w } => {
                    if self.man.down_left() {
                        if let Some(p) = self.man.pos() {
                            let nw = (start_w + (p.x - start_x)).clamp(col_min(col), 900.0);
                            self.col_w.insert(col, nw);
                        }
                        self.resize_cap = Some(ResizeCap::Col { col, start_x, start_w });
                    } else {
                        self.save_ui_state();
                    }
                }
            }
        }
        // Modal dialogs block main-UI hits beneath them; popups stay live.
        let modal_open = self.show_add
            || self.show_queues
            || self.show_about
            || self.details_id.is_some();
        let mut hover_dbg = "-".to_string();
        let mut fired_any = false;
        let mut fired_menu = false;
        let mut fired_ctx = false;
        let mut open_ctx: Option<(u64, egui::Pos2)> = None;
        let mut open_drop: Option<u8> = None;
        let mut close_drop = false;
        // Snapshot hover for cursor icons (resize zones).
        let mut zone_cursor: Option<egui::CursorIcon> = None;
        for (kind, rect) in self.hits.clone().into_iter().rev() {
            // When a modal dialog is open, main-list hits beneath it are
            // blocked — but the dialog's OWN controls must stay live.
            // Otherwise every Close/Pause/slider inside Queues/Details/About
            // is skipped and the buttons look dead. Block only the main
            // window surface; allow popups and all dialog kinds.
            let dialog_kind = matches!(
                kind,
                HitKind::MenuItem(_)
                    | HitKind::CtxItem(_, _)
                    | HitKind::Btn(_)
                    | HitKind::Check(_)
                    | HitKind::Slider(..)
                    | HitKind::Field(_)
                    | HitKind::ComboAnchor
                    | HitKind::DetTab(_)
                    | HitKind::DetParts
            );
            if modal_open && !dialog_kind {
                continue;
            }
            let hov = self.man.hover(&rect);
            let clk = self.man.clicked(&rect);
            let sec = self.man.secondary(&rect);
            if hov && hover_dbg == "-" {
                hover_dbg = match &kind {
                    HitKind::Tool(t) => format!("tool:{t:?}"),
                    HitKind::SideRow(k) => format!("side:{k}"),
                    HitKind::Row(id) => format!("row:#{id}"),
                    HitKind::RowCheck(id) => format!("check:#{id}"),
                    HitKind::HdrCol(c) => format!("hdr:{}", c.key()),
                    HitKind::HdrCheck => "hdrcheck".to_string(),
                    HitKind::ResizeCol(c) => format!("rz:{}", c.key()),
                    HitKind::MenuBtn(m) => format!("menu:{m}"),
                    HitKind::MenuItem(a) => format!("menuitem:{a}"),
                    HitKind::CtxItem(_, id) => format!("ctx:#{id}"),
                    HitKind::Btn(_) => "btn".to_string(),
                    HitKind::Check(_) => "check".to_string(),
                    HitKind::Slider(..) => "slider".to_string(),
                    HitKind::Field(f) => format!("field:{f}"),
                    HitKind::ComboAnchor => "combo".to_string(),
                    HitKind::DetTab(t) => format!("dettab:{t}"),
                    HitKind::DetParts => "detparts".to_string(),
                };
            }
            if clk || sec {
                fired_any = true;
            }
            match kind {
                HitKind::Tool(t) => {
                    if clk {
                        self.diag_acts += 1;
                        // Dimmed actions are inert (a disabled button must
                        // never act, even if its rect is clicked).
                        let idle = self.tasks.is_empty();
                        match t {
                            ToolId::AddUrl => {
                                self.add_url.clear();
                                self.add_file.clear();
                                self.add_checksum.clear();
                                self.add_quality.clear();
                                self.show_add = true;
                            }
                            ToolId::StartQueue => self.start_queue(),
                            ToolId::StopQueue if !idle => self.stop_queue(),
                            ToolId::StopAll if !idle => self.stop_all(),
                            ToolId::StopQueue | ToolId::StopAll => {}
                            ToolId::Queues => self.show_queues = true,
                            ToolId::Settings => Self::open_settings_window(),
                            ToolId::Grabber => {
                                self.grab_notice.clear();
                                self.show_grabber = true;
                            }
                        }
                    }
                }
                HitKind::MenuBtn(m) => {
                    if clk {
                        fired_menu = true;
                        if self.drop_menu == Some(m) {
                            close_drop = true;
                        } else {
                            open_drop = Some(m);
                        }
                        self.ctx_menu = None;
                    }
                }
                HitKind::MenuItem(a) => {
                    if clk {
                        fired_menu = true;
                        self.drop_menu = None;
                        self.do_menu_act(ctx, a);
                    }
                }
                HitKind::SideRow(key) => {
                    if clk {
                        if key == "Queues" {
                            self.show_queues = true;
                        } else {
                            self.filter = key.clone();
                        }
                    }
                }
                HitKind::RowCheck(id) => {
                    if clk {
                        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                            d.selected = !d.selected;
                        }
                    }
                }
                HitKind::Row(id) => {
                    if sec {
                        fired_ctx = true;
                        for d in self.downloads.iter_mut() {
                            d.selected = d.id == id;
                        }
                        if let Some(p) = self.man.pos() {
                            open_ctx = Some((id, p));
                        }
                    } else if clk {
                        for d in self.downloads.iter_mut() {
                            d.selected = d.id == id;
                        }
                    }
                    if clk && self.man.take_double() {
                        self.do_row_act(RowAct::Details, id);
                    }
                }
                HitKind::HdrCol(c) => {
                    if clk {
                        if self.sort_col == c {
                            self.sort_asc = !self.sort_asc;
                        } else {
                            self.sort_col = c;
                            self.sort_asc = true;
                        }
                    }
                }
                HitKind::HdrCheck => {
                    if clk {
                        let ids = self.visible_ids();
                        let all = !ids.is_empty()
                            && ids.iter().all(|id| {
                                self.downloads
                                    .iter()
                                    .find(|d| d.id == *id)
                                    .map(|d| d.selected)
                                    .unwrap_or(false)
                            });
                        for id in &ids {
                            if let Some(d) =
                                self.downloads.iter_mut().find(|d| d.id == *id)
                            {
                                d.selected = !all;
                            }
                        }
                    }
                }
                HitKind::ResizeCol(c) => {
                    if hov {
                        zone_cursor = Some(egui::CursorIcon::ResizeColumn);
                    }
                    if self.man.press_started_in(&rect) {
                        if let Some(p) = self.man.pos() {
                            self.resize_cap = Some(ResizeCap::Col {
                                col: c,
                                start_x: p.x,
                                start_w: col_w_of(&self.col_w, c),
                            });
                        }
                    }
                }
                HitKind::CtxItem(a, id) => {
                    if clk {
                        fired_ctx = true;
                        self.ctx_menu = None;
                        self.do_row_act(a, id);
                    }
                }
                HitKind::Btn(b) => {
                    if clk {
                        self.do_button(b);
                    }
                }
                HitKind::Check(c) => {
                    if clk {
                        self.diag_acts += 1;
                        match c {
                            CheckId::CatFold => {
                                self.category_folders = !self.category_folders
                            }
                            CheckId::Tray => self.close_to_tray = !self.close_to_tray,
                            CheckId::Unlimited => {
                                self.speed_limit_kbps =
                                    if self.speed_limit_kbps == 0 { 1024 } else { 0 }
                            }
                            CheckId::SchedOn => {
                                self.scheduler_enabled = !self.scheduler_enabled
                            }
                            CheckId::AddOverwrite => {
                                self.add_overwrite = !self.add_overwrite
                            }
                            CheckId::GrabDepth => self.grab_depth = !self.grab_depth,
                            CheckId::WinOn => {
                                self.speed_windows_on = !self.speed_windows_on;
                                self.refresh_speed_limit();
                            }
                            CheckId::UpdOn => self.update_check_on = !self.update_check_on,
                        }
                        self.save_ui_state();
                    }
                }
                HitKind::Slider(id, min, max, log) => {
                    if self.man.press_started_in(&rect) {
                        if let Some(p) = self.man.pos() {
                            let (tx0, tw) = slider_track(&rect);
                            let frac = ((p.x - tx0) / tw).clamp(0.0, 1.0) as f64;
                            self.slider_set(id, Self::frac_to_val(frac, min, max, log));
                            self.slider_cap = Some(SliderCap {
                                id,
                                min,
                                max,
                                log,
                                x0: tx0,
                                tw,
                            });
                        }
                    }
                }
                HitKind::Field(fid) => {
                    if clk {
                        focus_field(ctx, fid);
                    }
                }
                HitKind::ComboAnchor => {
                    if clk {
                        fired_menu = true;
                        self.finish_open = !self.finish_open;
                    }
                }
                HitKind::DetTab(t) => {
                    if clk {
                        self.details_tab = t;
                    }
                }
                HitKind::DetParts => {
                    if clk {
                        self.det_parts_open = !self.det_parts_open;
                    }
                }
            }
        }
        if let Some(cur) = zone_cursor {
            ctx.set_cursor_icon(cur);
        }
        if let Some((id, pos)) = open_ctx {
            self.ctx_menu = Some((id, pos));
        }
        if let Some(m) = open_drop {
            self.drop_menu = Some(m);
            self.ctx_menu = None;
        }
        if close_drop {
            self.drop_menu = None;
        }
        // Clicks elsewhere dismiss open popups (standard behavior).
        if fired_any && !fired_menu && !fired_ctx {
            self.drop_menu = None;
            self.ctx_menu = None;
        }
        self.man_hover_dbg = hover_dbg;
    }
}
