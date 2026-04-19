use crate::consensus::tx_recycler::TxRecycler;
use crate::node::tx_submitter::TransactionSubmitter;
use crate::node::ConsensusNode;
use anyhow::Result;
use consensus_core;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

pub struct TxSocketServer {
    transaction_client: Arc<dyn TransactionSubmitter>,
    node: Option<Arc<Mutex<ConsensusNode>>>,
    is_transitioning: Option<Arc<AtomicBool>>,
    peer_rpc_addresses: Vec<String>,
    peer_discovery_addresses: Option<Arc<RwLock<Vec<String>>>>,
    tx_recycler: Option<Arc<TxRecycler>>,
}

impl TxSocketServer {
    pub fn with_node(
        transaction_client: Arc<dyn TransactionSubmitter>,
        node: Option<Arc<Mutex<ConsensusNode>>>,
        is_transitioning: Option<Arc<AtomicBool>>,
        peer_rpc_addresses: Vec<String>,
    ) -> Self {
        Self {
            transaction_client,
            node,
            is_transitioning,
            peer_rpc_addresses,
            peer_discovery_addresses: None,
            tx_recycler: None,
        }
    }

    pub fn with_peer_discovery(mut self, addresses: Arc<RwLock<Vec<String>>>) -> Self {
        self.peer_discovery_addresses = Some(addresses);
        self
    }

    pub fn with_tx_recycler(mut self, recycler: Arc<TxRecycler>) -> Self {
        self.tx_recycler = Some(recycler);
        self
    }

    pub async fn start(self) -> Result<()> {
        let (ffi_tx_sender, mut ffi_tx_receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(1000);
        if crate::ffi::FFI_TX_SENDER.set(ffi_tx_sender).is_err() {
            warn!("⚠️ [FFI TX SENDER] Already initialized!");
        }
        info!("🔌 FFI Transaction Receiver started in place of UDS server");

        let client = self.transaction_client;
        let node = self.node;
        let is_transitioning = self.is_transitioning;
        let peer_rpc_addresses = self.peer_rpc_addresses;
        let peer_discovery_addresses = self.peer_discovery_addresses;
        let tx_recycler = self.tx_recycler;

        while let Some(tx_data) = ffi_tx_receiver.recv().await {
            let client_ref = client.clone();
            let node_ref = node.clone();
            let is_transitioning_ref = is_transitioning.clone();
            let peer_rpc_addresses_ref = peer_rpc_addresses.clone();
            let peer_discovery_addresses_ref = peer_discovery_addresses.clone();
            let tx_recycler_ref = tx_recycler.clone();

            tokio::spawn(async move {
                Self::process_ffi_batch(
                    tx_data,
                    client_ref,
                    node_ref,
                    is_transitioning_ref,
                    peer_rpc_addresses_ref,
                    peer_discovery_addresses_ref,
                    tx_recycler_ref,
                )
                .await;
            });
        }
        Ok(())
    }

    async fn process_ffi_batch(
        tx_data: Vec<u8>,
        client: Arc<dyn TransactionSubmitter>,
        node: Option<Arc<Mutex<ConsensusNode>>>,
        is_transitioning: Option<Arc<AtomicBool>>,
        peer_rpc_addresses: Vec<String>,
        peer_discovery_addresses: Option<Arc<RwLock<Vec<String>>>>,
        tx_recycler: Option<Arc<TxRecycler>>,
    ) {
        use prost::bytes::Buf;
        let mut individual_txs = Vec::new();
        let mut offset = 0;
        let data_len = tx_data.len();
        let mut parse_error = false;

        // Zero-copy extraction
        while offset < data_len {
            let mut buf = &tx_data[offset..];
            let initial_remaining = buf.remaining();

            let tag = match prost::encoding::decode_varint(&mut buf) {
                Ok(t) => t,
                Err(_) => {
                    parse_error = true;
                    break;
                }
            };

            let tag_len = initial_remaining - buf.remaining();
            if tag_len == 0 {
                parse_error = true;
                break;
            }
            offset += tag_len;

            let field_number = tag >> 3;
            let wire_type = tag & 0x07;

            if field_number == 1 && wire_type == 2 {
                let mut buf_val = &tx_data[offset..];
                let init_rem = buf_val.remaining();
                let length = match prost::encoding::decode_varint(&mut buf_val) {
                    Ok(l) => l as usize,
                    Err(_) => {
                        parse_error = true;
                        break;
                    }
                };
                let length_varint_size = init_rem - buf_val.remaining();
                offset += length_varint_size;

                if offset + length <= data_len {
                    individual_txs.push(tx_data[offset..offset + length].to_vec());
                } else {
                    parse_error = true;
                    break;
                }
                offset += length;
            } else {
                match wire_type {
                    0 => {
                        let mut buf_varint = &tx_data[offset..];
                        let init_rem = buf_varint.remaining();
                        let _ = prost::encoding::decode_varint(&mut buf_varint).unwrap_or(0);
                        offset += init_rem - buf_varint.remaining();
                    }
                    1 => offset += 8,
                    2 => {
                        let mut buf_len = &tx_data[offset..];
                        let init_rem = buf_len.remaining();
                        let skip_len = match prost::encoding::decode_varint(&mut buf_len) {
                            Ok(l) => l as usize,
                            Err(_) => {
                                parse_error = true;
                                break;
                            }
                        };
                        offset += (init_rem - buf_len.remaining()) + skip_len;
                    }
                    5 => offset += 4,
                    _ => {
                        parse_error = true;
                        break;
                    }
                }
            }
        }

        if parse_error || individual_txs.is_empty() {
            error!("❌ [FFI TX FLOW] Failed to decode Transactions message");
            return;
        }

        debug!("✅ [FFI TX FLOW] Zero-copy extracted {} TXs", individual_txs.len());
        let transactions_to_submit = individual_txs;

        // RETRY LOOP FOR EPOCH TRANSITIONS
        let mut attempt = 0;
        let mut current_client = client;

        loop {
            // Lock-free transitioning check
            if let Some(ref transitioning) = is_transitioning {
                if transitioning.load(Ordering::SeqCst) {
                    warn!("⚡ [FFI TX FLOW] Epoch transition in progress. Delaying {} TXs internally.", transactions_to_submit.len());
                    attempt += 1;
                    if attempt >= 120 {
                        error!("❌ [FFI TX FLOW] Dropped {} TXs after 120 retries during transition", transactions_to_submit.len());
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    continue;
                }
            }

            // Node acceptance check (takes node lock momentarily)
            if let Some(ref node_arc) = node {
                let lock_result = tokio::time::timeout(std::time::Duration::from_millis(200), node_arc.lock()).await;
                match lock_result {
                    Ok(node_guard) => {
                        let (should_accept, should_queue, reason) = node_guard.check_transaction_acceptance().await;
                        
                        // Update current_client just in case we transitioned recently
                        if let Some(fresh_submitter) = node_guard.transaction_submitter() {
                            current_client = fresh_submitter;
                        }

                        if should_queue {
                            debug!("📨 [FFI TX FLOW] Queueing {} transactions for next epoch: {}", transactions_to_submit.len(), reason);
                            let _ = node_guard.queue_transactions_for_next_epoch(transactions_to_submit.clone()).await;
                            return; // Enqueued successfully
                        }

                        if !should_accept {
                            let is_sync_only = reason.contains("Node is still initializing");
                            if is_sync_only {
                                warn!("⏳ [FFI TX FLOW] Node is catching up. Delaying {} TXs internally.", transactions_to_submit.len());
                                drop(node_guard);
                                attempt += 1;
                                if attempt >= 300 { // Allow 5 minutes during boot
                                    error!("❌ [FFI TX FLOW] Dropped {} TXs after timeout waiting for sync", transactions_to_submit.len());
                                    return;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                                continue;
                            }

                            warn!("🚫 [FFI TX FLOW] Rejecting {} TXs: {}", transactions_to_submit.len(), reason);
                            return; // Permanent failure
                        }
                    }
                    Err(_) => {
                        // Lock timeout. If transitioning, sleep and retry. Else proceed.
                        let is_epoch_transition = is_transitioning
                            .as_ref()
                            .is_some_and(|flag| flag.load(Ordering::SeqCst));

                        if is_epoch_transition {
                            attempt += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                            continue;
                        }
                    }
                }
            }

            // Submission phase
            const MAX_BUNDLE_SIZE: usize = 60000;
            let total_tx_count = transactions_to_submit.len();
            let mut total_submitted = 0usize;

            let chunks_list: Vec<Vec<Vec<u8>>> = if total_tx_count <= MAX_BUNDLE_SIZE {
                vec![transactions_to_submit.clone()]
            } else {
                transactions_to_submit.chunks(MAX_BUNDLE_SIZE).map(|c| c.to_vec()).collect()
            };

            let mut all_succeeded = true;
            for (chunk_idx, chunk_vec) in chunks_list.into_iter().enumerate() {
                let chunk_len = chunk_vec.len();
                
                if let Some(ref recycler) = tx_recycler {
                    recycler.track_submitted(&chunk_vec).await;
                }

                match current_client.submit_no_wait(chunk_vec).await {
                    Ok(included_in_block_rx) => {
                        total_submitted += chunk_len;
                        tokio::spawn(async move {
                            if let Ok((block_ref, _indices, status_receiver)) = included_in_block_rx.await {
                                tokio::spawn(async move {
                                    if let Ok(consensus_core::BlockStatus::GarbageCollected(gc_block)) = status_receiver.await {
                                        warn!("♻️ [FFI TX STATUS] Block {:?} Garbage Collected.", gc_block);
                                    }
                                });
                            }
                        });
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("SyncOnly") || err_str.contains("shutting down") || err_str.contains("channel closed") {
                            warn!("♻️ [FFI TX FLOW] Transition context loss. Delaying internally. Error: {}", err_str);
                            all_succeeded = false;
                            break;
                        } else {
                            error!("❌ [FFI TX FLOW] Submission failed fatally: {}", e);
                            all_succeeded = false;
                        }
                    }
                }
            }

            if all_succeeded {
                return; // Everything submitted cleanly
            }

            // If we broke out early due to transient transition error, sleep and retry
            attempt += 1;
            if attempt >= 120 {
                error!("❌ [FFI TX FLOW] Dropped remaining TXs after Max Retries reached");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        }
    }
}
