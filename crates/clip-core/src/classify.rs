//! Heuristic classification of *text* clipboard payloads into a richer
//! [`TextKind`]. Non-text representations (image/files/etc.) are classified by
//! the capture layer from the clipboard format itself; this module only refines
//! plain strings so the UI can show a link chip, a color swatch, or code styling.

use crate::model::ContentKind;
use url::Url;

/// Refinement of a plain-text payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    Link,
    Color,
    Code,
    Plain,
}

impl TextKind {
    pub fn content_kind(self) -> ContentKind {
        match self {
            TextKind::Link => ContentKind::Link,
            TextKind::Color => ContentKind::Color,
            TextKind::Code => ContentKind::Code,
            TextKind::Plain => ContentKind::Text,
        }
    }
}

/// Classify a plain-text clipboard string.
pub fn classify_text(s: &str) -> TextKind {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return TextKind::Plain;
    }
    if is_link(trimmed) {
        return TextKind::Link;
    }
    if is_color(trimmed) {
        return TextKind::Color;
    }
    if looks_like_code(trimmed) {
        return TextKind::Code;
    }
    TextKind::Plain
}

/// A single-line `http(s)`/`ftp`/`file` URL with a host.
fn is_link(s: &str) -> bool {
    if s.contains(['\n', '\r', ' ']) {
        return false;
    }
    match Url::parse(s) {
        Ok(u) => matches!(u.scheme(), "http" | "https" | "ftp" | "file") && u.has_host(),
        Err(_) => false,
    }
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, or a `rgb()/rgba()/hsl()` function.
fn is_color(s: &str) -> bool {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    let lower = s.to_ascii_lowercase();
    (lower.starts_with("rgb(")
        || lower.starts_with("rgba(")
        || lower.starts_with("hsl(")
        || lower.starts_with("hsla("))
        && lower.ends_with(')')
}

/// Cheap multi-line code heuristic: several lines and a density of
/// code-ish characters or a leading indent. Intentionally conservative — a
/// false negative just means plain-text styling, which is harmless.
fn looks_like_code(s: &str) -> bool {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() < 2 {
        return false;
    }
    let indented = lines
        .iter()
        .any(|l| l.starts_with("    ") || l.starts_with('\t'));
    let symbols = s
        .chars()
        .filter(|c| matches!(c, '{' | '}' | ';' | '(' | ')' | '=' | '<' | '>' | '#'))
        .count();
    indented || symbols >= lines.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_links() {
        assert_eq!(classify_text("https://example.com/path"), TextKind::Link);
        assert_eq!(classify_text("http://a.b"), TextKind::Link);
        // not a link: bare word, or text with spaces
        assert_eq!(classify_text("example"), TextKind::Plain);
        assert_eq!(classify_text("see https://x.com please"), TextKind::Plain);
    }

    #[test]
    fn detects_colors() {
        assert_eq!(classify_text("#ff0066"), TextKind::Color);
        assert_eq!(classify_text("#abc"), TextKind::Color);
        assert_eq!(classify_text("rgba(0,0,0,0.5)"), TextKind::Color);
        assert_eq!(classify_text("#zzz"), TextKind::Plain);
    }

    #[test]
    fn detects_code() {
        let code = "fn main() {\n    println!(\"hi\");\n}";
        assert_eq!(classify_text(code), TextKind::Code);
        assert_eq!(classify_text("just a sentence."), TextKind::Plain);
    }
}
