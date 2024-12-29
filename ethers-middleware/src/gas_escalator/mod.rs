mod geometric;
pub use geometric::GeometricGasPrice;

mod linear;
pub use linear::LinearGasPrice;

use async_trait::async_trait;

use futures_util::lock::Mutex;
use instant::Instant;
use std::{pin::Pin, sync::Arc};
use thiserror::Error;
use tracing::{self, instrument};
use tracing_futures::Instrument;

use ethers_core::types::{transaction::eip2718::TypedTransaction, BlockId, TxHash, H256, U256};
use ethers_providers::{interval, FromErr, Middleware, PendingTransaction, StreamExt};

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

pub type ToEscalate = Arc<Mutex<Vec<MonitoredTransaction>>>;

#[cfg(target_arch = "wasm32")]
type WatcherFuture<'a> = Pin<Box<dyn futures_util::stream::Stream<Item = ()> + 'a>>;
#[cfg(not(target_arch = "wasm32"))]
type WatcherFuture<'a> = Pin<Box<dyn futures_util::stream::Stream<Item = ()> + Send + 'a>>;

/// Trait for fetching updated gas prices after a transaction has been first
/// broadcast
pub trait GasEscalator: Send + Sync + std::fmt::Debug {
    /// Given the initial gas price and the time elapsed since the transaction's
    /// first broadcast, it returns the new gas price
    fn get_gas_price(&self, initial_price: U256, time_elapsed: u64) -> U256;
}

/// Error thrown when the GasEscalator interacts with the blockchain
#[derive(Debug, Error)]
pub enum GasEscalatorError<M: Middleware> {
    #[error("{0}")]
    /// Thrown when an internal middleware errors
    MiddlewareError(M::Error),
}

/// The frequency at which transactions will be bumped
#[derive(Debug, Clone, Copy)]
pub enum Frequency {
    /// On a per block basis using the eth_newBlock filter
    PerBlock,
    /// On a duration basis (in milliseconds)
    Duration(u64),
}

#[derive(Debug)]
pub(crate) struct GasEscalatorMiddlewareInternal<M> {
    pub(crate) inner: Arc<M>,
    /// The transactions which are currently being monitored for escalation
    #[allow(clippy::type_complexity)]
    pub txs: ToEscalate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitoredTransaction {
    hash: TxHash,
    inner: TypedTransaction,
    creation_time: Instant,
    block: Option<BlockId>,
}

impl MonitoredTransaction {
    fn escalate_gas_price<E: GasEscalator>(&self, escalator: E) -> Option<TypedTransaction> {
        // Get the new gas price based on how much time passed since the
        // tx was last broadcast
        let time_elapsed = self.creation_time.elapsed().as_secs();
        match self.inner.clone() {
            TypedTransaction::Legacy(tx) => {
                let Some(gas_price) = tx.gas_price else {
                    return None;
                };
                let new_gas_price = escalator.get_gas_price(gas_price, time_elapsed);
                let mut updated_tx = tx.clone();
                updated_tx.gas_price = Some(new_gas_price);
                Some(updated_tx.into())
            }
            TypedTransaction::Eip2930(tx) => {
                let Some(gas_price) = tx.tx.gas_price else {
                    return None;
                };
                let new_gas_price = escalator.get_gas_price(gas_price, time_elapsed);
                let mut updated_tx = tx.clone();
                updated_tx.tx.gas_price = Some(new_gas_price);
                Some(updated_tx.into())
            }
            TypedTransaction::Eip1559(tx) => {
                let Some(max_fee_per_gas) = tx.max_fee_per_gas else {
                    return None;
                };
                let Some(max_priority_fee_per_gas) = tx.max_priority_fee_per_gas else {
                    return None;
                };
                let new_max_fee_per_gas = escalator.get_gas_price(max_fee_per_gas, time_elapsed);
                let new_max_priority_fee_per_gas =
                    escalator.get_gas_price(max_priority_fee_per_gas, time_elapsed);
                let mut updated_tx = tx.clone();
                updated_tx.max_fee_per_gas = Some(new_max_fee_per_gas);
                updated_tx.max_priority_fee_per_gas = Some(new_max_priority_fee_per_gas);
                Some(updated_tx.into())
            }
        }
    }
}

/// A Gas escalator allows bumping transactions' gas price to avoid getting them
/// stuck in the memory pool.
///
/// GasEscalator runs a background task which monitors the blockchain for tx
/// confirmation, and bumps fees over time if txns do not occur. This task
/// periodically loops over a stored history of sent transactions, and checks
/// if any require fee bumps. If so, it will resend the same transaction with a
/// higher fee.
///
/// Using [`GasEscalatorMiddleware::new`] will create a new instance of the
/// background task. Using [`GasEscalatorMiddleware::clone`] will crate a new
/// instance of the middleware, but will not create a new background task. The
/// background task is shared among all clones.
///
/// ## Footgun
///
/// If you drop the middleware, the background task will be dropped as well,
/// and any transactions you have sent will stop escalating. We recommend
/// holding an instance of the middleware throughout your application's
/// lifecycle, or leaking an `Arc` of it so that it is never dropped.
///
/// ## Outstanding issue
///
/// This task is fallible, and will stop if the provider's connection is lost.
/// If this happens, the middleware will become unable to properly escalate gas
/// prices. Transactions will still be dispatched, but no fee-bumping will
/// happen. This will also cause a memory leak, as the middleware will keep
/// appending to the list of transactions to escalate (and nothing will ever
/// clear that list).
///
/// We intend to fix this issue in a future release.
///
/// ## Example
///
/// ```no_run
/// use ethers_providers::{Provider, Http};
/// use ethers_middleware::{
///     gas_escalator::{GeometricGasPrice, Frequency, GasEscalatorMiddleware},
///     gas_oracle::{GasNow, GasCategory, GasOracleMiddleware},
/// };
/// use std::{convert::TryFrom, time::Duration, sync::Arc};
///
/// let provider = Provider::try_from("http://localhost:8545")
///     .unwrap()
///     .interval(Duration::from_millis(2000u64));
///
/// let provider = {
///     let escalator = GeometricGasPrice::new(5.0, 10u64, None::<u64>);
///     GasEscalatorMiddleware::new(provider, escalator, Frequency::PerBlock)
/// };
///
/// // ... proceed to wrap it in other middleware
/// let gas_oracle = GasNow::new().category(GasCategory::SafeLow);
/// let provider = GasOracleMiddleware::new(provider, gas_oracle);
/// ```
#[derive(Debug, Clone)]
pub struct GasEscalatorMiddleware<M> {
    pub(crate) inner: Arc<GasEscalatorMiddlewareInternal<M>>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<M> Middleware for GasEscalatorMiddleware<M>
where
    M: Middleware,
{
    type Error = GasEscalatorError<M>;
    type Provider = M::Provider;
    type Inner = M;

    fn inner(&self) -> &Self::Inner {
        &self.inner.inner
    }

    async fn send_transaction<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        tx: T,
        block: Option<BlockId>,
    ) -> Result<PendingTransaction<'_, Self::Provider>, Self::Error> {
        self.inner.send_transaction(tx, block).await
    }
}

impl<M> GasEscalatorMiddlewareInternal<M>
where
    M: Middleware,
{
    async fn send_transaction<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        tx: T,
        block: Option<BlockId>,
    ) -> Result<PendingTransaction<'_, M::Provider>, GasEscalatorError<M>> {
        let tx = tx.into();

        let pending_tx = self
            .inner
            .send_transaction(tx.clone(), block)
            .await
            .map_err(GasEscalatorError::MiddlewareError)?;

        // insert the tx in the pending txs
        let mut lock = self.txs.lock().await;
        lock.push(MonitoredTransaction {
            hash: *pending_tx,
            inner: tx,
            creation_time: Instant::now(),
            block,
        });

        Ok(pending_tx)
    }
}

impl<M> GasEscalatorMiddleware<M>
where
    M: Middleware,
{
    /// Initializes the middleware with the provided gas escalator and the chosen
    /// escalation frequency (per block or per second)
    #[allow(clippy::let_and_return)]
    #[cfg(not(target_arch = "wasm32"))]
    #[instrument(skip(inner, escalator, frequency))]
    pub fn new<E>(inner: M, escalator: E, frequency: Frequency) -> Self
    where
        E: GasEscalator + Clone + 'static,
        M: 'static,
    {
        let inner = Arc::new(inner);

        let txs: ToEscalate = Default::default();

        let this =
            Arc::new(GasEscalatorMiddlewareInternal { inner: inner.clone(), txs: txs.clone() });

        let esc = EscalationTask { inner, escalator, frequency, txs };

        spawn(esc.monitor().instrument(tracing::debug_span!("gas_escalation")));

        Self { inner: this }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct EscalationTask<M, E> {
    inner: M,
    escalator: E,
    frequency: Frequency,
    txs: ToEscalate,
}

const RETRYABLE_ERRORS: [&str; 3] =
    ["replacement transaction underpriced", "already known", "Fair pubdata price too high"];

#[cfg(not(target_arch = "wasm32"))]
impl<M, E: Clone> EscalationTask<M, E> {
    pub fn new(inner: M, escalator: E, frequency: Frequency, txs: ToEscalate) -> Self {
        Self { inner, escalator, frequency, txs }
    }

    /// Handles errors from broadcasting a gas-escalated transaction
    ///
    /// **Returns** `None`, if the transaction is to be dropped from the escalator, `Some` to keep monitoring and escalating it
    fn handle_broadcast_error(
        err_message: String,
        old_monitored_tx: MonitoredTransaction,
        new_tx: TypedTransaction,
    ) -> Option<(H256, Instant)> {
        if err_message.contains("nonce too low") {
            // may happen if we try to broadcast a new, gas-escalated tx when the original tx
            // already landed onchain, meaning we no longer need to escalate it
            tracing::warn!(err = err_message, ?old_monitored_tx.hash, ?new_tx, "Nonce error when escalating gas price. Tx may have already been included onchain. Dropping it from escalator");
            None
        } else if RETRYABLE_ERRORS.iter().any(|err_msg| err_message.contains(err_msg)) {
            // if the error is one of the known retryable errors, we can keep trying to escalate
            tracing::warn!(
                err = err_message,
                old_tx = ?old_monitored_tx.hash,
                new_tx = ?new_tx,
                "Encountered retryable error, re-adding to escalator"
            );
            // return the old tx_hash and creation time so the transaction is re-escalated
            // as soon as `monitored_txs` are evaluated again
            Some((old_monitored_tx.hash, old_monitored_tx.creation_time))
        } else {
            tracing::error!(
                err = err_message,
                old_tx = ?old_monitored_tx.hash,
                new_tx = ?new_tx,
                "Unexpected error when broadcasting gas-escalated transaction. Dropping it from escalator."
            );
            None
        }
    }

    /// Broadcasts the new transaction with the escalated gas price
    ///
    /// **Returns** a tx hash to monitor and the time it was created, unless the tx was already
    /// included or an unknown error occurred
    async fn broadcast_tx_if_escalated(
        &self,
        old_monitored_tx: MonitoredTransaction,
        new_tx: TypedTransaction,
    ) -> Option<(H256, Instant)>
    where
        M: Middleware,
        E: GasEscalator,
    {
        // gas price wasn't escalated
        // keep monitoring the old tx
        if old_monitored_tx.inner.eq(&new_tx) {
            return Some((old_monitored_tx.hash, old_monitored_tx.creation_time));
        }

        // send a replacement tx with the escalated gas price
        match self.inner.send_transaction(new_tx.clone(), old_monitored_tx.block).await {
            Ok(new_tx_hash) => {
                let new_tx_hash = *new_tx_hash;
                tracing::debug!(
                    old_tx = ?old_monitored_tx,
                    new_tx = ?new_tx,
                    new_tx_hash = ?new_tx_hash,
                    "escalated gas price"
                );
                // Return the new tx hash to monitor and the time it was created.
                // The latter is used to know when to escalate the gas price again
                Some((new_tx_hash, Instant::now()))
            }
            Err(err) => Self::handle_broadcast_error(err.to_string(), old_monitored_tx, new_tx),
        }
    }

    async fn escalate_stuck_txs(&self) -> Result<(), GasEscalatorError<M>>
    where
        M: Middleware,
        E: GasEscalator,
    {
        // We take monitored txs out of the mutex, and add them back if they weren't included yet
        let monitored_txs: Vec<_> = {
            let mut txs = self.txs.lock().await;
            std::mem::take(&mut (*txs))
            // Lock scope ends
        };

        if !monitored_txs.is_empty() {
            tracing::trace!(?monitored_txs, "In the escalator watcher loop. Monitoring txs");
        }
        let mut new_txs_to_monitor = vec![];
        for monitored_tx in monitored_txs {
            let receipt = self
                .inner
                .get_transaction_receipt(monitored_tx.hash)
                .await
                .map_err(GasEscalatorError::MiddlewareError)?;

            tracing::trace!(tx_hash = ?monitored_tx.hash, "checking if exists");

            if receipt.is_some() {
                // tx was already included, can drop from escalator
                tracing::debug!(tx = ?monitored_tx.hash, "Transaction was included onchain, dropping from escalator");
                continue;
            }
            let Some(new_tx) = monitored_tx.escalate_gas_price(self.escalator.clone()) else {
                tracing::error!(tx=?monitored_tx.hash, "gas price is not set for transaction, dropping from escalator");
                continue;
            };

            let maybe_tx_to_monitor =
                self.broadcast_tx_if_escalated(monitored_tx.clone(), new_tx.clone()).await;

            if let Some((new_txhash, new_creation_time)) = maybe_tx_to_monitor {
                new_txs_to_monitor.push(MonitoredTransaction {
                    hash: new_txhash,
                    inner: new_tx,
                    creation_time: new_creation_time,
                    block: monitored_tx.block,
                });
            }
        }
        // we add the new txs to monitor back to the list
        // we don't replace here, as the vec in the mutex may contain
        // items!
        self.txs.lock().await.extend(new_txs_to_monitor);
        Ok(())
    }

    async fn monitor(self) -> Result<(), GasEscalatorError<M>>
    where
        M: Middleware,
        E: GasEscalator,
    {
        // the escalation frequency is either on a per-block basis, or on a duration basis
        let escalation_frequency_watcher: WatcherFuture = match self.frequency {
            Frequency::PerBlock => Box::pin(
                self.inner
                    .watch_blocks()
                    .await
                    .map_err(GasEscalatorError::MiddlewareError)?
                    .map(|_| ()),
            ),
            Frequency::Duration(ms) => Box::pin(interval(std::time::Duration::from_millis(ms))),
        };

        let mut escalation_frequency_watcher = escalation_frequency_watcher.fuse();
        while escalation_frequency_watcher.next().await.is_some() {
            if let Err(err) = self.escalate_stuck_txs().await {
                tracing::error!(err = ?err, "error escalating stuck transactions");
            }
        }
        tracing::error!("timing future has gone away");
        Ok(())
    }
}

// Boilerplate
impl<M: Middleware> FromErr<M::Error> for GasEscalatorError<M> {
    fn from(src: M::Error) -> GasEscalatorError<M> {
        GasEscalatorError::MiddlewareError(src)
    }
}
