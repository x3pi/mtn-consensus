//! IPC server for Go ↔ Rust JMT state communication.
//!
//! Uses Unix Domain Socket (same pattern as tx_socket_server) with a binary
//! protocol for minimal serialization overhead.
//!
//! # Protocol
//!
//! Request:  [cmd:1][payload_len:4][payload...]
//! Response: [status:1][payload_len:4][payload...]
//!
//! # Commands
//!
//! | Cmd | Name          | Request payload               | Response payload        |
//! |-----|---------------|-------------------------------|-------------------------|
//! | 0x01| GET           | [key_len:4][key]              | [value] or empty        |
//! | 0x02| PUT           | [key_len:4][key][value]        | empty                   |
//! | 0x03| BATCH_UPDATE  | [n:4]([key_len:4][key][val_len:4][val])*n | empty       |
//! | 0x04| COMMIT        | empty                         | [root_hash:32]          |
//! | 0x05| ROOT_HASH     | empty                         | [root_hash:32]          |
//! | 0x06| VERSION       | empty                         | [version:8]             |
//! | 0x07| GET_ALL       | empty                         | [n:4]([k_len:4][k][v_len:4][v])*n |
//! | 0x08| RESET         | [version:8][root_hash:32]     | empty                   |
//!
//! Status: 0x00 = OK, 0x01 = ERROR (payload = error message)

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use super::jmt_trie::JmtStateTrie;

// Command bytes
const CMD_GET: u8 = 0x01;
const CMD_PUT: u8 = 0x02;
const CMD_BATCH_UPDATE: u8 = 0x03;
const CMD_COMMIT: u8 = 0x04;
const CMD_ROOT_HASH: u8 = 0x05;
const CMD_VERSION: u8 = 0x06;
const CMD_GET_ALL: u8 = 0x07;
const CMD_RESET: u8 = 0x08;
const CMD_INIT: u8 = 0x09;

// Response status
const STATUS_OK: u8 = 0x00;
const STATUS_ERROR: u8 = 0x01;

/// StateIpcServer serves JMT state operations over UDS.
///
/// This runs as a background tokio task alongside the consensus engine.
/// Go connects to it to read/write state during EVM execution.
///
/// # Transaction Flow
/// ```text
/// Go EVM executes block → collects dirty state entries
///     → sends BATCH_UPDATE over UDS
///     → Rust JMT updates and computes new root
///     → sends COMMIT over UDS
///     → Rust persists to MDBX, returns root hash
///     → Go uses root hash in block header
/// ```
pub struct StateIpcServer {
    socket_path: String,
    env: Arc<crate::state_store::mdbx_store::MdbxEnv>,
}

impl StateIpcServer {
    /// Create a new State IPC server.
    pub fn new(socket_path: String, env: Arc<crate::state_store::mdbx_store::MdbxEnv>) -> Self {
        Self { socket_path, env }
    }

    /// Start the UDS server. Runs until cancelled.
    pub async fn start(self) -> Result<()> {
        // Remove old socket file if exists
        if Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        info!(
            "📦 [JMT-IPC] State IPC server listening on {}",
            self.socket_path
        );

        let env = self.env;

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let env = env.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, env).await {
                            warn!("📦 [JMT-IPC] Connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("📦 [JMT-IPC] Accept error: {}", e);
                }
            }
        }
    }

    /// Handle a single client connection (persistent, multiplexed).
    async fn handle_connection(
        mut stream: UnixStream,
        env: Arc<crate::state_store::mdbx_store::MdbxEnv>,
    ) -> Result<()> {
        info!("📦 [JMT-IPC] New client connected, awaiting INIT...");

        // 1. First command must be CMD_INIT to establish namespace
        let init_cmd = stream.read_u8().await?;
        if init_cmd != CMD_INIT {
            return Err(anyhow::anyhow!("Expected CMD_INIT (0x09), got 0x{:02x}", init_cmd));
        }

        let ns_len = stream.read_u32().await? as usize;
        let mut ns_payload = vec![0u8; ns_len];
        if ns_len > 0 {
            stream.read_exact(&mut ns_payload).await?;
        }

        let namespace = String::from_utf8(ns_payload).unwrap_or_else(|_| "default".to_string());
        
        let store = env.open_store(&namespace)?;
        let trie = Arc::new(JmtStateTrie::new(store));

        let initial_root = trie.root_hash();

        // Acknowledge INIT with root hash
        stream.write_u8(STATUS_OK).await?;
        stream.write_u32(32).await?;
        stream.write_all(&initial_root).await?;

        info!("📦 [JMT-IPC] Client initialized for namespace '{}', root={}", namespace, hex::encode(&initial_root[..8]));

        loop {
            // Read command byte
            let cmd = match stream.read_u8().await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    debug!("📦 [JMT-IPC] Client disconnected ({})", namespace);
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            };

            // Read payload length
            let payload_len = stream.read_u32().await? as usize;

            // Read payload
            let mut payload = vec![0u8; payload_len];
            if payload_len > 0 {
                stream.read_exact(&mut payload).await?;
            }

            // Process command
            let response = match cmd {
                CMD_GET => Self::handle_get(&trie, &payload),
                CMD_PUT => Self::handle_put(&trie, &payload),
                CMD_BATCH_UPDATE => Self::handle_batch_update(&trie, &payload),
                CMD_COMMIT => Self::handle_commit(&trie, &payload),
                CMD_ROOT_HASH => Self::handle_root_hash(&trie),
                CMD_VERSION => Self::handle_version(&trie),
                CMD_GET_ALL => Self::handle_get_all(&trie),
                CMD_RESET => Self::handle_reset(&trie, &payload),
                _ => Err(anyhow::anyhow!("Unknown command: 0x{:02x}", cmd)),
            };

            // Send response
            match response {
                Ok(data) => {
                    stream.write_u8(STATUS_OK).await?;
                    stream.write_u32(data.len() as u32).await?;
                    if !data.is_empty() {
                        stream.write_all(&data).await?;
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string().into_bytes();
                    stream.write_u8(STATUS_ERROR).await?;
                    stream.write_u32(err_msg.len() as u32).await?;
                    stream.write_all(&err_msg).await?;
                }
            }
            stream.flush().await?;
        }
    }

    // ─── Command handlers ───────────────────────────────────────────────

    fn handle_get(trie: &JmtStateTrie, payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() < 4 {
            return Err(anyhow::anyhow!("GET: payload too short"));
        }
        let key_len = u32::from_be_bytes(payload[0..4].try_into().unwrap()) as usize;
        if payload.len() < 4 + key_len {
            return Err(anyhow::anyhow!("GET: payload truncated"));
        }
        let key = &payload[4..4 + key_len];

        match trie.get(key)? {
            Some(value) => Ok(value),
            None => Ok(Vec::new()), // Empty = not found
        }
    }

    fn handle_put(trie: &JmtStateTrie, payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() < 4 {
            return Err(anyhow::anyhow!("PUT: payload too short"));
        }
        let key_len = u32::from_be_bytes(payload[0..4].try_into().unwrap()) as usize;
        if payload.len() < 4 + key_len {
            return Err(anyhow::anyhow!("PUT: payload truncated"));
        }
        let key = &payload[4..4 + key_len];
        let value = &payload[4 + key_len..];

        // Single put goes through batch_update_and_commit for consistency
        trie.batch_update_and_commit(
            &[key.to_vec()],
            &[Some(value.to_vec())],
        )?;
        Ok(Vec::new())
    }

    fn handle_batch_update(trie: &JmtStateTrie, payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() < 4 {
            return Err(anyhow::anyhow!("BATCH_UPDATE: payload too short"));
        }
        let n = u32::from_be_bytes(payload[0..4].try_into().unwrap()) as usize;

        let mut keys = Vec::with_capacity(n);
        let mut values = Vec::with_capacity(n);
        let mut offset = 4;

        for _ in 0..n {
            // Read key
            if offset + 4 > payload.len() {
                return Err(anyhow::anyhow!("BATCH_UPDATE: truncated at key_len"));
            }
            let key_len =
                u32::from_be_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if offset + key_len > payload.len() {
                return Err(anyhow::anyhow!("BATCH_UPDATE: truncated at key"));
            }
            let key = payload[offset..offset + key_len].to_vec();
            offset += key_len;

            // Read value
            if offset + 4 > payload.len() {
                return Err(anyhow::anyhow!("BATCH_UPDATE: truncated at val_len"));
            }
            let val_len =
                u32::from_be_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if offset + val_len > payload.len() {
                return Err(anyhow::anyhow!("BATCH_UPDATE: truncated at value"));
            }
            let value = if val_len == 0 {
                None // Deletion
            } else {
                Some(payload[offset..offset + val_len].to_vec())
            };
            offset += val_len;

            keys.push(key);
            values.push(value);
        }

        // Apply batch and commit to JMT
        let root = trie.batch_update_and_commit(&keys, &values)?;
        Ok(root.to_vec()) // Return new root hash
    }

    fn handle_commit(_trie: &JmtStateTrie, _payload: &[u8]) -> Result<Vec<u8>> {
        // batch_update_and_commit already commits.
        // This is a no-op for now, but could be used for explicit flush.
        Ok(_trie.root_hash().to_vec())
    }

    fn handle_root_hash(trie: &JmtStateTrie) -> Result<Vec<u8>> {
        Ok(trie.root_hash().to_vec())
    }

    fn handle_version(trie: &JmtStateTrie) -> Result<Vec<u8>> {
        let version = trie.version();
        Ok(version.to_be_bytes().to_vec())
    }

    fn handle_get_all(trie: &JmtStateTrie) -> Result<Vec<u8>> {
        // GetAll scans all jv: prefixed keys from MDBX
        // This is used for state replication to sub-nodes
        let all = trie.get_all()?;
        let mut buf = Vec::with_capacity(4 + all.len() * 64);
        buf.extend_from_slice(&(all.len() as u32).to_be_bytes());
        for (key, value) in &all {
            buf.extend_from_slice(&(key.len() as u32).to_be_bytes());
            buf.extend_from_slice(key);
            buf.extend_from_slice(&(value.len() as u32).to_be_bytes());
            buf.extend_from_slice(value);
        }
        Ok(buf)
    }

    fn handle_reset(_trie: &JmtStateTrie, payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() < 40 {
            return Err(anyhow::anyhow!("RESET: need 8 bytes version + 32 bytes root_hash"));
        }
        let _version = u64::from_be_bytes(payload[0..8].try_into().unwrap());
        let mut root = [0u8; 32];
        root.copy_from_slice(&payload[8..40]);
        // For now, reset is a placeholder — would need to reinitialize trie
        info!("📦 [JMT-IPC] Reset requested (version={}, root={})", _version, hex::encode(&root[..8]));
        Ok(Vec::new())
    }
}
