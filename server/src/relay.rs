//! The relay hub: one live broadcast room per account plus a pluggable,
//! optionally-durable backlog (see [`crate::backlog`]) replayed on connect.
//! Frames are **opaque ciphertext** — the server never decrypts.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;

use crate::backlog::{BacklogStore, MemoryBacklog};

/// A relayed frame: who sent it (so receivers skip their own) and the
/// end-to-end-encrypted payload.
#[derive(Clone, Debug)]
pub struct Frame {
    pub from: String,
    pub data: Vec<u8>,
}

/// Per-account live rooms + a backlog store + per-account connection caps,
/// optionally bridged to other instances via a Redis bus.
pub struct Hub {
    rooms: Mutex<HashMap<String, broadcast::Sender<Frame>>>,
    conns: Mutex<HashMap<String, usize>>,
    capacity: usize,
    max_conns: usize,
    backlog: Box<dyn BacklogStore>,
    /// This instance's id, tagged on bus publishes so we ignore our own echoes.
    origin: String,
    /// Cross-instance fan-out (HA). `None` = single-instance.
    bus: Option<std::sync::Arc<crate::bus::RedisBus>>,
}

/// Frames larger than this aren't kept in the catch-up backlog — they're blob
/// chunks (~350 KB base64), which must not be replayed in bulk on reconnect.
/// Clip metadata + text + control frames are well under this.
const BACKLOG_MAX_FRAME_BYTES: usize = 16 * 1024;

/// A unique-enough id for this relay process (for bus echo suppression).
pub fn gen_origin() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("inst-{}-{}", std::process::id(), nanos)
}

impl Default for Hub {
    fn default() -> Self {
        Self::new(2048, 8 * 1024 * 1024)
    }
}

impl Hub {
    /// In-memory backlog with the given live-channel capacity and backlog size.
    pub fn new(capacity: usize, backlog_cap: usize) -> Self {
        Self::with_backlog(capacity, Box::new(MemoryBacklog::new(backlog_cap)))
    }

    /// Use a custom (e.g. SQLite-durable) backlog store.
    pub fn with_backlog(capacity: usize, backlog: Box<dyn BacklogStore>) -> Self {
        Self {
            rooms: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
            capacity,
            max_conns: 32,
            backlog,
            origin: gen_origin(),
            bus: None,
        }
    }

    /// Attach a Redis bus for cross-instance fan-out. Call before sharing the
    /// hub; `origin` must match the id given to the bus subscriber.
    pub fn attach_bus(&mut self, bus: std::sync::Arc<crate::bus::RedisBus>, origin: String) {
        self.bus = Some(bus);
        self.origin = origin;
    }

    /// Join an account room: returns the backlog (catch-up) + a live receiver.
    pub fn join(&self, account: &str) -> (Vec<Frame>, broadcast::Receiver<Frame>) {
        let rx = {
            let mut rooms = self.rooms.lock().unwrap();
            rooms
                .entry(account.to_string())
                .or_insert_with(|| broadcast::channel(self.capacity).0)
                .subscribe()
        };
        (self.backlog.recent(account), rx)
    }

    /// Publish a frame from a locally-connected device: fan out to other
    /// instances (if a bus is attached), then deliver locally.
    pub fn publish(&self, account: &str, frame: Frame) -> usize {
        if let Some(bus) = &self.bus {
            bus.publish_remote(account, &self.origin, &frame);
        }
        self.deliver_local(account, frame)
    }

    /// Apply a frame received from another instance via the bus: deliver it
    /// locally (backlog + broadcast) without re-publishing to the bus.
    pub fn apply_remote(&self, account: &str, frame: Frame) {
        self.deliver_local(account, frame);
    }

    /// Append to this instance's backlog and broadcast to its live receivers.
    fn deliver_local(&self, account: &str, frame: Frame) -> usize {
        // Only small frames (clip metadata, text, pin/delete control) go in the
        // catch-up backlog. Large blob chunks still fan out LIVE, but are not
        // parked — replaying megabytes of old media at every reconnect floods
        // clients (a phone over cellular) into a reconnect loop. Media syncs
        // live when both devices are online; a reconnecting device catches up on
        // recent clips instantly and re-fetches media on the next copy.
        if frame.data.len() <= BACKLOG_MAX_FRAME_BYTES {
            self.backlog.append(account, &frame);
        }
        let rooms = self.rooms.lock().unwrap();
        match rooms.get(account) {
            Some(tx) => tx.send(frame).unwrap_or(0),
            None => 0,
        }
    }

    /// Reserve a connection slot for an account; false if at the cap.
    pub fn try_acquire(&self, account: &str) -> bool {
        let mut conns = self.conns.lock().unwrap();
        let n = conns.entry(account.to_string()).or_insert(0);
        if *n >= self.max_conns {
            return false;
        }
        *n += 1;
        true
    }

    /// Release a previously-acquired connection slot.
    pub fn release(&self, account: &str) {
        let mut conns = self.conns.lock().unwrap();
        if let Some(n) = conns.get_mut(account) {
            *n = n.saturating_sub(1);
        }
    }

    pub fn room_count(&self) -> usize {
        self.rooms.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(from: &str, data: &[u8]) -> Frame {
        Frame {
            from: from.into(),
            data: data.to_vec(),
        }
    }

    #[tokio::test]
    async fn fans_out_to_other_devices_in_same_account() {
        let hub = Hub::default();
        let (_, mut a) = hub.join("acct1");
        let (_, mut b) = hub.join("acct1");
        assert_eq!(hub.publish("acct1", frame("A", b"cipher")), 2);
        assert_eq!(a.recv().await.unwrap().from, "A");
        assert_eq!(b.recv().await.unwrap().data, b"cipher");
    }

    #[test]
    fn reconnecting_device_gets_backlog() {
        let hub = Hub::default();
        let _a = hub.join("acct1");
        hub.publish("acct1", frame("A", b"missed-1"));
        hub.publish("acct1", frame("A", b"missed-2"));
        let (backlog, _rx) = hub.join("acct1");
        let datas: Vec<_> = backlog.iter().map(|f| f.data.clone()).collect();
        assert_eq!(datas, vec![b"missed-1".to_vec(), b"missed-2".to_vec()]);
    }

    #[tokio::test]
    async fn accounts_are_isolated() {
        let hub = Hub::default();
        let (_, mut other) = hub.join("acct2");
        hub.join("acct1");
        hub.publish("acct1", frame("A", b"x"));
        assert!(other.try_recv().is_err());
    }

    #[test]
    fn connection_cap_enforced() {
        let hub = Hub::with_backlog(16, Box::new(MemoryBacklog::new(10)));
        // default max_conns is 32
        for _ in 0..32 {
            assert!(hub.try_acquire("a"));
        }
        assert!(!hub.try_acquire("a")); // 33rd rejected
        hub.release("a");
        assert!(hub.try_acquire("a")); // slot freed
    }
}
