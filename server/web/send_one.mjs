// Connects, sends ONE encrypted clip, exits. Used to seed the relay backlog.
// Usage: node send_one.mjs <ws-url> <account> <secret> <text>
import { webcrypto as crypto } from "node:crypto";
const [, , RELAY, account = "yk-clip", secret = "clip-pass-2005", text = "durable clip"] = process.argv;
const enc = new TextEncoder();
let salt = account; while (salt.length < 16) salt += "#";
const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
const key = await crypto.subtle.deriveKey(
  { name: "PBKDF2", salt: enc.encode(salt), iterations: 200000, hash: "SHA-256" },
  base, { name: "AES-GCM", length: 256 }, false, ["encrypt"]);
const ws = new WebSocket(`${RELAY}?account=${account}&device=sender`);
ws.binaryType = "arraybuffer";
ws.onopen = async () => {
  const clip = { id: "dur1", kind: "text", preview: text, body: [...enc.encode(text)],
    blob_ref: null, size: text.length, created_at: Date.now(), pinned: false };
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, enc.encode(JSON.stringify(clip))));
  const out = new Uint8Array(12 + ct.length); out.set(iv, 0); out.set(ct, 12);
  ws.send(out);
  setTimeout(() => process.exit(0), 600);
};
