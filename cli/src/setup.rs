//! `carryall setup` — a one-shot, cross-platform first-run wizard. Writes the
//! sync config and (on Linux) installs the autostart service. Runs fully
//! interactively in a terminal, or zero-prompt in `--yes`/piped/non-TTY mode.
//! Every step validates and degrades gracefully — no panics on any OS.

use std::io::{self, IsTerminal, Write};

pub fn run() {
    let auto = std::env::args().any(|a| a == "-y" || a == "--yes" || a == "--auto")
        || !io::stdin().is_terminal();

    println!("\n  Carryall · setup");
    println!("  ────────────────\n");

    let path = crate::daemon::config_path();
    if path.as_ref().map(|p| p.exists()).unwrap_or(false) {
        // Don't silently clobber an existing config.
        if auto || !confirm("  You already have a config — reconfigure it?", false) {
            println!(
                "\n  Keeping your current setup. Add a phone any time with `carryall pair`.\n"
            );
            return;
        }
    }

    let default_account = std::env::var("USER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "me".into());
    let default_relay = "ws://127.0.0.1:8787/sync";

    let (account, mut secret, relay) = if auto {
        println!("  Running non-interactively — using defaults + a generated key.\n");
        (
            default_account.clone(),
            String::new(),
            default_relay.to_string(),
        )
    } else {
        let a = prompt(
            "  Account (a name shared across your devices)",
            &default_account,
        );
        let s = prompt_line("  Passphrase — your encryption key (Enter to generate one): ");
        let r = prompt(
            "  Relay URL (where devices connect; Enter for local)",
            default_relay,
        );
        (a, s, r)
    };

    if secret.is_empty() {
        secret = generate_passphrase();
        println!("\n  → Generated a strong passphrase:\n\n      {secret}\n");
        println!("    Use this exact passphrase on your phone (pairing fills it in for you).\n");
    }

    let Some(path) = path else {
        eprintln!("  ✗ couldn't determine the config location.");
        return;
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("  ✗ couldn't create {}: {e}", dir.display());
            return;
        }
    }
    let toml = format!("account = \"{account}\"\nsecret  = \"{secret}\"\nrelay   = \"{relay}\"\n");
    match std::fs::write(&path, toml) {
        Ok(_) => println!("  ✓ Saved {}", path.display()),
        Err(e) => {
            eprintln!("  ✗ couldn't write {}: {e}", path.display());
            return;
        }
    }

    if auto || confirm("\n  Start Carryall automatically on login?", true) {
        install_autostart();
    }

    println!("\n  Done — Carryall will capture and sync your clipboard.");
    println!("  • Add your phone:  carryall pair");
    println!("  • Run now:         carryall daemon\n");
}

fn prompt(label: &str, default: &str) -> String {
    print!("{label} [{default}]: ");
    let _ = io::stdout().flush();
    let mut s = String::new();
    if io::stdin().read_line(&mut s).is_err() {
        return default.to_string();
    }
    let s = s.trim();
    if s.is_empty() {
        default.to_string()
    } else {
        s.to_string()
    }
}

fn prompt_line(label: &str) -> String {
    print!("{label}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
    s.trim().to_string()
}

fn confirm(label: &str, default_yes: bool) -> bool {
    print!("{label} {}: ", if default_yes { "[Y/n]" } else { "[y/N]" });
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
    match s.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

/// A strong random passphrase from the OS CSPRNG (18 bytes → 24 url-safe chars).
/// Cross-platform via `getrandom`; the time fallback is only for the (practically
/// impossible) case where the OS RNG is unavailable.
fn generate_passphrase() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let mut buf = [0u8; 18];
    if getrandom::getrandom(&mut buf).is_ok() {
        URL_SAFE_NO_PAD.encode(buf)
    } else {
        format!("carryall-{:x}", crate::daemon::now_ms())
    }
}

/// Install autostart so the daemon runs on every login — one-shot on each OS:
/// systemd `--user` on Linux, a launchd agent on macOS, a hidden Startup-folder
/// launcher on Windows. Every step degrades to a clear manual hint, never an
/// error — `carryall daemon` always works by hand.
fn install_autostart() {
    #[cfg(target_os = "linux")]
    install_autostart_linux();
    #[cfg(target_os = "macos")]
    install_autostart_macos();
    #[cfg(target_os = "windows")]
    install_autostart_windows();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    println!("  ℹ Autostart isn't wired for this OS — run `carryall daemon` at login.");
}

/// Copy our running binary to a stable location the autostart entry points at,
/// so it never references a temporary build path. Returns the path to use in the
/// autostart entry (the deployed copy, or the current exe if a copy isn't
/// possible). Only compiled where an autostart backend exists.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn deploy_binary(target: &std::path::Path) -> std::path::PathBuf {
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(exe) = std::env::current_exe() else {
        return target.to_path_buf();
    };
    if exe == target {
        return exe;
    }
    let _ = std::fs::remove_file(target); // avoid ETXTBSY on a running binary
    if std::fs::copy(&exe, target).is_ok() {
        target.to_path_buf()
    } else {
        exe // couldn't copy — point the entry at where we are now
    }
}

#[cfg(target_os = "linux")]
fn install_autostart_linux() {
    let Ok(home) = std::env::var("HOME") else {
        println!("  (no HOME set — start manually: carryall daemon &)");
        return;
    };
    let home = std::path::Path::new(&home);

    // Put the binary where the unit expects it (~/.local/bin/carryall).
    deploy_binary(&home.join(".local/bin/carryall"));

    // Write the user service unit.
    let unit_dir = home.join(".config/systemd/user");
    if std::fs::create_dir_all(&unit_dir).is_err() {
        println!("  (couldn't create the systemd dir — start manually: carryall daemon &)");
        return;
    }
    let unit = "[Unit]\nDescription=Carryall clipboard daemon (background capture + E2E sync)\n\
                After=graphical-session.target\nPartOf=graphical-session.target\n\n\
                [Service]\nExecStart=%h/.local/bin/carryall daemon\nRestart=on-failure\nRestartSec=3\n\n\
                [Install]\nWantedBy=graphical-session.target\n";
    if std::fs::write(unit_dir.join("carryall-daemon.service"), unit).is_err() {
        println!("  (couldn't write the service file — start manually: carryall daemon &)");
        return;
    }

    // Enable + start it.
    let run = |args: &[&str]| {
        std::process::Command::new("systemctl")
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if run(&["--user", "daemon-reload"]) && run(&["--user", "enable", "--now", "carryall-daemon"]) {
        println!("  ✓ Autostart enabled — Carryall runs on every login.");
    } else {
        println!("  (systemctl --user unavailable — start manually: carryall daemon &)");
    }
}

#[cfg(target_os = "macos")]
fn install_autostart_macos() {
    let Ok(home) = std::env::var("HOME") else {
        println!("  (no HOME set — start manually: carryall daemon &)");
        return;
    };
    let home = std::path::Path::new(&home);
    let bin = deploy_binary(&home.join(".local/bin/carryall"));

    let agents = home.join("Library/LaunchAgents");
    if std::fs::create_dir_all(&agents).is_err() {
        println!("  (couldn't create LaunchAgents — start manually: carryall daemon &)");
        return;
    }
    let label = "com.carryall.daemon";
    let plist_path = agents.join(format!("{label}.plist"));
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         \t<key>Label</key><string>{label}</string>\n\
         \t<key>ProgramArguments</key><array><string>{bin}</string><string>daemon</string></array>\n\
         \t<key>RunAtLoad</key><true/>\n\
         \t<key>KeepAlive</key><true/>\n\
         </dict></plist>\n",
        bin = bin.display(),
    );
    if std::fs::write(&plist_path, plist).is_err() {
        println!("  (couldn't write the launch agent — start manually: carryall daemon &)");
        return;
    }
    // Reload: drop any previous copy (ignore errors), then load the new one.
    let _ = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(&plist_path)
        .status();
    let loaded = std::process::Command::new("launchctl")
        .arg("load")
        .arg(&plist_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if loaded {
        println!("  ✓ Autostart enabled (launchd) — Carryall runs on every login.");
    } else {
        println!("  (launchctl unavailable — start manually: carryall daemon &)");
    }
}

#[cfg(target_os = "windows")]
fn install_autostart_windows() {
    // Copy to a stable per-user location, then drop a launcher in the Startup
    // folder. A .vbs runs the daemon with a hidden window (0), so there's no
    // console flash at login.
    let Ok(local) = std::env::var("LOCALAPPDATA") else {
        println!("  (no LOCALAPPDATA — start manually: carryall daemon)");
        return;
    };
    let bin = deploy_binary(
        &std::path::Path::new(&local)
            .join("Carryall")
            .join("carryall.exe"),
    );

    let Ok(roaming) = std::env::var("APPDATA") else {
        println!("  (no APPDATA — start manually: carryall daemon)");
        return;
    };
    let startup =
        std::path::Path::new(&roaming).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup");
    if std::fs::create_dir_all(&startup).is_err() {
        println!("  (couldn't access the Startup folder — start manually: carryall daemon)");
        return;
    }
    // VBS quotes the path with Chr(34); window style 0 = hidden, wait = False.
    let vbs = format!(
        "Set s = CreateObject(\"WScript.Shell\")\r\n\
         s.Run Chr(34) & \"{bin}\" & Chr(34) & \" daemon\", 0, False\r\n",
        bin = bin.display(),
    );
    if std::fs::write(startup.join("carryall.vbs"), vbs).is_ok() {
        println!("  ✓ Autostart enabled (Startup folder) — Carryall runs on every login.");
    } else {
        println!("  (couldn't write the Startup entry — start manually: carryall daemon)");
    }
}
