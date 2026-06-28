//! `clip-sync` — the cross-device sync engine.
//!
//! Pure and network-free so it unit-tests everywhere; the Tauri app and the
//! relay server drive it over real transports (LAN/WebRTC + WebSocket relay).
//!
//! Pieces:
//! - `hlc` — Hybrid Logical Clock for causal ordering across devices.
//! - `event` — content-addressed clip events + conflict-free (LWW) merge: how
//!   full history stays consistent everywhere (vs Apple's single 2-minute slot).
//! - `crypto` — end-to-end encryption; the relay only ever sees ciphertext.
//! - `chunk` — content-addressed chunking for resumable, dedup'd large blobs
//!   (files/video) — beating the "use AirDrop instead" punt.

pub mod chunk;
pub mod compress;
pub mod crypto;
pub mod event;
pub mod hlc;

pub use chunk::{chunk, chunk_hashes, CHUNK_SIZE};
pub use compress::{pack, unpack};
pub use crypto::{decrypt, derive_key, encrypt, CryptoError, Key};
pub use event::{merge, ClipEvent, Op};
pub use hlc::Hlc;
