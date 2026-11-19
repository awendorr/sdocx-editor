//! sdocx-editor: a desktop editor for Samsung Notes `.sdocx` files.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod worker;

fn main() -> eframe::Result {
    let open = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("sdocx-editor")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "sdocx-editor",
        options,
        Box::new(move |cc| Ok(Box::new(app::EditorApp::new(cc, open)))),
    )
}
