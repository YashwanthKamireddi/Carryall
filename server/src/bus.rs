//! Optional Redis cross-instance bus for HA: relays each account's encrypted
//! frames between relay instances so a device can connect to *any* instance and
//! still see clips published on another. Redis carries ciphertext only.
//!
//! Each instance tags its publishes with a unique `origin` id. The subscriber
//! ignores frames it originated (already delivered locally), and applies frames
//! from other instances into its local rooms + backlog — so every instance ends
//! up mirroring the full stream and can serve catch-up to any device.

use std::sync::Arc;

use futures_util::StreamExt;
use redis::AsyncCommands;
use tokio::sync::mpsc;

use crate::relay::{Frame, Hub};

const CHANNEL_PREFIX: &str = "clip:room:";

/// Publish side: a non-blocking handle the (synchronous) `Hub::publish` uses to
/// hand frames to a background task that writes them to Redis.
pub struct RedisBus {
    tx: mpsc::UnboundedSender<(String, Vec<u8>)>,
}

impl RedisBus {
    /// Connect and spawn the writer task. Returns the publish handle.
    pub async fn connect(url: &str) -> redis::RedisResult<Arc<RedisBus>> {
        let client = redis::Client::open(url)?;
        let mut con = client.get_multiplexed_async_connection().await?;
        let (tx, mut rx) = mpsc::unbounded_channel::<(String, Vec<u8>)>();
        tokio::spawn(async move {
            while let Some((channel, payload)) = rx.recv().await {
                let _: redis::RedisResult<()> = con.publish(channel, payload).await;
            }
        });
        Ok(Arc::new(RedisBus { tx }))
    }

    /// Enqueue a frame for cross-instance fan-out (non-blocking).
    pub fn publish_remote(&self, account: &str, origin: &str, frame: &Frame) {
        let channel = format!("{CHANNEL_PREFIX}{account}");
        let _ = self.tx.send((channel, encode(origin, frame)));
    }
}

/// Subscribe to all rooms and apply frames from *other* instances into the hub.
pub async fn spawn_subscriber(url: &str, hub: Arc<Hub>, origin: String) -> redis::RedisResult<()> {
    let client = redis::Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.psubscribe(format!("{CHANNEL_PREFIX}*")).await?;
    tokio::spawn(async move {
        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let channel = msg.get_channel_name().to_string();
            let Some(account) = channel.strip_prefix(CHANNEL_PREFIX) else {
                continue;
            };
            let payload: Vec<u8> = match msg.get_payload() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if let Some((msg_origin, frame)) = decode(&payload) {
                if msg_origin != origin {
                    hub.apply_remote(account, frame);
                }
            }
        }
    });
    Ok(())
}

/// Wire format: `origin\nfrom\n` followed by the raw ciphertext bytes. Origin
/// and device ids never contain newlines.
fn encode(origin: &str, frame: &Frame) -> Vec<u8> {
    let mut v = format!("{origin}\n{}\n", frame.from).into_bytes();
    v.extend_from_slice(&frame.data);
    v
}

fn decode(payload: &[u8]) -> Option<(String, Frame)> {
    let n1 = payload.iter().position(|&b| b == b'\n')?;
    let origin = String::from_utf8_lossy(&payload[..n1]).into_owned();
    let rest = &payload[n1 + 1..];
    let n2 = rest.iter().position(|&b| b == b'\n')?;
    let from = String::from_utf8_lossy(&rest[..n2]).into_owned();
    let data = rest[n2 + 1..].to_vec();
    Some((origin, Frame { from, data }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let f = Frame {
            from: "deviceA".into(),
            data: vec![0, 1, 2, 255, b'\n', 7], // bytes incl. a newline
        };
        let (origin, got) = decode(&encode("inst-123", &f)).unwrap();
        assert_eq!(origin, "inst-123");
        assert_eq!(got.from, "deviceA");
        assert_eq!(got.data, f.data);
    }
}
