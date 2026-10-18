#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Download Craft: GUI + CLI + native host + standalone dialogs.
//! Release builds run without a console window; all diagnostics go to
//! %APPDATA%/download_craft/crash.log and the in-app log/queues panels.

mod aes128;
mod app;
mod basket;
mod cli;
mod complete;
mod confirm;
mod dm;
mod engine;
mod fonts;
mod ftp;
mod grabber;
mod manual;
mod model;
mod native_host;
mod progress;
mod settings;
mod tray;

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
    // IDM-style standalone "Download File Info" dialog (one OS window per
    // captured download, always on top). Spawned by the native host.
    if args.iter().any(|a| a == "--confirm-download") {
        return confirm::run();
    }
    // IDM-style standalone per-download progress window.
    if args.iter().any(|a| a == "--progress") {
        return progress::run();
    }
    // IDM-style standalone "Download complete" dialog (popped by the app when
    // a download finishes).
    if args.iter().any(|a| a == "--complete") {
        return complete::run();
    }
    // P3.3 companions: drop basket + mini status.
    if args.iter().any(|a| a == "--basket") {
        return basket::run_basket();
    }
    // Standalone Settings window (two-pane, separate OS window).
    if args.iter().any(|a| a == "--settings") {
        return settings::run();
    }
    if args.iter().any(|a| a == "--mini") {
        return basket::run_mini();
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
        .with_min_inner_size([840.0, 480.0]);
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
            crate::app::setup_fonts(cc);
            app::apply_theme(&cc.egui_ctx, &app::stored_theme());

            Ok(Box::new(CraftApp::new(rt.clone())))
        }),
    )
}
