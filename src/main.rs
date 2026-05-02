mod app;
mod book;
mod extract;
mod import;
mod llm;
mod pdf;
mod scope;
mod settings;
mod subprocess;
mod theme;
mod ui;

use app::CkWriterApp;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CkWriter")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "CkWriter",
        opts,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(CkWriterApp::new(cc)))
        }),
    )
}
