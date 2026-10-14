//! Temporary input diagnostic: standard vs custom-painted widgets.
/// Run with `cargo run --example input_probe`, observe, then delete.
use eframe::egui;

struct Probe {
    hovers: u64,
    clicks: u64,
    secondary: u64,
    box_hover_frames: u64,
    box_clicks: u64,
}

impl eframe::App for Probe {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Input probe — observe, then close");
            let p = ctx
                .pointer_hover_pos()
                .map(|p| format!("{},{}", p.x as i32, p.y as i32))
                .unwrap_or_else(|| "-".into());
            let r1 =
                ui.add(egui::Button::new("Standard Button — hover me, left/right click me"));
            if r1.hovered() {
                self.hovers += 1;
            }
            if r1.clicked() {
                self.clicks += 1;
            }
            if r1.secondary_clicked() {
                self.secondary += 1;
            }
            let (rect, r2) =
                ui.allocate_exact_size(egui::vec2(400.0, 30.0), egui::Sense::click());
            ui.painter().rect_filled(
                rect,
                4.0,
                if r2.hovered() {
                    egui::Color32::DARK_GREEN
                } else {
                    egui::Color32::DARK_RED
                },
            );
            if r2.hovered() {
                self.box_hover_frames += 1;
            }
            if r2.clicked() {
                self.box_clicks += 1;
            }
            ui.label(format!(
                "pointer: {p} | stdBtn hover-frames: {} left: {} right: {} | customBox hover-frames: {} clicks: {}",
                self.hovers, self.clicks, self.secondary, self.box_hover_frames, self.box_clicks
            ));
            ui.label("Box is GREEN while hovered, RED otherwise.");
            ctx.request_repaint();
        });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Input Probe",
        eframe::NativeOptions::default(),
        Box::new(|_| {
            Ok(Box::new(Probe {
                hovers: 0,
                clicks: 0,
                secondary: 0,
                box_hover_frames: 0,
                box_clicks: 0,
            }))
        }),
    )
}
