#![cfg(not(target_arch = "wasm32"))]
use std::convert::TryFrom;

use ethers_core::{types::*, utils::Anvil};
use ethers_middleware::{
    gas_escalator::{Frequency, GasEscalatorMiddleware, GeometricGasPrice},
    SignerMiddleware,
};
use ethers_providers::{Http, Middleware, Provider};
use ethers_signers::{LocalWallet, Signer};

#[tokio::test]
async fn gas_escalator_live() {
    let anvil = Anvil::new().block_time(2u64).spawn();
    let chain_id = anvil.chain_id();
    let provider = Provider::<Http>::try_from(anvil.endpoint()).unwrap();

    // wrap with signer
    let wallet: LocalWallet = anvil.keys().first().unwrap().clone().into();
    let wallet = wallet.with_chain_id(chain_id);
    let address = wallet.address();
    let provider = SignerMiddleware::new(provider, wallet);

    // wrap with escalator
    let escalator = GeometricGasPrice::new(5.0, 1u64, Some(2_000_000_000_000u64));
    let provider = GasEscalatorMiddleware::new(provider, escalator, Frequency::Duration(300));

    let nonce = provider.get_transaction_count(address, None).await.unwrap();
    // 1 gwei default base fee
    let gas_price = U256::from(1_000_000_000_u64);
    // 125_000_000_000
    let tx = TransactionRequest::pay(Address::zero(), 1u64)
        .gas_price(gas_price)
        .nonce(nonce)
        .chain_id(chain_id);

    // regardless of whether we get a receipt here, if gas is escalated by sending a new tx, this receipt won't be useful,
    // since a tx with a different hash will end up replacing
    let _pending = provider.send_transaction(tx, None).await.expect("could not send").await;
    // checking the logs shows that the gas price is indeed being escalated twice
}
