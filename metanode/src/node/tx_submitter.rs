// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use consensus_core::BlockStatus;
use consensus_core::TransactionClient;
use tokio::sync::RwLock;

// Forward declaration to avoid circular import
// We'll use super::ConsensusNode

/// Small abstraction so RPC can keep working across in-process authority restarts.
#[async_trait]
pub trait TransactionSubmitter: Send + Sync {
    async fn submit(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> Result<(
        consensus_types::block::BlockRef,
        Vec<consensus_types::block::TransactionIndex>,
        tokio::sync::oneshot::Receiver<BlockStatus>,
    )>;

    async fn submit_no_wait(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<(
            consensus_types::block::BlockRef,
            Vec<consensus_types::block::TransactionIndex>,
            tokio::sync::oneshot::Receiver<BlockStatus>,
        )>,
    >;
}

/// Direct adapter for `consensus_core::TransactionClient`.
#[allow(dead_code)]
pub struct DirectTransactionSubmitter {
    client: Arc<TransactionClient>,
}

impl DirectTransactionSubmitter {
    #[allow(dead_code)]
    pub fn new(client: Arc<TransactionClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl TransactionSubmitter for DirectTransactionSubmitter {
    async fn submit(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> Result<(
        consensus_types::block::BlockRef,
        Vec<consensus_types::block::TransactionIndex>,
        tokio::sync::oneshot::Receiver<BlockStatus>,
    )> {
        // Map consensus-core error into anyhow.
        let (block_ref, indices, status) = self
            .client
            .submit(transactions)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit transaction: {}", e))?;
        Ok((block_ref, indices, status))
    }

    async fn submit_no_wait(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<(
            consensus_types::block::BlockRef,
            Vec<consensus_types::block::TransactionIndex>,
            tokio::sync::oneshot::Receiver<BlockStatus>,
        )>,
    > {
        self.client
            .submit_no_wait(transactions)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit transaction: {}", e))
    }
}

/// Proxy that can swap the inner TransactionClient on epoch transition.
pub struct TransactionClientProxy {
    inner: RwLock<Arc<TransactionClient>>,
}

impl TransactionClientProxy {
    pub fn new(initial: Arc<TransactionClient>) -> Self {
        Self {
            inner: RwLock::new(initial),
        }
    }

    pub async fn set_client(&self, new_client: Arc<TransactionClient>) {
        let mut guard = self.inner.write().await;
        *guard = new_client;
    }
}

#[async_trait]
impl TransactionSubmitter for TransactionClientProxy {
    async fn submit(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> Result<(
        consensus_types::block::BlockRef,
        Vec<consensus_types::block::TransactionIndex>,
        tokio::sync::oneshot::Receiver<BlockStatus>,
    )> {
        let client = { self.inner.read().await.clone() };
        let (block_ref, indices, status) = client
            .submit(transactions)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit transaction: {}", e))?;
        Ok((block_ref, indices, status))
    }

    async fn submit_no_wait(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<(
            consensus_types::block::BlockRef,
            Vec<consensus_types::block::TransactionIndex>,
            tokio::sync::oneshot::Receiver<BlockStatus>,
        )>,
    > {
        let client = { self.inner.read().await.clone() };
        client
            .submit_no_wait(transactions)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit transaction: {}", e))
    }
}

/// No-op submitter for SyncOnly nodes.
/// Always returns an error to trigger the forwarding path in TxSocketServer.
/// SyncOnly nodes cannot submit directly to consensus but can forward to validators.
pub struct NoOpTransactionSubmitter;

#[async_trait]
impl TransactionSubmitter for NoOpTransactionSubmitter {
    async fn submit(
        &self,
        _transactions: Vec<Vec<u8>>,
    ) -> Result<(
        consensus_types::block::BlockRef,
        Vec<consensus_types::block::TransactionIndex>,
        tokio::sync::oneshot::Receiver<BlockStatus>,
    )> {
        Err(anyhow::anyhow!(
            "SyncOnly node cannot submit to consensus directly"
        ))
    }

    async fn submit_no_wait(
        &self,
        _transactions: Vec<Vec<u8>>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<(
            consensus_types::block::BlockRef,
            Vec<consensus_types::block::TransactionIndex>,
            tokio::sync::oneshot::Receiver<BlockStatus>,
        )>,
    > {
        Err(anyhow::anyhow!(
            "SyncOnly node cannot submit to consensus directly"
        ))
    }
}
