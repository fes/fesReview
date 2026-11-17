mod backend;
#[cfg(target_os = "macos")]
mod controller;
mod ui;
fn main() -> eframe::Result {
    #[cfg(target_os = "macos")]
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let result = eframe::run_native(
        "fesReview",
        eframe::NativeOptions {
            run_and_return: true,
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1100.0, 760.0])
                .with_min_inner_size([720.0, 480.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(ui::ReviewApp::new(
                cc,
                #[cfg(target_os = "macos")]
                shutdown_tx,
            )))
        }),
    );
    #[cfg(target_os = "macos")]
    controller::finish_shutdown(shutdown_rx);
    result
}
