use serde_json::json;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use rand::{rngs::OsRng, RngCore};
use std::convert::TryInto;

pub mod derivation;
pub mod encryption;

pub const TX_KEY_PREFIX: &str = "IOEncryptionKeyV1";
pub const USER_KEY_PREFIX: &str = "UserEncryptionKeyV1";
pub const TAG_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 15;
pub const KEY_SIZE: usize = 32;

#[derive(Debug, Deserialize)]
struct NodePublicKeyResponse {
    result: String,
}

pub async fn encrypt_data(
    node_url: &str,
    data: &[u8],
) -> Option<(Vec<u8>, [u8; 32])> {
    // Get node public key
    let node_public_key = match get_node_public_key(node_url).await {
        Some(pk) => pk,
        None => {
            return None;
        }
    };

    // Generate random encryption key
    let mut rng = OsRng;
    let mut key_material = [0u8; KEY_SIZE];
    rng.fill_bytes(&mut key_material);

    // Derive encryption key
    let encryption_key = derivation::derive_encryption_key(
        &key_material,
        USER_KEY_PREFIX.as_bytes()
    );

    // Encrypt data
    let encrypted = encryption::encrypt_ecdh(
        encryption_key.clone(),
        node_public_key,
        data
    );

    match encrypted {
        Ok(res) => Some((res.to_vec(), encryption_key)),
        Err(err) => {
            println!("Cannot encrypt transaction data. Reason: {:?}", err);
            None
        }
    }
}

pub async fn decrypt_data(node_url: &str, encryption_key: [u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    let node_public_key = match get_node_public_key(node_url).await {
        Some(pk) => pk,
        None => {
            return None;
        }
    };

    // Decrypt data
    let decrypted = encryption::decrypt_ecdh(
        encryption_key,
        node_public_key,
        data
    );

    match decrypted {
        Ok(res) => Some(res.to_vec()),
        Err(err) => {
            println!("Cannot decrypt node response. Reason: {:?}", err);
            None
        }
    }
}

pub async fn get_node_public_key(url: &str) -> Option<[u8; 32]> {
    let url = match Url::parse(url) {
        Ok(url) => url,
        Err(err) => {
            println!("Cannot obtain node public key. Reason: {:?}", err);
            return None;
        }
    };

    let body = serde_json::json!({
        "id": 1,
        "jsonrpc": "2.0",
        "method": "eth_getNodePublicKey",
        "params": ["latest"],
    });

    let client = Client::new();
    let response = client.post(url).json(&body).send().await.ok()?;

    if !response.status().is_success() {
        println!("Request failed with status: {}", response.status());
        return None;
    }

    let api_response: NodePublicKeyResponse = response.json().await.ok()?;
    let result_str = api_response.result.trim_start_matches("0x");

    let res = match hex::decode(result_str) {
        Ok(res) => res,
        Err(err) => {
            println!("Cannot obtain node public key. Reason: {:?}", err);
            return None;
        }
    };

    Some(convert_to_fixed_size_array(res))
}

fn convert_to_fixed_size_array(data: Vec<u8>) -> [u8; 32] {
    let mut fixed_array = [0u8; 32];
    fixed_array.copy_from_slice(&data[..32]);
    fixed_array
}

fn bytearray_to_const_size<T, const N: usize>(v: Vec<T>) -> Option<[T; N]> {
    v.try_into().ok()
}