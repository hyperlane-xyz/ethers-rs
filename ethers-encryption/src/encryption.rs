use sha2::Sha256;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use deoxys::aead::generic_array::GenericArray;
use deoxys::aead::{Aead, KeyInit, Payload};
use deoxys::DeoxysII256;

use crate::{KEY_SIZE, TX_KEY_PREFIX, NONCE_SIZE, TAG_SIZE };

use crate::derivation::{
    derive_shared_secret,
    derive_encryption_key,
    x25519_private_to_public
};

pub fn encrypt_ecdh(
    private_key: [u8; KEY_SIZE],
    node_public_key: [u8; KEY_SIZE],
    data: &[u8],
) -> Result<Vec<u8>, deoxys::Error> {
    let shared_secret = derive_shared_secret(private_key, node_public_key);
    let salt = TX_KEY_PREFIX.as_bytes();
    let encryption_key = derive_encryption_key(shared_secret.as_bytes(), salt);

    // Append encryption public key
    let encrypted_data = deoxys_encrypt(&encryption_key, data)?;
    let public_key = x25519_private_to_public(private_key);
    let mut result = Vec::<u8>::new();
    result.extend_from_slice(&public_key);
    result.extend(encrypted_data);

    Ok(result)
}

pub fn decrypt_ecdh(
    private_key: [u8; KEY_SIZE],
    node_public_key: [u8; KEY_SIZE],
    encrypted_data: &[u8],
) -> Result<Vec<u8>, deoxys::Error> {
    let shared_secret = derive_shared_secret(private_key, node_public_key);
    let salt = TX_KEY_PREFIX.as_bytes();
    let encryption_key = derive_encryption_key(shared_secret.as_bytes(), salt);
    deoxys_decrypt(&encryption_key, encrypted_data)
}

pub fn deoxys_encrypt(
    private_key: &[u8; KEY_SIZE],
    data: &[u8],
) -> Result<Vec<u8>, deoxys::Error> {
    let mut rng = OsRng;
    let mut aad = [0u8; TAG_SIZE];
    rng.fill_bytes(&mut aad);
    let mut nonce = [0u8; NONCE_SIZE];
    rng.fill_bytes(&mut nonce);
    let nonce = GenericArray::from_slice(&nonce);
    let payload = Payload {
        msg: data,
        aad: &aad,
    };
    let key = GenericArray::from_slice(private_key);
    let encrypted = DeoxysII256::new(key).encrypt(nonce, payload);
    match encrypted {
        Ok(ciphertext) => {
            let encrypted_data = [&nonce, aad.as_slice(), ciphertext.as_slice()].concat();
            Ok(encrypted_data)
        }
        Err(e) => Err(e)
    }
}

pub fn deoxys_decrypt(
    private_key: &[u8; KEY_SIZE],
    encrypted_data: &[u8],
) -> Result<Vec<u8>, deoxys::Error> {
    let nonce = &encrypted_data[0..NONCE_SIZE];
    let aad = &encrypted_data[NONCE_SIZE..NONCE_SIZE+TAG_SIZE];
    let ciphertext = &encrypted_data[NONCE_SIZE+TAG_SIZE..];
    let payload = Payload { msg: ciphertext, aad };
    let key = GenericArray::from_slice(private_key);
    DeoxysII256::new(key).decrypt( GenericArray::from_slice(nonce), payload)
}
