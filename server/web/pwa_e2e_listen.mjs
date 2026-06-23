// Acts as the iPhone PWA: connects to the relay (over the public tunnel),
// decrypts every clip it receives, and prints it. Used to prove desktop→phone
// sync end-to-end. Usage: node pwa_e2e_listen.mjs <wss-url> <account> <secret>
import { webcrypto as crypto } from "node:crypto";

const [, , RELAY, account = "yk-clip", secret = "clip-pass-2005"] = process.argv;
const enc = new TextEncoder(), dec = new TextDecoder();

let salt = account; while (salt.length < 16) salt += "#";
const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
const key = await crypto.subtle.deriveKey(
  { name: "PBKDF2", salt: enc.encode(salt), iterations: 200000, hash: "SHA-256" },
  base, { name: "AES-GCM", length: 256 }, false, ["decrypt"]);

const ws = new WebSocket(`${RELAY}?account=${account}&device=phone-sim`);
ws.binaryType = "arraybuffer";
ws.onopen = () => console.error("phone-sim connected");
ws.onmessage = async (e) => {
  try {
    const u = new Uint8Array(e.data);
    const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: u.slice(0, 12) }, key, u.slice(12));
    const clip = JSON.parse(dec.decode(pt));
    console.log(`PHONE RECEIVED: [${clip.kind}] ${clip.preview}`);
  } catch (err) {
    console.error("decrypt failed:", err.message);
  }
};
setTimeout(() => process.exit(0), 12000);
