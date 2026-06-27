//! Carryall — a lightweight background clipboard daemon with E2E cross-device sync.
//!
//! No GUI, no TUI: it watches your native OS clipboard, syncs every copy to your
//! other devices end-to-end encrypted, and applies clips arriving from other
//! devices straight back onto your native clipboard — so what you copy on your
//! phone is instantly pasteable here.
//!
//!   carryall            run the daemon (foreground)
//!   carryall setup      one-shot first-run setup (config + autostart)
//!   carryall daemon     run the daemon (explicit)
//!   carryall pair       print a QR to pair your phone in one scan
//!   carryall --version  print version

mod apply;
mod blobs;
mod capture;
mod daemon;
mod lan;
mod proto;
mod setup;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => println!("carryall {}", env!("CARGO_PKG_VERSION")),
        Some("setup") => setup::run(),
        Some("pair") => daemon::print_pairing(),
        Some("--help") | Some("-h") => {
            println!(
                "carryall — background clipboard daemon with E2E cross-device sync\n\n\
                 USAGE:\n  carryall setup      first-run setup (config + autostart)\n  \
                 carryall [daemon]   run the daemon\n  carryall pair       \
                 print a QR to pair your phone\n  carryall --version\n\n\
                 Setup writes ~/.config/clip/config.toml (account, secret, relay).\n\
                 Run `carryall setup` to get started."
            );
        }
        _ => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(daemon::run());
        }
    }
}
