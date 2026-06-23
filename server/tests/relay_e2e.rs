//! End-to-end sync path test: two devices, one account, a real in-process
//! relay. Device A encrypts a clip and sends it; device B receives, decrypts,
//! and reconstructs the identical clip — proving relay fan-out + E2E crypto +
//! `Clip` serialization all line up, and that the relay only ever sees
//! ciphertext. This is the "beat Apple's Apple-only, single-item sync" path.

use std::time::Duration;

use clip_core::{Clip, ContentKind};
use clip_sync::{decrypt, derive_key, encrypt};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn two_devices_sync_a_clip_end_to_end() {
    // Start a real relay on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, clip_relay::router()).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let key = derive_key(b"a strong passphrase", b"account-salt-xx").unwrap();
    let base = format!("ws://{addr}/sync?account=team");

    let (mut dev_a, _) = tokio_tungstenite::connect_async(format!("{base}&device=A"))
        .await
        .unwrap();
    let (mut dev_b, _) = tokio_tungstenite::connect_async(format!("{base}&device=B"))
        .await
        .unwrap();
    // Ensure both have joined their room before publishing.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let clip = Clip {
        id: "content-hash-1".into(),
        kind: ContentKind::Link,
        preview: "https://clip.app".into(),
        body: Some(b"https://clip.app".to_vec()),
        blob_ref: None,
        size: 16,
        created_at: 1_700_000_000_000,
        pinned: false,
    };

    // A: encrypt + send.
    let ciphertext = encrypt(&key, &serde_json::to_vec(&clip).unwrap());
    dev_a.send(Message::Binary(ciphertext)).await.unwrap();

    // B: receive within a timeout, decrypt, reconstruct.
    let msg = tokio::time::timeout(Duration::from_secs(3), dev_b.next())
        .await
        .expect("device B should receive within timeout")
        .expect("stream not closed")
        .expect("no ws error");
    let data = match msg {
        Message::Binary(d) => d,
        other => panic!("expected binary frame, got {other:?}"),
    };

    let recovered: Clip = serde_json::from_slice(&decrypt(&key, &data).unwrap()).unwrap();
    assert_eq!(recovered, clip, "B reconstructs A's clip exactly");
}
