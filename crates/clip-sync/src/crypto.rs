//! End-to-end encryption for synced payloads.
//!
//! AES-256-GCM (authenticated) with a key derived from the user's passphrase
//! via PBKDF2-HMAC-SHA256. The key never leaves the device; the relay and
//! network only ever carry the ciphertext produced here.
//!
//! Wire format: `nonce(12) || ciphertext+tag`.
//!
//! PBKDF2 (rather than Argon2) is deliberate: the browser PWA derives the
//! identical key with the native Web Crypto API, with no WASM — so a phone in
//! Safari and the Rust desktop share one E2E scheme. [`PBKDF2_ITERATIONS`] and
//! the salt derivation must stay in lockstep with the JS client.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;

const NONCE_LEN: usize = 12;

/// PBKDF2 iteration count. Must equal the PWA's `iterations`.
pub const PBKDF2_ITERATIONS: u32 = 200_000;

/// A 256-bit symmetric key. Not serializable on purpose — it stays in memory.
#[derive(Clone)]
pub struct Key([u8; 32]);

impl Key {
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Key(b)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    Decrypt,
}

/// Derive the account key from a passphrase and a per-account salt.
/// PBKDF2-HMAC-SHA256, [`PBKDF2_ITERATIONS`] rounds, 256-bit output.
pub fn derive_key(passphrase: &[u8], salt: &[u8]) -> Result<Key, CryptoError> {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase, salt, PBKDF2_ITERATIONS, &mut out);
    Ok(Key(out))
}

/// Encrypt `plaintext`, returning `nonce || ciphertext+tag`.
pub fn encrypt(key: &Key, plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(&key.0).expect("32-byte key");
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .expect("AES-GCM encryption is infallible for valid input");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Decrypt data produced by [`encrypt`]. Fails on a wrong key or tampering
/// (GCM authentication), or if the input is too short to hold a nonce.
pub fn decrypt(key: &Key, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::Decrypt);
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(&key.0).map_err(|_| CryptoError::Decrypt)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = derive_key(b"correct horse battery staple", b"clip-salt-1").unwrap();
        let msg = b"a clipboard secret \x00\xff with binary";
        let ct = encrypt(&key, msg);
        assert_ne!(&ct[12..], &msg[..]); // actually encrypted
        assert_eq!(decrypt(&key, &ct).unwrap(), msg);
    }

    #[test]
    fn wrong_key_fails() {
        let k1 = derive_key(b"pass-one", b"clip-salt-1").unwrap();
        let k2 = derive_key(b"pass-two", b"clip-salt-1").unwrap();
        let ct = encrypt(&k1, b"hello");
        assert_eq!(decrypt(&k2, &ct), Err(CryptoError::Decrypt));
    }

    #[test]
    fn tamper_is_detected() {
        let key = derive_key(b"pass", b"clip-salt-1").unwrap();
        let mut ct = encrypt(&key, b"hello world");
        let last = ct.len() - 1;
        ct[last] ^= 0x01; // flip a ciphertext bit
        assert_eq!(decrypt(&key, &ct), Err(CryptoError::Decrypt));
    }

    #[test]
    fn same_passphrase_same_key_different_salt_differs() {
        let a = encrypt(&derive_key(b"p", b"salt-aaaa").unwrap(), b"x");
        // Different salt → different key → can't decrypt.
        let kb = derive_key(b"p", b"salt-bbbb").unwrap();
        assert_eq!(decrypt(&kb, &a), Err(CryptoError::Decrypt));
    }

    #[test]
    fn nonce_makes_ciphertexts_unique() {
        let key = derive_key(b"pass", b"clip-salt-1").unwrap();
        assert_ne!(encrypt(&key, b"same"), encrypt(&key, b"same"));
    }
}
