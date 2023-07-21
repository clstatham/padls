#![allow(non_snake_case)]
#![allow(clippy::type_complexity)]

use std::{path::PathBuf, time::Duration};

use app::PadlsApp;

use eframe::NativeOptions;

pub mod app;
pub mod bit;
pub mod circuit;
pub mod ops;
pub mod parser;

pub const GLOBAL_QUEUE_INTERVAL: u32 = 1;
pub const CLOCK_FULL_INTERVAL: Duration = Duration::from_millis(1000);

pub const NUM_DISPLAYS: u8 = 4;

use clap::Parser;
#[derive(Parser, Debug)]
struct Args {
    script_path: PathBuf,
    #[arg(short, long, default_value_t = 1)]
    threads: u8,
}

fn main() {
    let args = Args::parse();
    let app = PadlsApp::new(args.threads, args.script_path);
    eframe::run_native(
        "padls",
        NativeOptions::default(),
        Box::new(|_cc| Box::new(app)),
    )
    .unwrap();
}
