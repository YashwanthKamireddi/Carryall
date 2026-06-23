//! Emits a hex ciphertext using the same key-derivation + AES-GCM as the app,
//! so an external Web Crypto client (the PWA / a Node check) can prove it
//! decrypts identically. Inputs are fixed to match the interop test script.

use clip_sync::{derive_key, encrypt};

fn main() {
    let account = "team";
    let secret = "passphrase123";

    // Salt scheme shared with the JS client: pad account to >= 16 bytes with '#'.
    let mut salt = account.as_bytes().to_vec();
    while salt.len() < 16 {
        salt.push(b'#');
    }

    let key = derive_key(secret.as_bytes(), &salt).unwrap();
    let ct = encrypt(&key, b"hello from rust");

    let hex: String = ct.iter().map(|b| format!("{b:02x}")).collect();
    println!("{hex}");
}
