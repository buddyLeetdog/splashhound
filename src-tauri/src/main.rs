#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--scan") {
        if let Some(dir) = args.get(pos + 1) {
            inflatool::headless_scan(std::path::PathBuf::from(dir));
            return;
        }
    }
    inflatool::run();
}
