//! `clip-core` — the platform-agnostic engine shared by the desktop app, the
//! (future) mobile app, and tests.
//!
//! It deliberately depends on **nothing platform-specific** (no Tauri, no OS
//! clipboard, no SQLite) so it compiles and unit-tests everywhere, including in
//! CI without a WebView. The Tauri layer (`src-tauri`) wires these pure pieces
//! to the real OS clipboard, storage, and UI.

pub mod classify;
pub mod hash;
pub mod model;

pub use classify::{classify_text, TextKind};
pub use hash::content_hash;
pub use model::{Clip, ContentKind, NewClip};
