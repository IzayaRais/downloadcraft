mod app;
mod cli;
mod engine;
mod manual;
mod model;
mod native_host;

use app::CraftApp;
use clap::Parser;
use std::sync::Arc;

/// Persist panics to %APPDATA%/download_craft/crash.log so a crash can be
/// diagnosed after the fact (the message also goes to the console).
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let msg = format!("panic: {info}\nlocation: {:?}\nbacktrace:\n{bt}\n", info.location());
        if let Some(dir) = model::config_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), &msg);
        }
        eprintln!("{msg}");
        prev(info);
    }));
}

fn main() -> eframe::Result<()> {
    // Migrate legacy `idm_rust` config (queue/settings/inbox) forward.
    model::migrate_config();
    install_panic_hook();
    // Native-messaging host mode for the browser extension:
    // launched by Chrome/Edge/Firefox, speaks length-prefixed JSON on stdio.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--native-host") {
        return native_host::run();
    }
    // CLI mode when a subcommand is given (`--help` works too).
    if args.len() > 1 {
        match cli::Cli::try_parse_from(&args) {
            Ok(c) => {
                if let Err(e) = cli::run(c) {
                    eprintln!("error: {e:#}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            Err(e) => {
                // clap handles --help/--version (exit 0) vs real errors (exit 2).
                e.exit();
            }
        }
    }

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1050.0, 600.0])
        .with_min_inner_size([840.0, 480.0])
        .with_decorations(false);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        crate::model::APP_NAME,
        options,
        Box::new(|cc| {
            // Material Icons (Apache-2.0, Google) as fallback font for UI glyphs.
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "material".to_owned(),
                Arc::new(egui::FontData::from_static(include_bytes!(
                    "../assets/fonts/MaterialIcons-Regular.ttf"
                ))),
            );
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                fam.push("material".to_owned());
            }
            cc.egui_ctx.set_fonts(fonts);

            cc.egui_ctx.set_visuals(app::dark_visuals());
            let mut style = (*cc.egui_ctx.style()).clone();
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
            for w in [
                &mut style.visuals.widgets.inactive,
                &mut style.visuals.widgets.hovered,
                &mut style.visuals.widgets.active,
                &mut style.visuals.widgets.noninteractive,
                &mut style.visuals.widgets.open,
            ] {
                w.corner_radius = egui::CornerRadius::same(6);
            }
            style.spacing.icon_width = 14.0;
            cc.egui_ctx.set_style(style);

            Ok(Box::new(CraftApp::new(rt.clone())))
        }),
    )
}
