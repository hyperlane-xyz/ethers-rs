use ethers_providers::Middleware;
use eyre::Result;

use ethers_core::{
    types::{BlockNumber, U256},
    utils::{
        eip1559_default_estimator, EIP1559_FEE_ESTIMATION_PAST_BLOCKS,
        EIP1559_FEE_ESTIMATION_REWARD_PERCENTILE,
    },
};

pub async fn estimate_eip1559_fees_default<M>(
    provider: &M,
    base_fee_per_gas: U256,
) -> Result<(U256, U256, U256)>
where
    M: Middleware,
{
    let fee_history = provider
        .fee_history(
            EIP1559_FEE_ESTIMATION_PAST_BLOCKS,
            BlockNumber::Latest,
            &[EIP1559_FEE_ESTIMATION_REWARD_PERCENTILE],
        )
        .await
        .map_err(|e| eyre::eyre!("Failed to fetch fee history: {}", e))?;

    // use the provided fee estimator function, or fallback to the default implementation.
    let (max_fee_per_gas, max_priority_fee_per_gas) =
        eip1559_default_estimator(base_fee_per_gas, fee_history.reward);

    Ok((base_fee_per_gas, max_fee_per_gas, max_priority_fee_per_gas))
}
