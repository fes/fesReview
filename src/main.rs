mod backend;
mod ui;
fn main() -> eframe::Result {
    eframe::run_native(
        "fesReview",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1100.0, 760.0])
                .with_min_inner_size([720.0, 480.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(ui::ReviewApp::new(cc)))),
    )
}
