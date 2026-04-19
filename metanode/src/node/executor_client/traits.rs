// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use async_trait::async_trait;
use consensus_core::CommittedSubDag;

use crate::node::executor_client::proto;
use crate::node::executor_client::proto::ValidatorInfo;
use crate::node::executor_client::ExecutorClient;
use crate::node::rpc_circuit_breaker::RpcCircuitBreaker;

#[allow(dead_code)]
#[async_trait]
pub trait TExecutorClient: Send + Sync {
    // 1. Basic flags
    fn is_enabled(&self) -> bool;
    fn can_commit(&self) -> bool;
    fn storage_path(&self) -> Option<&std::path::Path>;

    // 2. Circuit Breaker
    fn circuit_breaker(&self) -> &RpcCircuitBreaker;
    async fn check_send_circuit_breaker(&self) -> Result<()>;
    async fn record_send_failure(&self);
    async fn record_send_success(&self);
    async fn reset_connections(&self);

    // 3. RPC Queries
    async fn get_validators_at_block(
        &self,
        block_number: u64,
    ) -> Result<(Vec<ValidatorInfo>, u64, u64)>;
    async fn get_last_block_number(&self) -> Result<(u64, u64, bool)>;
    async fn get_last_global_exec_index(&self) -> Result<u64>;

    // 4. RPC Epoch Queries
    async fn get_epoch_boundary_data(
        &self,
        epoch: u64,
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)>;
    async fn get_safe_epoch_boundary_data(
        &self,
        epoch: u64,
        peer_rpc_addresses: &[String],
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)>;
    async fn get_current_epoch(&self) -> Result<u64>;
    async fn get_epoch_start_timestamp(&self, epoch: u64) -> Result<u64>;
    async fn advance_epoch(
        &self,
        new_epoch: u64,
        epoch_start_timestamp_ms: u64,
        boundary_block: u64,
        boundary_gei: u64,
    ) -> Result<()>;

    // 5. Block Sending
    async fn send_committed_subdag(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<u64>;
    async fn flush_buffer(&self) -> Result<()>;
    async fn send_committed_subdag_direct(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<()>;

    // 6. Block Sync
    async fn sync_blocks(&self, blocks: Vec<proto::BlockData>) -> Result<(u64, u64)>;
    async fn get_blocks_range(&self, start_block: u64, limit: u64)
        -> Result<Vec<proto::BlockData>>;

    // 7. Transition Handoff
    async fn set_consensus_start_block(&self, block_number: u64) -> Result<(bool, u64, String)>;
    async fn set_sync_start_block(&self, last_consensus_block: u64) -> Result<(bool, u64, String)>;
    async fn wait_for_sync_to_block(
        &self,
        target_block: u64,
        timeout_seconds: u64,
    ) -> Result<(bool, u64, String)>;

    // 8. Init
    async fn initialize_from_go(&self);
}

#[async_trait]
impl TExecutorClient for ExecutorClient {
    fn is_enabled(&self) -> bool {
        ExecutorClient::is_enabled(self)
    }
    fn can_commit(&self) -> bool {
        ExecutorClient::can_commit(self)
    }
    fn storage_path(&self) -> Option<&std::path::Path> {
        ExecutorClient::storage_path(self)
    }

    fn circuit_breaker(&self) -> &RpcCircuitBreaker {
        ExecutorClient::circuit_breaker(self)
    }
    async fn check_send_circuit_breaker(&self) -> Result<()> {
        ExecutorClient::check_send_circuit_breaker(self).await
    }
    async fn record_send_failure(&self) {
        ExecutorClient::record_send_failure(self).await
    }
    async fn record_send_success(&self) {
        ExecutorClient::record_send_success(self).await
    }
    async fn reset_connections(&self) {
        ExecutorClient::reset_connections(self).await
    }

    async fn get_validators_at_block(
        &self,
        block_number: u64,
    ) -> Result<(Vec<ValidatorInfo>, u64, u64)> {
        ExecutorClient::get_validators_at_block(self, block_number).await
    }
    async fn get_last_block_number(&self) -> Result<(u64, u64, bool)> {
        ExecutorClient::get_last_block_number(self).await
    }
    async fn get_last_global_exec_index(&self) -> Result<u64> {
        ExecutorClient::get_last_global_exec_index(self).await
    }

    async fn get_epoch_boundary_data(
        &self,
        epoch: u64,
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)> {
        ExecutorClient::get_epoch_boundary_data(self, epoch).await
    }
    async fn get_safe_epoch_boundary_data(
        &self,
        epoch: u64,
        peer_rpc_addresses: &[String],
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)> {
        ExecutorClient::get_safe_epoch_boundary_data(self, epoch, peer_rpc_addresses).await
    }
    async fn get_current_epoch(&self) -> Result<u64> {
        ExecutorClient::get_current_epoch(self).await
    }
    async fn get_epoch_start_timestamp(&self, epoch: u64) -> Result<u64> {
        ExecutorClient::get_epoch_start_timestamp(self, epoch).await
    }
    async fn advance_epoch(
        &self,
        new_epoch: u64,
        epoch_start_timestamp_ms: u64,
        boundary_block: u64,
        boundary_gei: u64,
    ) -> Result<()> {
        ExecutorClient::advance_epoch(
            self,
            new_epoch,
            epoch_start_timestamp_ms,
            boundary_block,
            boundary_gei,
        )
        .await
    }

    async fn send_committed_subdag(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<u64> {
        ExecutorClient::send_committed_subdag(
            self,
            subdag,
            epoch,
            global_exec_index,
            leader_address,
        )
        .await
    }
    async fn flush_buffer(&self) -> Result<()> {
        ExecutorClient::flush_buffer(self).await
    }
    async fn send_committed_subdag_direct(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<()> {
        ExecutorClient::send_committed_subdag_direct(
            self,
            subdag,
            epoch,
            global_exec_index,
            leader_address,
        )
        .await
    }

    async fn sync_blocks(&self, blocks: Vec<proto::BlockData>) -> Result<(u64, u64)> {
        ExecutorClient::sync_blocks(self, blocks).await
    }
    async fn get_blocks_range(
        &self,
        start_block: u64,
        limit: u64,
    ) -> Result<Vec<proto::BlockData>> {
        ExecutorClient::get_blocks_range(self, start_block, limit).await
    }

    async fn set_consensus_start_block(&self, block_number: u64) -> Result<(bool, u64, String)> {
        ExecutorClient::set_consensus_start_block(self, block_number).await
    }
    async fn set_sync_start_block(&self, last_consensus_block: u64) -> Result<(bool, u64, String)> {
        ExecutorClient::set_sync_start_block(self, last_consensus_block).await
    }
    async fn wait_for_sync_to_block(
        &self,
        target_block: u64,
        timeout_seconds: u64,
    ) -> Result<(bool, u64, String)> {
        ExecutorClient::wait_for_sync_to_block(self, target_block, timeout_seconds).await
    }

    async fn initialize_from_go(&self) {
        ExecutorClient::initialize_from_go(self).await
    }
}
