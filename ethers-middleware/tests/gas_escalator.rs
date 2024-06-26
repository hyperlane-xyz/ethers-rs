#![cfg(not(target_arch = "wasm32"))]
use std::convert::TryFrom;

use ethers_core::{types::*, utils::Anvil};
use ethers_middleware::{
    gas_escalator::{Frequency, GasEscalatorMiddleware, GeometricGasPrice},
    NonceManagerMiddleware, SignerMiddleware,
};
use ethers_providers::{Http, Middleware, Provider};
use ethers_signers::{LocalWallet, Signer};
use instant::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn gas_escalator_live() {
    let anvil = Anvil::new().port(8545u16).block_time(2u64).spawn();
    let chain_id = anvil.chain_id();
    let provider = Provider::<Http>::try_from(anvil.endpoint()).unwrap();

    // wrap with signer
    let wallet: LocalWallet = anvil.keys().first().unwrap().clone().into();
    let wallet = wallet.with_chain_id(chain_id);
    let address = wallet.address();
    let provider = SignerMiddleware::new(provider, wallet);

    // wrap with nonce manager
    // let nonce_manager_provider = NonceManagerMiddleware::new(provider, address);

    // wrap with escalator
    let escalator = GeometricGasPrice::new(5.0, 1u64, Some(2_000_000_000_000u64));
    let provider = GasEscalatorMiddleware::new(provider, escalator, Frequency::Duration(300));

    let nonce = provider.get_transaction_count(address, None).await.unwrap();
    // 1 gwei default base fee
    let gas_price = U256::from(1_000_000_000_u64);
    // 125000000000
    let tx = TransactionRequest::pay(Address::zero(), 1u64).gas_price(gas_price).nonce(nonce);
    // .chain_id(chain_id);

    eprintln!("sending");
    let pending = provider.send_transaction(tx, None).await.expect("could not send");
    eprintln!("waiting");
    let receipt = pending.await;
    //  match pending.await {
    //     Ok(receipt) => receipt.expect("dropped"),
    //     Err(e) => {
    //         eprintln!("reverted: {:?}", e);
    //         panic!()
    //     }
    // };
    // assert_eq!(receipt.from, address);
    // assert_eq!(receipt.to, Some(Address::zero()));
    println!("done escalating");
    sleep(Duration::from_secs(3)).await;
    // assert!(receipt.effective_gas_price.unwrap() > gas_price * 2, "{receipt:?}");
    println!(
        "receipt gas price: , hardcoded_gas_price: {}, receipt: {:?}",
        // receipt.effective_gas_price.unwrap(),
        gas_price,
        receipt
    );
}
