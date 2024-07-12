use std::path::PathBuf;

use app::PadlsApp;

use eframe::{egui::Visuals, NativeOptions};

pub mod app;
pub mod bit;
pub mod gates;
pub mod gpu;
pub mod parser;
pub mod runner;

use clap::Parser;
#[derive(Parser, Debug)]
struct Args {
    script_path: PathBuf,
}

fn main() {
    let args = Args::parse();
    let app = PadlsApp::new(args.script_path);
    eframe::run_native(
        "padls",
        NativeOptions::default(),
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(Visuals::dark());
            Ok(Box::new(app))
        }),
    )
    .unwrap();
}
