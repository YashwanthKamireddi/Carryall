//! Clip — lightweight clipboard daemon + TUI with E2E cross-device sync.
//!
//!   clip            open the TUI to browse/search/paste history
//!   clip daemon     run the background capture + sync service
//!   clip --version  print version

mod blobs;
mod capture;
mod daemon;
mod tui;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("daemon") => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(daemon::run());
        }
        Some("--version") | Some("-V") => {
            println!("clip {}", env!("CARGO_PKG_VERSION"));
        }
        Some("--help") | Some("-h") => {
            println!("clip            open the TUI\nclip daemon     run the background service\nclip --version  version");
        }
        _ => {
            if let Err(e) = tui::run() {
                eprintln!("clip: {e}");
                std::process::exit(1);
            }
        }
    }
}
