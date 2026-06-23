//! Clip sync relay server binary.
//!
//! Devices connect a WebSocket to `/sync?account=<id>&device=<id>`. Anything a
//! device sends is fanned out to the account's *other* devices verbatim. The
//! payloads are end-to-end encrypted by the clients (see `clip-sync::crypto`),
//! so this server only ever moves ciphertext — it cannot read clips.
//!
//! This is the cross-network "relay path"; the LAN fast-path (mDNS + WebRTC)
//! is handled client-side and bypasses this server when devices are co-located.

#[tokio::main]
async fn main() {
    let app = clip_relay::router_from_env().await;
    let addr = std::env::var("CLIP_RELAY_ADDR").unwrap_or_else(|_| "0.0.0.0:8787".into());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind relay address");
    eprintln!("clip-relay listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}
