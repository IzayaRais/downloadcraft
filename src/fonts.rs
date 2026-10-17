//! Segoe UI registration shared by every window.
//!
//! The Add-download dialog and the browser-capture popup are drawn to the
//! concept in `popup_concepts.html`, which asks for Segoe UI. Loading it as a
//! *named* family lets those dialogs use the exact face the design was drawn in
//! while the manager window keeps its own font. Missing system fonts fall back
//! to the bundled defaults, so this is safe on any machine.

/// Named family holding Segoe UI regular (falls back to the app font).
pub const FONT_UI: &str = "segoe";
/// Named family holding Segoe UI Semibold (falls back to the app font).
pub const FONT_SEMI: &str = "popup-semibold";

/// Segoe UI regular at `size`.
pub fn reg(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_UI.into()))
}

/// Segoe UI Semibold at `size` — the concept's 500/600 weights.
pub fn semi(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_SEMI.into()))
}

/// Register Material Icons plus Segoe UI.
///
/// `primary` also promotes Segoe UI to the head of the proportional family,
/// which is what the standalone dialogs want (their whole surface is the
/// concept). The manager window passes `false`, so only the named families are
/// added and its own typography stays untouched.
pub fn install(cc: &eframe::CreationContext<'_>, primary: bool) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "material".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/MaterialIcons-Regular.ttf"
        ))),
    );
    let mut has_reg = false;
    let mut has_semi = false;
    for (key, file) in [("segoe", "segoeui.ttf"), ("segoe_semi", "seguisb.ttf")] {
        let path = std::path::Path::new("C:\\Windows\\Fonts").join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            fonts.font_data.insert(
                key.to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            if key == "segoe" {
                has_reg = true;
            } else {
                has_semi = true;
            }
        }
    }
    {
        let prop = fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default();
        if primary && has_reg {
            prop.insert(0, "segoe".to_owned());
        }
        prop.push("material".to_owned());
    }
    // Named families: the regular cut prefers Segoe UI, then the app font, so
    // icon glyphs (Material Icons) keep resolving inside the dialog too.
    let base = fonts.families[&egui::FontFamily::Proportional].clone();
    let regular = if has_reg {
        let mut v = vec!["segoe".to_owned()];
        v.extend(base.iter().filter(|f| f.as_str() != "segoe").cloned());
        v
    } else {
        base
    };
    let semibold = if has_semi {
        vec!["segoe_semi".to_owned(), "material".to_owned()]
    } else {
        regular.clone()
    };
    fonts
        .families
        .insert(egui::FontFamily::Name(FONT_UI.into()), regular);
    fonts
        .families
        .insert(egui::FontFamily::Name(FONT_SEMI.into()), semibold);
    cc.egui_ctx.set_fonts(fonts);
}
