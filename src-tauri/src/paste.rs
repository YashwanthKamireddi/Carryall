//! Auto-paste: after Clip writes a chosen item to the clipboard and hides the
//! palette, synthesize a paste keystroke into whatever app regained focus —
//! so picking an item *pastes instantly*, the thing native history UIs can't do.
//!
//! Key synthesis is inherently OS/display-server specific, so we shell out to
//! the right tool rather than pull a heavy input crate:
//! - Wayland: `wtype` (virtual-keyboard protocol; works on wlroots/Hyprland)
//! - X11: `xdotool`
//!
//! A later cross-platform pass will use `enigo` for Windows/macOS.

use std::time::Duration;

/// Fire a paste keystroke shortly after the caller hides the palette, giving
/// the compositor time to return focus to the previous window. Best-effort:
/// if no synth tool is available it silently no-ops (the item is still on the
/// clipboard for a manual paste).
pub fn paste_after_focus_return() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(130));
        synthesize_paste();
    });
}

#[cfg(target_os = "linux")]
fn synthesize_paste() {
    use std::process::Command;

    let wayland = std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    if wayland && which("wtype") {
        // -M/-m press/release modifier; bare key types it. Sends Ctrl+V.
        let _ = Command::new("wtype")
            .args(["-M", "ctrl", "v", "-m", "ctrl"])
            .spawn();
        return;
    }
    if which("xdotool") {
        let _ = Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .spawn();
    }
}

#[cfg(not(target_os = "linux"))]
fn synthesize_paste() {
    // TODO: enigo-based synthesis for Windows/macOS in the cross-platform pass.
}

#[cfg(target_os = "linux")]
fn which(bin: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
