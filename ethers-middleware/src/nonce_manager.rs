use async_trait::async_trait;
use ethers_core::types::{transaction::eip2718::TypedTransaction, *};
use ethers_providers::{FromErr, Middleware, PendingTransaction};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use thiserror::Error;
use tracing::instrument;

const DEFAULT_TX_COUNT_FOR_RESYNC: u64 = 10;

#[derive(Debug)]
/// Middleware used for calculating nonces locally, useful for signing multiple
/// consecutive transactions without waiting for them to hit the mempool
pub struct NonceManagerMiddleware<M> {
    inner: M,
    init_guard: futures_locks::Mutex<()>,
    initialized: AtomicBool,
    nonce: AtomicU64,
    tx_count_for_resync: Option<AtomicU64>,
    txs_since_resync: AtomicU64,
    address: Address,
}

impl<M> NonceManagerMiddleware<M>
where
    M: Middleware,
{
    /// Instantiates the nonce manager with a 0 nonce. The `address` should be the
    /// address which you'll be sending transactions from
    pub fn new(inner: M, address: Address) -> Self {
        Self {
            inner,
            init_guard: Default::default(),
            initialized: Default::default(),
            nonce: Default::default(),
            tx_count_for_resync: Default::default(),
            txs_since_resync: 0u64.into(),
            address,
        }
    }

    /// Returns the next nonce to be used
    pub fn next(&self) -> U256 {
        let nonce = self.nonce.fetch_add(1, Ordering::SeqCst);
        nonce.into()
    }

    pub fn get_tx_count_for_resync(&self) -> u64 {
        self.tx_count_for_resync
            .as_ref()
            .map(|count| count.load(Ordering::SeqCst))
            .unwrap_or(DEFAULT_TX_COUNT_FOR_RESYNC)
    }

    pub async fn initialize_nonce(
        &self,
        block: Option<BlockId>,
    ) -> Result<U256, NonceManagerError<M>> {
        if self.initialized.load(Ordering::SeqCst) {
            // return current nonce
            return Ok(self.nonce.load(Ordering::SeqCst).into());
        }

        let _guard = self.init_guard.lock().await;

        // do this again in case multiple tasks enter this codepath
        if self.initialized.load(Ordering::SeqCst) {
            // return current nonce
            return Ok(self.nonce.load(Ordering::SeqCst).into());
        }

        // initialize the nonce the first time the manager is called
        let nonce = self
            .inner
            .get_transaction_count(self.address, block)
            .await
            .map_err(NonceManagerError::MiddlewareError)?;
        self.nonce.store(nonce.as_u64(), Ordering::SeqCst);
        self.initialized.store(true, Ordering::SeqCst);
        Ok(nonce)
    } // guard dropped here

    async fn get_transaction_count_with_manager(
        &self,
        block: Option<BlockId>,
    ) -> Result<U256, NonceManagerError<M>> {
        // initialize the nonce the first time the manager is called
        if !self.initialized.load(Ordering::SeqCst) {
            let nonce = self
                .inner
                .get_transaction_count(self.address, block)
                .await
                .map_err(NonceManagerError::MiddlewareError)?;
            self.nonce.store(nonce.as_u64(), Ordering::SeqCst);
            self.initialized.store(true, Ordering::SeqCst);
        }

        Ok(self.next())
    }
}

#[derive(Error, Debug)]
/// Thrown when an error happens at the Nonce Manager
pub enum NonceManagerError<M: Middleware> {
    /// Thrown when the internal middleware errors
    #[error("{0}")]
    MiddlewareError(M::Error),
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<M> Middleware for NonceManagerMiddleware<M>
where
    M: Middleware,
{
    type Error = NonceManagerError<M>;
    type Provider = M::Provider;
    type Inner = M;

    fn inner(&self) -> &M {
        &self.inner
    }

    #[instrument(skip(self), name = "NonceManager::fill_transaction")]
    async fn fill_transaction(
        &self,
        tx: &mut TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<(), Self::Error> {
        if tx.nonce().is_none() {
            tx.set_nonce(self.get_transaction_count_with_manager(block).await?);
        }

        Ok(self
            .inner()
            .fill_transaction(tx, block)
            .await
            .map_err(NonceManagerError::MiddlewareError)?)
    }

    /// Signs and broadcasts the transaction. The optional parameter `block` can be passed so that
    /// gas cost and nonce calculations take it into account. For simple transactions this can be
    /// left to `None`.
    #[instrument(skip(self, tx, block), name = "NonceManager::send_transaction")]
    async fn send_transaction<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        tx: T,
        block: Option<BlockId>,
    ) -> Result<PendingTransaction<'_, Self::Provider>, Self::Error> {
        let mut tx = tx.into();

        if tx.nonce().is_none() {
            tx.set_nonce(self.get_transaction_count_with_manager(block).await?);
        }
        let nonce = tx.nonce();
        tracing::debug!(?nonce, "Sending transaction");
        match self.inner.send_transaction(tx.clone(), block).await {
            Ok(pending_tx) => {
                let new_txs_since_resync = self.txs_since_resync.fetch_add(1, Ordering::SeqCst);
                tracing::debug!(?nonce, "Sent transaction");
                let tx_count_for_resync = self.get_tx_count_for_resync();
                if new_txs_since_resync >= tx_count_for_resync {
                    let onchain_nonce = self.get_transaction_count(self.address, block).await?;
                    self.nonce.store(onchain_nonce.as_u64(), Ordering::SeqCst);
                    self.txs_since_resync.store(0, Ordering::SeqCst);
                    tracing::debug!(?nonce, "Resynced internal nonce with onchain nonce");
                } else {
                    self.txs_since_resync.store(new_txs_since_resync, Ordering::SeqCst);
                    let txs_until_resync = tx_count_for_resync - new_txs_since_resync;
                    tracing::debug!(?txs_until_resync, "Transactions until nonce resync");
                }
                Ok(pending_tx)
            }
            Err(err) => {
                tracing::error!(
                    ?nonce,
                    error=?err,
                    "Error sending transaction. Checking onchain nonce."
                );
                let onchain_nonce = self.get_transaction_count(self.address, block).await?;
                let internal_nonce = self.nonce.load(Ordering::SeqCst);
                if onchain_nonce != internal_nonce.into() {
                    // try re-submitting the transaction with the correct nonce if there
                    // was a nonce mismatch
                    self.nonce.store(onchain_nonce.as_u64(), Ordering::SeqCst);
                    tx.set_nonce(onchain_nonce);
                    tracing::warn!(
                        onchain_nonce=?onchain_nonce.as_u64(),
                        ?internal_nonce,
                        error=?err,
                        "Onchain nonce didn't match internal nonce. Resending transaction with updated nonce."
                    );
                    self.inner
                        .send_transaction(tx, block)
                        .await
                        .map_err(NonceManagerError::MiddlewareError)
                } else {
                    tracing::warn!(
                        onchain_nonce=?onchain_nonce.as_u64(),
                        ?internal_nonce,
                        error=?err,
                        "Onchain nonce matches internal nonce. Propagating error."
                    );
                    // propagate the error otherwise
                    Err(NonceManagerError::MiddlewareError(err))
                }
            }
        }
    }
}

// Boilerplate
impl<M: Middleware> FromErr<M::Error> for NonceManagerError<M> {
    fn from(src: M::Error) -> NonceManagerError<M> {
        NonceManagerError::MiddlewareError(src)
    }
}
