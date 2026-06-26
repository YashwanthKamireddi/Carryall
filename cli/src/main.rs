//! Clip — a lightweight background clipboard daemon with E2E cross-device sync.
//!
//! No GUI, no TUI: it watches your native OS clipboard, syncs every copy to your
//! other devices end-to-end encrypted, and applies clips arriving from other
//! devices straight back onto your native clipboard — so what you copy on your
//! phone is instantly pasteable here.
//!
//!   clip            run the daemon (foreground)
//!   clip daemon     run the daemon (explicit)
//!   clip --version  print version

mod apply;
mod blobs;
mod capture;
mod daemon;
mod lan;
mod proto;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => println!("clip {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => {
            println!(
                "clip — background clipboard daemon with E2E cross-device sync\n\n\
                 USAGE:\n  clip [daemon]   run the daemon\n  clip --version\n\n\
                 Sync config: ~/.config/clip/config.toml (account, secret, relay)\n\
                 or env: CLIP_ACCOUNT, CLIP_SECRET, CLIP_RELAY_URL"
            );
        }
        _ => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(daemon::run());
        }
    }
}
