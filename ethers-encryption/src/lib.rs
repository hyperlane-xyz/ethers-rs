use serde::{Deserialize, Serialize};
use rand::{rngs::OsRng, RngCore};
use std::convert::TryInto;

pub mod derivation;
pub mod encryption;

/// Salt for derivation of transaction encryption key
pub const TX_KEY_PREFIX: &str = "IOEncryptionKeyV1";
/// Salt for derivation of user key material
pub const USER_KEY_PREFIX: &str = "UserEncryptionKeyV1";
/// Size of `tag` for DEOXYS-II encryption
pub const TAG_SIZE: usize = 16;
/// Size of `nonce` for DEOXYS-II encryption
pub const NONCE_SIZE: usize = 15;
/// Default size of private / public key
pub const KEY_SIZE: usize = 32;

/// Encrypts provided transaction or call data field
///
/// * node_url - URL of JSON-RPC to obtain node public key
/// * data – raw data to encrypt
///
/// Returns Some(encrypted_data, used_key) if encryption was successful, returns None in case
/// of error
pub async fn encrypt_data(
    node_public_key: [u8; 32],
    data: &[u8],
) -> Option<(Vec<u8>, [u8; KEY_SIZE])> {
    // Generate random encryption key
    let mut rng = OsRng;
    let mut key_material = [0u8; KEY_SIZE];
    rng.fill_bytes(&mut key_material);

    // Derive encryption key
    let encryption_key = derivation::derive_encryption_key(
        &key_material,
        USER_KEY_PREFIX.as_bytes(),
    );

    // Encrypt data
    let encrypted = encryption::encrypt_ecdh(
        encryption_key.clone(),
        node_public_key,
        data,
    );

    match encrypted {
        Ok(res) => Some((res.to_vec(), encryption_key)),
        Err(err) => {
            println!("Cannot encrypt transaction data. Reason: {:?}", err);
            None
        }
    }
}

/// Decrypts provided ciphertext, received as a node response
///
/// * node_url – URL of JSON-RPC to obtain node public key
/// * encryption_key – key, used during encryption of raw data
/// * data – ciphertext, received from node
///
/// Returns Some(decrypted_data) in case of success, otherwise returns None
pub async fn decrypt_data(node_public_key: [u8; 32], encryption_key: [u8; KEY_SIZE], data: &[u8]) -> Option<Vec<u8>> {
    // Decrypt data
    let decrypted = encryption::decrypt_ecdh(
        encryption_key,
        node_public_key,
        data,
    );

    match decrypted {
        Ok(res) => Some(res.to_vec()),
        Err(err) => {
            println!("Cannot decrypt node response. Reason: {:?}", err);
            None
        }
    }
}

pub fn convert_to_fixed_size_array(data: Vec<u8>) -> [u8; KEY_SIZE] {
    let mut fixed_array = [0u8; KEY_SIZE];
    fixed_array.copy_from_slice(&data[..KEY_SIZE]);
    fixed_array
}

