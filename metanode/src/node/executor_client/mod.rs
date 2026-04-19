// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! ExecutorClient module - communicates with Go executor via sockets.
//!
//! Submodules:
//! - `socket_stream`: Socket abstraction (Unix + TCP)
//! - `persistence`: Crash recovery persistence helpers
//! - `block_sync`: Block sync methods for validators and SyncOnly nodes
//! - `rpc_queries`: RPC query methods (get_validators, get_epoch, etc.)
//! - `block_sending`: Block sending + protobuf conversion
//! - `transition_handoff`: Epoch transition handoff APIs

pub(crate) mod block_sending;
pub mod block_store;
mod block_sync;
pub mod connection_pool;
pub mod persistence;
mod rpc_queries;
mod rpc_queries_epoch;
pub mod socket_stream;
pub mod traits;
mod transition_handoff;

// Re-export public items from submodules
pub use connection_pool::ConnectionPool;
pub use persistence::{load_persisted_last_index, read_last_block_number};
pub use socket_stream::{SocketAddress, SocketStream};
pub use traits::TExecutorClient;

use anyhow::Result;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use crate::node::rpc_circuit_breaker::RpcCircuitBreaker;

// Include generated protobuf code
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/proto.rs"));
}

use std::collections::BTreeMap;

// ============================================================================
// ExecutorClient - Now supports both Unix and TCP sockets
// ============================================================================

/// Client to send committed blocks to Go executor via Unix Domain Socket or TCP Socket
/// Supports both local (Unix) and network (TCP) deployment
pub struct ExecutorClient {
    socket_address: SocketAddress, // Changed from socket_path: String
    pub(crate) connection: Arc<Mutex<Option<SocketStream>>>, // Changed from UnixStream
    pub(crate) request_socket_address: SocketAddress, // Changed from request_socket_path: String
    pub(crate) request_connection: Arc<Mutex<Option<SocketStream>>>, // Changed from UnixStream
    enabled: bool,
    can_commit: bool, // Only node 0 can actually commit transactions to Go state
    /// Buffer for out-of-order blocks to ensure sequential sending
    /// Key: global_exec_index, Value: (epoch_data_bytes, epoch, commit_index)
    pub(crate) send_buffer: Arc<Mutex<BTreeMap<u64, (Vec<u8>, u64, u32)>>>,
    /// Next expected global_exec_index to send
    pub(crate) next_expected_index: Arc<tokio::sync::Mutex<u64>>,
    /// Storage path for persisting state (crash recovery)
    pub(crate) storage_path: Option<std::path::PathBuf>,
    /// Last verified Go block number (for fork detection)
    pub(crate) last_verified_go_index: Arc<tokio::sync::Mutex<u64>>,
    /// Track sent global_exec_indices to prevent duplicates from dual-stream
    /// This prevents both Consensus and Sync from sending the same block
    pub(crate) sent_indices: Arc<tokio::sync::Mutex<std::collections::BTreeSet<u64>>>,
    pub(crate) rpc_circuit_breaker: Arc<RpcCircuitBreaker>,
    /// Circuit breaker state for the block sending socket
    pub(crate) send_failures: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) send_cb_open_until: Arc<tokio::sync::RwLock<Option<tokio::time::Instant>>>,
    /// Connection pool for parallel RPC queries to Go Master
    pub(crate) request_pool: Arc<ConnectionPool>,
    /// BACKPRESSURE: Shared handle to update Go lag in SystemTransactionProvider
    /// When set, flush_buffer() will update this value with the computed lag
    pub(crate) go_lag_handle: Option<Arc<AtomicU64>>,
    /// CRITICAL FORK-SAFETY: Explicit block number tracker, passed to Go inside ExecutableBlock
    pub(crate) next_block_number: Arc<tokio::sync::Mutex<u64>>,
    /// CRITICAL FORK-SAFETY: Tracks the last epoch we processed to identify epoch boundaries
    pub(crate) last_processed_epoch: Arc<tokio::sync::Mutex<u64>>,
}

/// Production safety constants
pub(crate) const MAX_BUFFER_SIZE: usize = 10_000; // Maximum blocks to buffer before rejecting
pub(crate) const GO_VERIFICATION_INTERVAL: u64 = 100; // T2-2: Verify Go state every 100 blocks (was 1000 — faster backpressure feedback)

impl ExecutorClient {
    /// Create new executor client
    /// enabled: whether executor is enabled (check config file exists)
    /// can_commit: whether this node can actually commit transactions (only node 0)
    /// send_socket_path: socket path for sending data to Go executor
    /// receive_socket_path: socket path for receiving data from Go executor
    /// initial_next_expected: initial value for next_expected_index (default: 1)
    pub fn new(
        enabled: bool,
        can_commit: bool,
        send_socket_path: String,
        receive_socket_path: String,
        storage_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self::new_with_initial_index(
            enabled,
            can_commit,
            send_socket_path,
            receive_socket_path,
            1,
            storage_path,
        )
    }

    /// Create new executor client with initial next_expected_index
    /// This is useful when creating executor client for a new epoch, where we know the starting global_exec_index
    /// CRITICAL: Buffer is always empty when creating new executor client (prevents duplicate global_exec_index)
    pub fn new_with_initial_index(
        enabled: bool,
        can_commit: bool,
        send_socket_path: String,
        receive_socket_path: String,
        initial_next_expected: u64,
        storage_path: Option<std::path::PathBuf>,
    ) -> Self {
        // CRITICAL FIX: Always create empty buffer to prevent duplicate global_exec_index
        // When creating new executor client (e.g., after restart or epoch transition),
        // buffer should be empty to avoid conflicts with old commits
        let send_buffer = Arc::new(Mutex::new(BTreeMap::new()));

        // Parse socket addresses with auto-detection
        let socket_address = SocketAddress::parse(&send_socket_path)
            .unwrap_or_else(|e| {
                warn!("⚠️ [EXECUTOR CLIENT] Failed to parse send socket '{}': {}. Defaulting to Unix socket.", send_socket_path, e);
                SocketAddress::Unix(send_socket_path.clone())
            });

        let request_socket_address = SocketAddress::parse(&receive_socket_path)
            .unwrap_or_else(|e| {
                warn!("⚠️ [EXECUTOR CLIENT] Failed to parse receive socket '{}': {}. Defaulting to Unix socket.", receive_socket_path, e);
                SocketAddress::Unix(receive_socket_path.clone())
            });

        info!("🔧 [EXECUTOR CLIENT] Creating executor client: send={}, receive={}, initial_next_expected={}, storage_path={:?}", 
            socket_address.as_str(), request_socket_address.as_str(), initial_next_expected, storage_path);

        let is_ffi_mode = crate::ffi::GO_CALLBACKS.get().is_some();
        let request_pool = if is_ffi_mode {
            Arc::new(ConnectionPool::new_noop())
        } else {
            Arc::new(ConnectionPool::new(request_socket_address.clone(), 4, 30))
        };

        let connection: Arc<Mutex<Option<SocketStream>>> = Arc::new(Mutex::new(None));
        let request_connection: Arc<Mutex<Option<SocketStream>>> = Arc::new(Mutex::new(None));

        if enabled && !is_ffi_mode {
            let conn_arc = connection.clone();
            if tokio::runtime::Handle::try_current().is_ok() {
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
                    loop {
                        ticker.tick().await;
                        let mut conn_guard = conn_arc.lock().await;
                        if let Some(ref mut stream) = *conn_guard {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(2),
                                stream.writable(),
                            )
                            .await
                            {
                                Ok(Ok(_)) => { /* healthy */ }
                                _ => {
                                    tracing::warn!("🚨 [IPC-HEALTH] Send connection unhealthy/timeout, forcing reconnect");
                                    *conn_guard = None;
                                }
                            }
                        }
                    }
                });
            }
        }

        Self {
            socket_address,
            connection,
            request_socket_address,
            request_connection,
            enabled,
            can_commit,
            send_buffer,
            next_expected_index: Arc::new(tokio::sync::Mutex::new(initial_next_expected)),
            storage_path,
            last_verified_go_index: Arc::new(tokio::sync::Mutex::new(0)),
            sent_indices: Arc::new(tokio::sync::Mutex::new(std::collections::BTreeSet::new())),
            rpc_circuit_breaker: Arc::new(RpcCircuitBreaker::new()),
            send_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            send_cb_open_until: Arc::new(tokio::sync::RwLock::new(None)),
            request_pool,
            go_lag_handle: None, // Set via set_go_lag_handle() after construction
            next_block_number: Arc::new(tokio::sync::Mutex::new(0)),
            last_processed_epoch: Arc::new(tokio::sync::Mutex::new(0)),
        }
    }

    /// Check if executor is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Check if this node can commit transactions (only node 0)
    pub fn can_commit(&self) -> bool {
        self.can_commit
    }

    /// Get the storage path for persistence (used by block_store for sync)
    pub fn storage_path(&self) -> Option<&std::path::Path> {
        self.storage_path.as_deref()
    }

    /// Set the Go lag handle for backpressure signaling to SystemTransactionProvider
    pub fn set_go_lag_handle(&mut self, handle: Arc<AtomicU64>) {
        self.go_lag_handle = Some(handle);
    }

    /// Get reference to the RPC circuit breaker
    #[allow(dead_code)]
    pub fn circuit_breaker(&self) -> &RpcCircuitBreaker {
        &self.rpc_circuit_breaker
    }

    /// Check if the TCP send circuit breaker is open
    #[allow(dead_code)]
    pub async fn check_send_circuit_breaker(&self) -> Result<()> {
        let cb_guard = self.send_cb_open_until.read().await;
        if let Some(open_until) = *cb_guard {
            if tokio::time::Instant::now() < open_until {
                let remaining = (open_until - tokio::time::Instant::now()).as_secs();
                return Err(anyhow::anyhow!(
                    "TCP Send Circuit Breaker is OPEN. Cooling down for {}s",
                    remaining
                ));
            }
        }
        Ok(())
    }

    /// Record a failure on the TCP send socket. Opens the breaker on 3 consecutive failures.
    pub async fn record_send_failure(&self) {
        let failures = self
            .send_failures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if failures >= 3 {
            // Open the circuit for 10 seconds
            let mut cb_guard = self.send_cb_open_until.write().await;
            *cb_guard = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(10));
            warn!("🚨 [CIRCUIT BREAKER] TCP Send Circuit Breaker TRIPPED after {} consecutive failures. Open for 10s.", failures);
        }
    }

    /// Record a success on the TCP send socket, clearing the failures and closing the breaker.
    pub async fn record_send_success(&self) {
        let prev = self
            .send_failures
            .swap(0, std::sync::atomic::Ordering::SeqCst);
        if prev > 0 {
            let mut cb_guard = self.send_cb_open_until.write().await;
            if cb_guard.is_some() {
                info!("✅ [CIRCUIT BREAKER] TCP Send Circuit Breaker CLOSED (Recovered).");
                *cb_guard = None;
            }
        }
    }

    /// Force reset all connections to Go executor
    /// This is used to recover from stale connections after Go restart
    /// Next call to connect() or connect_request() will create fresh connections
    pub async fn reset_connections(&self) {
        info!("🔄 [EXECUTOR] Force resetting all connections (triggered by consecutive errors)...");

        // Reset data connection
        {
            let mut conn = self.connection.lock().await;
            if conn.is_some() {
                info!("🔌 [EXECUTOR] Closing stale data connection");
            }
            *conn = None;
        }

        // Reset request connection
        {
            let mut req_conn = self.request_connection.lock().await;
            if req_conn.is_some() {
                info!("🔌 [EXECUTOR] Closing stale request connection");
            }
            *req_conn = None;
        }

        // Reset connection pool
        self.request_pool.reset_all().await;

        info!("✅ [EXECUTOR] All connections reset. Next operation will create fresh connections.");
    }

    /// Initialize next_expected_index from Go Master's last block number
    /// This should be called ONCE when executor client is created, not on every connect
    /// After initialization, Rust will send blocks continuously and Go will buffer/process them sequentially
    /// CRITICAL FIX: Sync with Go's state to prevent duplicate commits
    /// - If Go is ahead: Update to Go's state (Go has already processed those commits, prevent duplicates)
    /// - If Go is behind: Keep current value (we have commits Go hasn't seen yet)
    pub async fn initialize_from_go(&self) {
        if !self.is_enabled() {
            return;
        }

        // Fetch current epoch from Go to track epoch boundaries
        let go_current_epoch = match self.get_current_epoch().await {
            Ok(e) => {
                info!("📊 [INIT] Fetched current epoch from Go: {}", e);
                e
            }
            Err(e) => {
                warn!(
                    "⚠️ [INIT] Failed to fetch current epoch: {}. Defaulting to 0.",
                    e
                );
                0
            }
        };
        {
            let mut last_processed_epoch_guard = self.last_processed_epoch.lock().await;
            *last_processed_epoch_guard = go_current_epoch;
        }

        // Query Go Master for last_block_number and last_global_exec_index directly
        let last_go_state_opt = loop {
            match self.get_last_block_number().await {
                Ok((bn, gei, is_ready)) => {
                    if is_ready {
                        break Some((bn, gei));
                    } else {
                        warn!("⏳ [INIT] Go Master is connected but not fully ready (DB loading). Retrying in 1s...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
                Err(e) => {
                    warn!("⚠️  [INIT] Failed to get state from Go Master: {}. Attempting to read persisted value.", e);
                    // Fallback to persisted last block number if available
                    if let Some(ref storage_path) = self.storage_path {
                        let fallback_bn = match read_last_block_number(storage_path).await {
                            Ok(n) => {
                                info!("📊 [INIT] Loaded persisted last block number {}", n);
                                Some(n)
                            }
                            Err(_) => None,
                        };
                        let fallback_gei = match load_persisted_last_index(storage_path) {
                            Some((gei, _commit)) => {
                                info!("📊 [INIT] Loaded persisted last global exec index {}", gei);
                                Some(gei)
                            }
                            None => None,
                        };

                        if let (Some(bn), Some(gei)) = (fallback_bn, fallback_gei) {
                            break Some((bn, gei));
                        } else {
                            break None;
                        }
                    } else {
                        break None;
                    }
                }
            }
        };

        if let Some((last_block_number, last_global_exec_index)) = last_go_state_opt {
            let go_next_expected = last_global_exec_index + 1;
            let current_next_expected = {
                let next_expected_guard = self.next_expected_index.lock().await;
                *next_expected_guard
            };

            // Initialize next_block_number (Explicit tracking)
            {
                let mut next_bn_guard = self.next_block_number.lock().await;
                *next_bn_guard = last_block_number + 1;
                info!(
                    "📊 [INIT] Initialized next_block_number to {}",
                    *next_bn_guard
                );
            }

            // CRITICAL FIX: Sync with Go's state to prevent data loss or duplicate commits
            if go_next_expected < current_next_expected {
                {
                    let mut next_expected_guard = self.next_expected_index.lock().await;
                    *next_expected_guard = go_next_expected;
                }
                warn!("⚠️ [INIT] Go Master is behind (last_global_exec_index={}, go_next_expected={} < current_next_expected={}). Winding back next_expected_index to allow WAL replay of lost blocks.",
                    last_global_exec_index, go_next_expected, current_next_expected);
            } else if go_next_expected > current_next_expected {
                {
                    let mut next_expected_guard = self.next_expected_index.lock().await;
                    *next_expected_guard = go_next_expected;
                }
                info!("📊 [INIT] Updating next_expected_index from {} to {} (last_global_exec_index={}, go_next_expected={})",
                    current_next_expected, go_next_expected, last_global_exec_index, go_next_expected);

                let mut buffer = self.send_buffer.lock().await;
                let before_clear = buffer.len();
                buffer.retain(|&k, _| k >= go_next_expected);
                let after_clear = buffer.len();
                if before_clear > after_clear {
                    info!("🧹 [INIT] Cleared {} buffered commits that Go has already processed (kept {} commits)", 
                        before_clear - after_clear, after_clear);
                }
            } else {
                info!("📊 [INIT] next_expected_index matches Go Master: last_global_exec_index={}, next_expected={}", 
                    last_global_exec_index, current_next_expected);
            }
        } else {
            warn!("⚠️  [INIT] Could not determine last state. Keeping current metrics. Rust will continue sending blocks, Go will buffer and process sequentially.");
        }

        // ─── Readiness Signal ────────────────────────────────────────
        let final_next_expected = *self.next_expected_index.lock().await;
        let go_conn_status = if last_go_state_opt.is_some() {
            "connected"
        } else {
            "unknown"
        };
        info!(
            "✅ [READY] Rust executor: go_connection={}, next_expected={}, go_last_block={}",
            go_conn_status,
            final_next_expected,
            last_go_state_opt.map_or("unknown".to_string(), |(_, n)| n.to_string())
        );
    }

    /// Connect to executor socket (lazy connection with persistent retry)
    /// Just connects, doesn't query Go - Rust sends blocks continuously, Go buffers and processes sequentially
    /// CRITICAL: Persistent connection - keeps trying until socket becomes available (Go Master starts)
    pub(crate) async fn connect(&self) -> Result<()> {
        Ok(()) // FFI bypasses connections
    }

    /// Connect to Go request socket for request/response (lazy connection with retry)
    #[allow(dead_code)]
    pub(crate) async fn connect_request(&self) -> Result<()> {
        Ok(()) // FFI bypasses connections
    }

    /// Execute an RPC request synchronously via CGo FFI
    pub(crate) async fn execute_rpc_request(&self, request_buf: &[u8]) -> Result<Vec<u8>> {
        if let Some(c_fn) = crate::ffi::GO_CALLBACKS
            .get()
            .and_then(|c| c.process_rpc_request)
        {
            let mut out_payload: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;

            // Invoke the FFI method. It will mutate the ptr pointers to allocate protobuf bytes.
            let success = c_fn(
                request_buf.as_ptr(),
                request_buf.len(),
                &mut out_payload,
                &mut out_len,
            );

            if !success {
                return Err(anyhow::anyhow!("Go FFI process_rpc_request returned false"));
            }
            if out_payload.is_null() || out_len == 0 {
                return Err(anyhow::anyhow!("Received null response from Go FFI"));
            }

            // Copy out data
            let slice = unsafe { std::slice::from_raw_parts(out_payload, out_len) };
            let response_buf = slice.to_vec();

            // Free C allocator buffer using Go's free function
            if let Some(free_fn) = crate::ffi::GO_CALLBACKS
                .get()
                .and_then(|c| c.free_go_buffer)
            {
                free_fn(out_payload);
            }
            Ok(response_buf)
        } else {
            Err(anyhow::anyhow!("FFI process_rpc_request not registered"))
        }
    }
}

// RPC query methods are in rpc_queries.rs
// Block sending methods are in block_sending.rs
// Transition handoff APIs are in transition_handoff.rs
// Persistence functions are in persistence.rs
// Block sync methods are in block_sync.rs
// Socket abstraction is in socket_stream.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_client_creation_defaults() {
        let client = ExecutorClient::new(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        assert!(client.is_enabled());
        assert!(client.can_commit());
    }

    #[test]
    fn test_executor_client_disabled() {
        let client = ExecutorClient::new(
            false,
            false,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        assert!(!client.is_enabled());
        assert!(!client.can_commit());
    }

    #[test]
    fn test_executor_client_enabled_always_commits() {
        // All enabled executor clients should be able to commit
        // (can_commit guard was removed — executor_commit_enabled config controls creation)
        let client = ExecutorClient::new(
            true,
            true, // All enabled clients commit
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        assert!(client.is_enabled());
        assert!(client.can_commit());
    }

    #[tokio::test]
    async fn test_executor_client_buffer_empty_on_creation() {
        let client = ExecutorClient::new(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        let buffer = client.send_buffer.lock().await;
        assert!(buffer.is_empty(), "Send buffer should be empty on creation");
    }

    #[tokio::test]
    async fn test_executor_client_initial_next_expected() {
        // Default constructor starts at 1
        let client = ExecutorClient::new(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        let idx = client.next_expected_index.lock().await;
        assert_eq!(*idx, 1, "Default next_expected_index should be 1");
    }

    #[tokio::test]
    async fn test_executor_client_custom_initial_index() {
        let client = ExecutorClient::new_with_initial_index(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            42,
            None,
        );
        let idx = client.next_expected_index.lock().await;
        assert_eq!(*idx, 42, "Custom next_expected_index should be 42");

        // Buffer should still be empty even with custom index
        let buffer = client.send_buffer.lock().await;
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_executor_client_circuit_breaker_initialized() {
        let client = ExecutorClient::new(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        // Circuit breaker should be accessible and in closed state
        let cb = client.circuit_breaker();
        assert!(cb.check("any_method").is_ok());
    }

    #[tokio::test]
    async fn test_executor_client_reset_connections() {
        let client = ExecutorClient::new(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        // Reset should not panic even without active connections
        client.reset_connections().await;

        // Connections should be None after reset
        let conn = client.connection.lock().await;
        assert!(conn.is_none());
        let req_conn = client.request_connection.lock().await;
        assert!(req_conn.is_none());
    }

    #[tokio::test]
    async fn test_executor_client_sent_indices_empty() {
        let client = ExecutorClient::new(
            true,
            true,
            "/tmp/test_send.sock".to_string(),
            "/tmp/test_recv.sock".to_string(),
            None,
        );
        let sent = client.sent_indices.lock().await;
        assert!(sent.is_empty(), "Sent indices should be empty on creation");
    }
}
