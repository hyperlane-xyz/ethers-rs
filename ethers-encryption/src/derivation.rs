use deoxys::aead::KeyInit;
use sha2::Sha256;
use hmac::{Hmac, Mac};

use crate::KEY_SIZE;

type HmacSha256 = Hmac<Sha256>;

/// Converts provided x25519 private key to public key
pub fn x25519_private_to_public(private_key: [u8; KEY_SIZE]) -> [u8; KEY_SIZE] {
    let secret = x25519_dalek::StaticSecret::from(private_key);
    let public_key = x25519_dalek::PublicKey::from(&secret);
    public_key.to_bytes()
}

/// Performs Diffie-Hellman derivation of encryption key for transaction encryption
/// * public_key – User public key
/// Returns shared secret which can be used for derivation of encryption key
pub fn derive_shared_secret(private_key: [u8; KEY_SIZE], public_key: [u8; KEY_SIZE]) -> x25519_dalek::SharedSecret {
    let secret = x25519_dalek::StaticSecret::from(private_key);
    secret.diffie_hellman(&x25519_dalek::PublicKey::from(public_key))
}

/// Derives encryption key using KDF
pub fn derive_encryption_key(private_key: &[u8], salt: &[u8]) -> [u8; KEY_SIZE] {
    let mut kdf = <HmacSha256 as KeyInit>::new_from_slice(salt).unwrap();
    kdf.update(private_key);
    let mut derived_key = [0u8; KEY_SIZE];
    let digest = kdf.finalize();
    derived_key.copy_from_slice(&digest.into_bytes()[..KEY_SIZE]);
    derived_key
}
