//! Font registration shared by every window.
//!
//! The Add-download dialog and the browser-capture popup are drawn to the
//! concept in `popup_concepts.html`, which asks for Segoe UI. Loading it as a
//! *named* family lets those dialogs use the exact face the design was drawn in
//! while the manager window keeps its own font. Missing system fonts fall back
//! to the bundled defaults, so this is safe on any machine.
//!
//! On top of that, every window gets the system's script fonts as *fallbacks*.
//! egui's bundled face is Latin only, so anything else - Bengali, Devanagari,
//! Tamil, Thai, Korean, Chinese - used to come out as tofu boxes: the file
//! name of a video titled in Bangla showed up as "Kalachan _ ▯▯▯▯▯▯ _ Tosiba".
//! Fallbacks are consulted per glyph, so the app's own typography is untouched
//! and only the characters it cannot draw are picked up from elsewhere.
//!
//! Fallbacks fix tofu, but they cannot fix *shaping*: epaint maps codepoints to
//! glyphs one at a time, so Bengali matras landed after their consonant and the
//! virama stayed visible instead of forming a conjunct. The Indic scripts
//! (`shaped`) therefore take a second route: rustybuzz shapes the run, ab_glyph
//! rasterises it and egui paints the result as a texture.

/// Named family holding Segoe UI regular (falls back to the app font).
pub const FONT_UI: &str = "segoe";
/// Named family holding Segoe UI Semibold (falls back to the app font).
pub const FONT_SEMI: &str = "popup-semibold";
/// Named family holding *only* Material Icons (see `install`).
pub const FONT_ICON: &str = "icons";

/// Segoe UI regular at `size`.
pub fn reg(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_UI.into()))
}

/// Material Icons glyph at `size`.
///
/// Icon codepoints sit in the Unicode Private Use Area, and the script
/// fallbacks deliberately include Segoe UI Symbol — which also maps PUA points
/// and therefore shadows the icon font. That is how U+E2C4 ("file_download")
/// came out as a little phone instead of a download arrow. Icons must go
/// through this family, where Material Icons is the only candidate.
pub fn icon(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_ICON.into()))
}

/// Segoe UI Semibold at `size` — the concept's 500/600 weights.
pub fn semi(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_SEMI.into()))
}

/// System files tried as script fallbacks: `(key, file, face index)`.
///
/// `Nirmala.ttc` carries the Indic scripts in one collection — Bengali
/// (also the national language of Bangladesh), Devanagari, Gujarati,
/// Gurmukhi, Kannada, Malayalam, Odia, Sinhala, Tamil and Telugu. The rest
/// cover the other scripts Windows ships fonts for.
#[cfg(windows)]
pub(crate) const SYSTEM_FONTS: &[(&str, &str, u32)] = &[
    ("nirmala", "Nirmala.ttc", 0),
    ("leelawadee", "LeelawUI.ttf", 0),
    ("myanmar", "mmrtext.ttf", 0),
    ("himalaya", "himalaya.ttf", 0),
    ("sylfaen", "sylfaen.ttf", 0),
    ("ebrima", "ebrima.ttf", 0),
    ("malgun", "malgun.ttf", 0),
    ("msyh", "msyh.ttc", 0),
    ("yugoth", "YuGothR.ttc", 0),
    ("seguisym", "seguisym.ttf", 0),
];

#[cfg(target_os = "macos")]
pub(crate) const SYSTEM_FONTS: &[(&str, &str, u32)] = &[
    ("arabic", "GeezaPro.ttc", 0), ("devanagari", "KohinoorDevanagari.ttc", 0),
    ("bengali", "KohinoorBangla.ttc", 0), ("cjk", "PingFang.ttc", 0),
];
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) const SYSTEM_FONTS: &[(&str, &str, u32)] = &[
    ("arabic", "NotoSansArabic-Regular.ttf", 0), ("devanagari", "NotoSansDevanagari-Regular.ttf", 0),
    ("bengali", "NotoSansBengali-Regular.ttf", 0), ("thai", "NotoSansThai-Regular.ttf", 0),
    ("cjk", "NotoSansCJK-Regular.ttc", 0),
];

/// Where the OS keeps its fonts. Missing optional fonts are skipped.
#[cfg(windows)]
pub(crate) fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    // A per-user font (`%LOCALAPPDATA%\Microsoft\Windows\Fonts`) is where a
    // user-installed script font would land, and it is checked first so it can
    // override the machine copy.
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(
            std::path::Path::new(&local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    let win = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    dirs.push(std::path::Path::new(&win).join("Fonts"));
    dirs
}

#[cfg(target_os = "macos")]
pub(crate) fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut paths = vec!["/System/Library/Fonts".into(), "/System/Library/Fonts/Supplemental".into(), "/Library/Fonts".into()];
    if let Some(h) = dirs::home_dir() { paths.insert(0, h.join("Library/Fonts")); }
    paths
}
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut paths = vec!["/usr/share/fonts/truetype/noto".into(), "/usr/share/fonts/opentype/noto".into(), "/usr/share/fonts/truetype/dejavu".into(), "/usr/local/share/fonts".into()];
    if let Some(h) = dirs::home_dir() { paths.insert(0, h.join(".local/share/fonts")); paths.push(h.join(".fonts")); }
    paths
}

/// Read one system font and register it under `key`.
///
/// Returns whether it was registered. epaint *panics* on a font it cannot
/// parse (it builds the faces on first use, outside any `catch_unwind`), so a
/// file that fails to parse is dropped here instead — a broken or exotic font
/// in the system directory must not take the app down with it.
fn load_system(
    fonts: &mut egui::FontDefinitions,
    key: &str,
    file: &str,
    index: u32,
) -> bool {
    {
        let bytes = font_dirs()
            .into_iter()
            .find_map(|dir| std::fs::read(dir.join(file)).ok());
        let Some(bytes) = bytes else {
            return false;
        };
        if ab_glyph::FontVec::try_from_vec_and_index(bytes.clone(), index).is_err() {
            return false;
        }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert(key.to_owned(), std::sync::Arc::new(data));
        true
    }
}

/// Register Material Icons, Segoe UI and the system script fallbacks.
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
    let has_reg = load_system(&mut fonts, "segoe", if cfg!(windows) { "segoeui.ttf" } else if cfg!(target_os="macos") { "Helvetica.ttc" } else { "DejaVuSans.ttf" }, 0);
    let has_semi = load_system(&mut fonts, "segoe_semi", if cfg!(windows) { "seguisb.ttf" } else if cfg!(target_os="macos") { "Helvetica.ttc" } else { "DejaVuSans-Bold.ttf" }, 0);
    fonts.font_data.insert("bundled-bengali".into(), std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansBengali.ttf"))));
    // Script fallbacks, in the order they are consulted. Segoe UI leads: it
    // covers Greek, Cyrillic, Arabic, Hebrew and the wider Latin ranges.
    let mut fallbacks: Vec<String> = Vec::new();
    if has_reg {
        fallbacks.push("segoe".to_owned());
    }
    for &(key, file, index) in SYSTEM_FONTS {
        if load_system(&mut fonts, key, file, index) {
            fallbacks.push(key.to_owned());
        }
    }
    fallbacks.push("bundled-bengali".into());
    {
        let prop = fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default();
        if primary && has_reg {
            prop.insert(0, "segoe".to_owned());
        }
        for key in &fallbacks {
            if !prop.contains(key) {
                prop.push(key.clone());
            }
        }
        prop.push("material".to_owned());
    }
    // Monospace falls back the same way: the URL and destination boxes are
    // drawn in it, and a name in any script has to stay readable there too.
    {
        let mono = fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default();
        for key in &fallbacks {
            if !mono.contains(key) {
                mono.push(key.clone());
            }
        }
    }
    // Named families: the regular cut prefers Segoe UI, then the app font, so
    // icon glyphs (Material Icons) and every script keep resolving inside the
    // dialog too.
    let base = fonts.families[&egui::FontFamily::Proportional].clone();
    let regular = if has_reg {
        let mut v = vec!["segoe".to_owned()];
        v.extend(base.iter().filter(|f| f.as_str() != "segoe").cloned());
        v
    } else {
        base
    };
    // Semibold keeps the concept's weight for Latin and falls through to the
    // same fallbacks for everything else (a Bangla heading is still semibold
    // *looking* because the fallback face is picked per glyph, not per run).
    let semibold = if has_semi {
        let mut v = vec!["segoe_semi".to_owned()];
        v.extend(regular.iter().filter(|f| f.as_str() != "segoe").cloned());
        v
    } else {
        regular.clone()
    };
    fonts
        .families
        .insert(egui::FontFamily::Name(FONT_UI.into()), regular);
    fonts
        .families
        .insert(egui::FontFamily::Name(FONT_SEMI.into()), semibold);
    // Icon-only family: Material Icons and nothing else, so no system font can
    // shadow a PUA icon codepoint (see `icon`).
    fonts.families.insert(
        egui::FontFamily::Name(FONT_ICON.into()),
        vec!["material".to_owned()],
    );
    cc.egui_ctx.set_fonts(fonts);
}
