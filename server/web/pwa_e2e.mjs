// Simulates the PWA's network + crypto path (the exact logic in app.js) against
// a live relay, using Node's global WebSocket + Web Crypto. One client sends an
// encrypted clip; a second ("phone") receives and decrypts it. Proves the
// iPhone PWA will see desktop clips. Usage: node pwa_e2e.mjs ws://host:port/sync
import { webcrypto as crypto } from "node:crypto";

const RELAY = process.argv[2];
const account = "team", secret = "passphrase123";
const enc = new TextEncoder(), dec = new TextDecoder();

let salt = account; while (salt.length < 16) salt += "#";
const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
const key = await crypto.subtle.deriveKey(
  { name: "PBKDF2", salt: enc.encode(salt), iterations: 200000, hash: "SHA-256" },
  base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);

const encryptClip = async (clip) => {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, enc.encode(JSON.stringify(clip))));
  const out = new Uint8Array(12 + ct.length); out.set(iv, 0); out.set(ct, 12); return out;
};
const decryptFrame = async (data) => {
  const u = new Uint8Array(data);
  const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: u.slice(0, 12) }, key, u.slice(12));
  return JSON.parse(dec.decode(pt));
};

const open = (device) => new Promise((res) => {
  const ws = new WebSocket(`${RELAY}?account=${account}&device=${device}`);
  ws.binaryType = "arraybuffer";
  ws.onopen = () => res(ws);
});

const desktop = await open("desktop");
const phone = await open("phone");
await new Promise((r) => setTimeout(r, 200)); // ensure both joined the room

const received = new Promise((res, rej) => {
  phone.onmessage = async (e) => { try { res(await decryptFrame(e.data)); } catch (err) { rej(err); } };
  setTimeout(() => rej(new Error("timeout: phone got nothing")), 3000);
});

const clip = { id: "abc123", kind: "link", preview: "https://clip.app",
  body: [...enc.encode("https://clip.app")], blob_ref: null, size: 16, created_at: Date.now(), pinned: false };
desktop.send(await encryptClip(clip));

const got = await received;
if (got.preview === clip.preview && got.kind === clip.kind) {
  console.log("PWA E2E OK: phone decrypted →", got.kind, got.preview);
  process.exit(0);
} else {
  console.error("MISMATCH:", got); process.exit(1);
}
