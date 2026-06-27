//! LAN peer-to-peer fast-path: direct device-to-device transfer over TCP on the
//! local network, skipping the relay hop for lowest latency. Same end-to-end
//! encryption and same clip/blob-chunk frames as the relay — only the transport
//! differs. Additive: the relay path keeps working; LAN is an extra delivery
//! route, and the daemon's per-session dedup guard prevents double-apply when a
//! clip arrives via both.
//!
//! Wire framing: `u32 LE length | encrypted-frame-bytes` (the same AES-GCM
//! ciphertext the relay carries). Discovery (mDNS) layers on top by supplying
//! peer addresses; a peer can also be set manually via `CLIP_PEER=host:port`.

use std::collections::HashMap;

use clip_core::Clip;
use clip_sync::{decrypt, encrypt, Key};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;

use crate::daemon::{finalize_apply, Ctx};

/// Default LAN port for direct peer transfer.
pub const DEFAULT_PORT: u16 = 8788;
/// Reject frames larger than this (abuse guard); blobs are chunked well under.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Start the LAN listener, optional manual peer, and mDNS auto-discovery. All
/// run for the daemon's lifetime. `device` and `acct_fp` (account fingerprint)
/// scope discovery to your own devices without leaking the account name.
pub fn spawn(ctx: Ctx, key: Key, port: u16, peer: Option<String>, device: String, acct_fp: String) {
    let lc = ctx.clone();
    let lk = key.clone();
    tokio::spawn(async move {
        if let Err(e) = listen(lc, lk, port).await {
            eprintln!("carryall lan: listener error: {e}");
        }
    });
    if let Some(addr) = peer {
        tokio::spawn(dial_loop(ctx.clone(), key.clone(), addr));
    }
    discover(ctx, key, port, device, acct_fp);
}

/// Best-effort mDNS discovery: advertise this device and dial same-account peers
/// found on the LAN. Any failure (no multicast, etc.) is logged and ignored —
/// the relay and manual-peer paths keep working.
fn discover(ctx: Ctx, key: Key, port: u16, device: String, acct_fp: String) {
    use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
    const SVC: &str = "_carryall._tcp.local.";

    let mdns = match ServiceDaemon::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("carryall lan: mDNS unavailable ({e}); relay/manual peer still active");
            return;
        }
    };

    // Advertise ourselves (account fingerprint + device id in TXT).
    let host = format!("{device}.local.");
    let props = [("acct", acct_fp.as_str()), ("dev", device.as_str())];
    match ServiceInfo::new(SVC, &device, &host, "", port, &props[..]) {
        Ok(info) => {
            let info = info.enable_addr_auto();
            if let Err(e) = mdns.register(info) {
                eprintln!("carryall lan: mDNS register failed: {e}");
            }
        }
        Err(e) => eprintln!("carryall lan: mDNS service info error: {e}"),
    }

    let browse = match mdns.browse(SVC) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("carryall lan: mDNS browse failed: {e}");
            return;
        }
    };

    tokio::spawn(async move {
        let mut dialed: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Ok(event) = browse.recv_async().await {
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            let acct = info.get_property_val_str("acct").unwrap_or_default();
            let peer_dev = info.get_property_val_str("dev").unwrap_or_default();
            // Only our own account, not ourselves.
            if acct != acct_fp || peer_dev == device {
                continue;
            }
            // Tiebreak: the lexicographically-smaller device id dials, so two
            // peers don't open duplicate connections to each other.
            if device.as_str() >= peer_dev {
                continue;
            }
            let p = info.get_port();
            if let Some(ip) = info.get_addresses().iter().next() {
                let addr = format!("{ip}:{p}");
                if dialed.insert(addr.clone()) {
                    eprintln!("carryall lan: discovered peer {peer_dev} at {addr}");
                    tokio::spawn(dial_loop(ctx.clone(), key.clone(), addr));
                }
            }
        }
    });
}

async fn listen(ctx: Ctx, key: Key, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("carryall lan: listening on :{port}");
    loop {
        let (sock, _addr) = listener.accept().await?;
        let (ctx, key) = (ctx.clone(), key.clone());
        tokio::spawn(async move {
            let _ = serve(ctx, key, sock).await;
        });
    }
}

async fn dial_loop(ctx: Ctx, key: Key, addr: String) {
    loop {
        if let Ok(sock) = TcpStream::connect(&addr).await {
            eprintln!("carryall lan: connected to peer {addr}");
            let _ = serve(ctx.clone(), key.clone(), sock).await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// Handle one peer connection bidirectionally: forward our captured clips to the
/// peer, and apply clips the peer sends us.
async fn serve(ctx: Ctx, key: Key, sock: TcpStream) -> std::io::Result<()> {
    let _ = sock.set_nodelay(true); // low latency
    let (mut rd, mut wr) = sock.into_split();

    // Writer task: forward locally-captured clips (+ their blobs) to this peer.
    let wkey = key.clone();
    let wctx = ctx.clone();
    let writer = tokio::spawn(async move {
        let mut outgoing = wctx.outgoing.subscribe();
        loop {
            match outgoing.recv().await {
                Ok(clip) => {
                    if send_clip(&mut wr, &wkey, &clip, &wctx).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });

    // Reader loop: receive frames, apply.
    let mut inbound = Inbound::default();
    while let Ok(frame) = read_frame(&mut rd).await {
        if let Ok(plain) = decrypt(&key, &frame) {
            inbound.handle(&ctx, &plain);
        }
    }
    writer.abort();
    Ok(())
}

/// Per-connection reassembly state (shared logic with the relay receive path).
#[derive(Default)]
struct Inbound {
    pending: HashMap<String, Clip>,
    chunks: crate::proto::Reassembler,
}

impl Inbound {
    fn handle(&mut self, ctx: &Ctx, plain: &[u8]) {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let Ok(val) = serde_json::from_slice::<serde_json::Value>(plain) else {
            return;
        };
        // A delete event — remove the clip from history (cross-device delete).
        if let Some(id) = val.get("delete").and_then(|v| v.as_str()) {
            if let Ok(store) = ctx.store.lock() {
                let _ = store.delete(id, crate::daemon::now_ms());
            }
            return;
        }
        // A pin toggle — sync pinned state across devices.
        if let Some(id) = val.get("pin").and_then(|v| v.as_str()) {
            let pinned = val.get("pinned").and_then(|v| v.as_bool()).unwrap_or(true);
            if let Ok(store) = ctx.store.lock() {
                let _ = store.set_pinned(id, pinned);
            }
            return;
        }
        if let Some(ref_) = val.get("blob_chunk").and_then(|v| v.as_str()) {
            // Already have this blob and not mid-transfer → ignore re-sends.
            if !self.pending.contains_key(ref_)
                && crate::blobs::existing_path(&ctx.blob_dir, ref_).is_some()
            {
                return;
            }
            let i = val.get("i").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let n = val.get("n").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let Some(b64) = val.get("b64").and_then(|v| v.as_str()) else {
                return;
            };
            let Ok(bytes) = STANDARD.decode(b64) else {
                return;
            };
            if let Some(whole) = self.chunks.add(ref_, i, n, bytes) {
                // Integrity: drop a blob whose content doesn't match its ref.
                if let Some(k) = self.pending.get(ref_).map(|c| c.kind) {
                    if !crate::blobs::verify_ref(k, ref_, &whole) {
                        eprintln!("carryall lan: blob integrity check failed ({ref_}); dropping");
                        self.pending.remove(ref_);
                        return;
                    }
                }
                crate::blobs::write(&ctx.blob_dir, ref_, &whole);
                if let Some(clip) = self.pending.remove(ref_) {
                    finalize_apply(ctx, clip);
                }
            }
            return;
        }
        let Ok(clip) = serde_json::from_value::<Clip>(val) else {
            return;
        };
        if let Ok(store) = ctx.store.lock() {
            let _ = store.upsert(&clip);
        }
        match clip.blob_ref.as_deref() {
            Some(r) if crate::blobs::existing_path(&ctx.blob_dir, r).is_none() => {
                crate::proto::pending_insert(&mut self.pending, r.to_string(), clip);
            }
            _ => finalize_apply(ctx, clip),
        }
    }
}

/// Send a clip frame, then stream its blob (if any) as chunk frames.
async fn send_clip(
    wr: &mut (impl AsyncWriteExt + Unpin),
    key: &Key,
    clip: &Clip,
    ctx: &Ctx,
) -> std::io::Result<()> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if let Ok(json) = serde_json::to_vec(clip) {
        write_frame(wr, &encrypt(key, &json)).await?;
    }
    if let Some(ref_) = clip.blob_ref.as_deref() {
        if let Some(path) = crate::blobs::existing_path(&ctx.blob_dir, ref_) {
            if let Ok(bytes) = std::fs::read(&path) {
                let n = bytes.len().div_ceil(crate::proto::CHUNK).max(1);
                for (i, part) in bytes.chunks(crate::proto::CHUNK).enumerate() {
                    let frame = serde_json::json!({
                        "blob_chunk": ref_, "i": i, "n": n, "b64": STANDARD.encode(part),
                    });
                    write_frame(wr, &encrypt(key, &serde_json::to_vec(&frame).unwrap())).await?;
                }
            }
        }
    }
    Ok(())
}

async fn read_frame(rd: &mut (impl AsyncReadExt + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    rd.read_exact(&mut len).await?;
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(std::io::Error::other("bad frame length"));
    }
    let mut buf = vec![0u8; n];
    rd.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame(wr: &mut (impl AsyncWriteExt + Unpin), data: &[u8]) -> std::io::Result<()> {
    wr.write_all(&(data.len() as u32).to_le_bytes()).await?;
    wr.write_all(data).await?;
    wr.flush().await
}
