//! Fast, lightweight TUI to browse/search/paste clip history (clipse-style).
//! Reads the same store the daemon writes; on select, puts the clip on the OS
//! clipboard so you can paste.

use std::io::{self, Write};
use std::path::Path;

use clip_core::{Clip, ContentKind};
use clip_store::Store;
use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext, RustImageData};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};

use crate::daemon::data_dir;

fn kind_label(k: ContentKind) -> &'static str {
    match k {
        ContentKind::Text => "TEXT",
        ContentKind::Link => "LINK",
        ContentKind::Color => "COLOR",
        ContentKind::Code => "CODE",
        ContentKind::RichText => "RICH",
        ContentKind::Image => "IMAGE",
        ContentKind::Files => "FILES",
        ContentKind::Video => "VIDEO",
        ContentKind::Audio => "AUDIO",
        ContentKind::Binary => "BIN",
    }
}
fn kind_color(k: ContentKind) -> Color {
    match k {
        ContentKind::Link => Color::Rgb(77, 124, 254),
        ContentKind::Color => Color::Rgb(255, 159, 10),
        ContentKind::Code => Color::Rgb(48, 209, 88),
        ContentKind::Image => Color::Rgb(255, 55, 95),
        ContentKind::Files | ContentKind::Video => Color::Rgb(90, 200, 250),
        ContentKind::Audio => Color::Rgb(255, 214, 10),
        _ => Color::Rgb(142, 142, 147),
    }
}
fn rel_time(ms: i64) -> String {
    let s = (crate::daemon::now_ms() - ms).max(0) / 1000;
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

pub fn run() -> io::Result<()> {
    let dir = data_dir();
    let store = Store::open(dir.join("clip.db")).map_err(|e| io::Error::other(e.to_string()))?;
    let blob_dir = dir.join("blobs");
    let mut all: Vec<Clip> = store.recent(500).unwrap_or_default();

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let mut query = String::new();
    let mut sel: usize = 0;
    let mut chosen: Option<Clip> = None;

    loop {
        let q = query.to_lowercase();
        let view: Vec<&Clip> = all
            .iter()
            .filter(|c| q.is_empty() || c.preview.to_lowercase().contains(&q))
            .collect();
        if sel >= view.len() {
            sel = view.len().saturating_sub(1);
        }

        term.draw(|f| ui(f, &query, &view, sel))?;

        if event::poll(std::time::Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match k.code {
                    KeyCode::Esc => break,
                    KeyCode::Enter => {
                        if let Some(c) = view.get(sel) {
                            chosen = Some((*c).clone());
                        }
                        break;
                    }
                    KeyCode::Down => sel = (sel + 1).min(view.len().saturating_sub(1)),
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Backspace => {
                        query.pop();
                        sel = 0;
                    }
                    KeyCode::Char(ch) => {
                        // Delete with Ctrl+D, reload with Ctrl+R.
                        if k.modifiers.contains(event::KeyModifiers::CONTROL) && ch == 'd' {
                            if let Some(c) = view.get(sel) {
                                let id = c.id.clone();
                                let _ = store.delete(&id);
                                all.retain(|x| x.id != id);
                            }
                        } else if k.modifiers.contains(event::KeyModifiers::CONTROL) && ch == 'r' {
                            all = store.recent(500).unwrap_or_default();
                        } else {
                            query.push(ch);
                            sel = 0;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;

    if let Some(clip) = chosen {
        copy_to_clipboard(&clip, &blob_dir);
    }
    Ok(())
}

fn ui(f: &mut Frame, query: &str, view: &[&Clip], sel: usize) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(f.area());

    let search = Paragraph::new(format!(
        " {}",
        if query.is_empty() { "Search…" } else { query }
    ))
    .style(Style::new().fg(if query.is_empty() {
        Color::DarkGray
    } else {
        Color::White
    }))
    .block(
        Block::bordered()
            .border_style(Style::new().fg(Color::Rgb(60, 60, 67)))
            .title(" Clipboard "),
    );
    f.render_widget(search, chunks[0]);

    let items: Vec<ListItem> = view
        .iter()
        .map(|c| {
            let line = Line::from(vec![
                Span::styled("▌ ", Style::new().fg(kind_color(c.kind))),
                Span::styled(
                    format!("{:<6} ", kind_label(c.kind)),
                    Style::new()
                        .fg(kind_color(c.kind))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(one_line(&c.preview, 60)),
                Span::styled(
                    format!("   {}", rel_time(c.created_at)),
                    Style::new().fg(Color::DarkGray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let mut state = ListState::default();
    if !view.is_empty() {
        state.select(Some(sel));
    }
    let list = List::new(items)
        .highlight_style(
            Style::new()
                .bg(Color::Rgb(40, 40, 50))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    f.render_stateful_widget(list, chunks[1], &mut state);
}

fn one_line(s: &str, max: usize) -> String {
    let s: String = s.lines().next().unwrap_or("").chars().take(max).collect();
    s
}

/// Put a clip on the OS clipboard so the user can paste it.
fn copy_to_clipboard(clip: &Clip, blob_dir: &Path) {
    if clip.kind == ContentKind::Image {
        let bytes = clip
            .blob_ref
            .as_deref()
            .and_then(|r| crate::blobs::read(blob_dir, r))
            .or_else(|| clip.body.clone());
        if let Some(bytes) = bytes {
            if let (Ok(ctx), Ok(img)) = (ClipboardContext::new(), RustImageData::from_bytes(&bytes))
            {
                let _ = ctx.set_image(img);
            }
        }
        return;
    }
    let body = clip
        .body
        .clone()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_else(|| clip.preview.clone());
    if matches!(
        clip.kind,
        ContentKind::Files | ContentKind::Video | ContentKind::Audio
    ) && set_files_clipboard(&body)
    {
        return;
    }
    if let Ok(ctx) = ClipboardContext::new() {
        let _ = ctx.set_text(body);
    }
}

/// Wayland: put real files on the clipboard via wl-copy (text/uri-list).
fn set_files_clipboard(body: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let wayland = std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var("WAYLAND_DISPLAY")
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        if wayland {
            let uris = body
                .lines()
                .filter(|l| !l.is_empty())
                .map(|p| {
                    if p.starts_with("file://") {
                        p.to_string()
                    } else {
                        format!("file://{}", p.replace(' ', "%20"))
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            use std::process::{Command, Stdio};
            if let Ok(mut child) = Command::new("wl-copy")
                .args(["--type", "text/uri-list"])
                .stdin(Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(uris.as_bytes());
                }
                let _ = child.wait();
                return true;
            }
        }
    }
    let _ = body;
    false
}
