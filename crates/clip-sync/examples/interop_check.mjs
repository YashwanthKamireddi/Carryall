// Decrypts a hex ciphertext (from `cargo run --example emit`) using the SAME
// Web Crypto primitives the iPhone PWA uses. If it prints "hello from rust",
// the Rust desktop and the browser share one E2E scheme.
import { webcrypto as crypto } from "node:crypto";

const account = "team";
const secret = "passphrase123";

let salt = account;
while (salt.length < 16) salt += "#";

const enc = new TextEncoder();
const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
const key = await crypto.subtle.deriveKey(
  { name: "PBKDF2", salt: enc.encode(salt), iterations: 200000, hash: "SHA-256" },
  base,
  { name: "AES-GCM", length: 256 },
  false,
  ["decrypt"],
);

const hex = process.argv[2];
const data = Uint8Array.from(Buffer.from(hex, "hex"));
const iv = data.slice(0, 12);
const ct = data.slice(12);
const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ct);
console.log(Buffer.from(pt).toString());
