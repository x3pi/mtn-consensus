//! JMT (Jellyfish Merkle Tree) wrapper backed by MDBX storage.
//!
//! This module bridges the `jmt` crate with our `MdbxStore` to provide:
//! - Versioned Merkle state commitment (SHA-256)
//! - Batch updates (O(K×log N) for K dirty entries in a tree of N)
//! - Merkle proofs for light client verification
//!
//! # Transaction Flow Integration
//!
//! ```text
//! Go (EVM execution) → state changes (key→value pairs)
//!       ↓ IPC
//! Rust JmtStateTrie::batch_update_and_commit(keys, values)
//!       ↓
//! JMT put_value_set → compute new root hash
//!       ↓
//! TreeUpdateBatch → persist new/stale nodes to MDBX
//!       ↓
//! Return root_hash to Go → block header
//! ```
//!
//! The existing consensus pipeline (DAG → commit → Go execute) is unchanged.
//! JMT only replaces the **state commitment** step (hash computation + DB write).

use std::sync::RwLock;

use anyhow::{Context, Result};
use jmt::storage::{LeafNode, Node, NodeBatch, NodeKey, TreeReader, TreeWriter};
use jmt::{KeyHash, OwnedValue, RootHash, Sha256Jmt, Version};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use super::mdbx_store::MdbxStore;
use super::traits::StateStore;

// ═══════════════════════════════════════════════════════════════════════════════
// MDBX-backed TreeReader/TreeWriter adapter for JMT
// ═══════════════════════════════════════════════════════════════════════════════

/// Prefix for JMT internal nodes in MDBX.
const JMT_NODE_PREFIX: &[u8] = b"jn:";
/// Prefix for flat key-value data (for direct reads without tree traversal).
const JMT_VALUE_PREFIX: &[u8] = b"jv:";
/// Prefix for versioned values (JMT value history).
const JMT_VERSIONED_VALUE_PREFIX: &[u8] = b"vv:";

/// MdbxTreeStore adapts MdbxStore to implement JMT's TreeReader/TreeWriter traits.
/// This is the bridge between JMT's in-memory tree operations and persistent MDBX storage.
#[derive(Clone)]
pub struct MdbxTreeStore {
    pub(crate) store: MdbxStore,
}

impl MdbxTreeStore {
    pub fn new(store: MdbxStore) -> Self {
        Self { store }
    }

    /// Encode a NodeKey to bytes for MDBX storage.
    fn encode_node_key(node_key: &NodeKey) -> Vec<u8> {
        let mut buf = Vec::with_capacity(JMT_NODE_PREFIX.len() + 64);
        buf.extend_from_slice(JMT_NODE_PREFIX);
        let serialized =
            borsh::to_vec(node_key).expect("NodeKey borsh serialization should not fail");
        buf.extend_from_slice(&serialized);
        buf
    }

    /// Encode a value key for flat storage (direct reads).
    pub(crate) fn encode_value_key(key_hash: &KeyHash) -> Vec<u8> {
        let mut buf = Vec::with_capacity(JMT_VALUE_PREFIX.len() + 32);
        buf.extend_from_slice(JMT_VALUE_PREFIX);
        buf.extend_from_slice(key_hash.0.as_ref());
        buf
    }

    /// Encode a versioned value key for JMT value history.
    fn encode_versioned_value_key(version: Version, key_hash: &KeyHash) -> Vec<u8> {
        let mut buf = Vec::with_capacity(JMT_VERSIONED_VALUE_PREFIX.len() + 8 + 32);
        buf.extend_from_slice(JMT_VERSIONED_VALUE_PREFIX);
        buf.extend_from_slice(&version.to_be_bytes());
        buf.extend_from_slice(key_hash.0.as_ref());
        buf
    }
}

impl TreeReader for MdbxTreeStore {
    fn get_node_option(&self, node_key: &NodeKey) -> Result<Option<Node>> {
        let db_key = Self::encode_node_key(node_key);
        match self.store.get(&db_key)? {
            Some(data) => {
                let node: Node = borsh::from_slice(&data)
                    .with_context(|| format!("Failed to deserialize JMT node at {:?}", node_key))?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    fn get_value_option(
        &self,
        max_version: Version,
        key_hash: KeyHash,
    ) -> Result<Option<OwnedValue>> {
        // Search from max_version downward for the latest value.
        // For now, do a simple exact version lookup. In production,
        // you'd scan from max_version backward.
        let db_key = Self::encode_versioned_value_key(max_version, &key_hash);
        match self.store.get(&db_key)? {
            Some(data) => Ok(Some(data)),
            None => {
                // Fallback: check flat value store (latest value)
                let flat_key = Self::encode_value_key(&key_hash);
                match self.store.get(&flat_key)? {
                    Some(data) if !data.is_empty() => Ok(Some(data)),
                    _ => Ok(None),
                }
            }
        }
    }

    fn get_rightmost_leaf(&self) -> Result<Option<(NodeKey, LeafNode)>> {
        // Not needed for our use case (only used for JMT restoration).
        Ok(None)
    }
}

impl TreeWriter for MdbxTreeStore {
    fn write_node_batch(&self, node_batch: &NodeBatch) -> Result<()> {
        let nodes = node_batch.nodes();
        let values = node_batch.values();

        if nodes.is_empty() && values.is_empty() {
            return Ok(());
        }

        // Collect node entries
        let mut all_pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(nodes.len() + values.len());

        for (node_key, node) in nodes.iter() {
            let db_key = Self::encode_node_key(node_key);
            let db_value =
                borsh::to_vec(node).expect("Node borsh serialization should not fail");
            all_pairs.push((db_key, db_value));
        }

        // Collect versioned value entries
        for ((version, key_hash), value) in values.iter() {
            let db_key = Self::encode_versioned_value_key(*version, key_hash);
            let db_value = match value {
                Some(v) => v.clone(),
                None => Vec::new(), // Deletion marker
            };
            all_pairs.push((db_key, db_value));
        }

        let pair_refs: Vec<(&[u8], &[u8])> = all_pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();

        self.store.batch_put(&pair_refs)?;
        debug!(
            "📦 [JMT] Wrote {} nodes + {} values to MDBX",
            nodes.len(),
            values.len()
        );
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// JmtStateTrie — main interface for state commitment
// ═══════════════════════════════════════════════════════════════════════════════

/// JmtStateTrie provides versioned Merkle state commitment using JMT + MDBX.
///
/// # Thread Safety
/// - Reads are lock-free (MDBX mmap)
/// - Writes are serialized through RwLock on version counter
///
/// # Fork Safety
/// All nodes must compute the SAME root hash for the SAME state.
/// JMT is deterministic: same key-value set → same root hash, regardless of insertion order.
pub struct JmtStateTrie {
    tree_store: MdbxTreeStore,
    /// Current version (increments with each commit, corresponds to block number)
    version: RwLock<Version>,
    /// Current root hash
    root_hash: RwLock<RootHash>,
}

impl JmtStateTrie {
    /// Create a new JmtStateTrie with empty state.
    pub fn new(store: MdbxStore) -> Self {
        let tree_store = MdbxTreeStore::new(store);
        Self {
            tree_store,
            version: RwLock::new(0),
            root_hash: RwLock::new(RootHash([0u8; 32])),
        }
    }

    /// Create a JmtStateTrie from existing state at a given version.
    pub fn from_version(store: MdbxStore, version: Version, root_hash: [u8; 32]) -> Self {
        let tree_store = MdbxTreeStore::new(store);
        Self {
            tree_store,
            version: RwLock::new(version),
            root_hash: RwLock::new(RootHash(root_hash)),
        }
    }

    /// Hash a raw key to a KeyHash (SHA-256).
    /// This is deterministic: same key → same hash on all nodes.
    pub fn hash_key(key: &[u8]) -> KeyHash {
        KeyHash(Sha256::digest(key).into())
    }

    /// Get the current root hash.
    pub fn root_hash(&self) -> [u8; 32] {
        self.root_hash.read().unwrap().0
    }

    /// Get the current version.
    pub fn version(&self) -> Version {
        *self.version.read().unwrap()
    }

    /// Get a value by key from the flat value store (O(1) read, no tree traversal).
    /// This is the fast path for EVM state reads.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let key_hash = Self::hash_key(key);
        let db_key = MdbxTreeStore::encode_value_key(&key_hash);
        self.tree_store.store.get(&db_key)
    }

    /// Get a value with Merkle proof (for light client verification).
    /// This traverses the JMT tree: O(log N).
    pub fn get_with_proof(
        &self,
        key: &[u8],
    ) -> Result<(Option<OwnedValue>, jmt::proof::SparseMerkleProof<Sha256>)> {
        let version = *self.version.read().unwrap();
        let key_hash = Self::hash_key(key);
        let jmt = Sha256Jmt::new(&self.tree_store);
        let (value, proof) = jmt.get_with_proof(key_hash, version)?;
        Ok((value, proof))
    }

    /// Apply a batch of state changes and compute new root hash.
    ///
    /// This is the core method called after Go executes a block of transactions:
    /// 1. Receives dirty key-value pairs from Go
    /// 2. Computes new JMT root hash (SHA-256 Merkle)
    /// 3. Persists new/updated nodes to MDBX
    /// 4. Persists flat key-value pairs for fast reads
    /// 5. Returns new root hash for block header
    ///
    /// # Fork Safety
    /// Deterministic: same (keys, values) → same root hash on all nodes.
    /// Version is incremented atomically to prevent double-commits.
    pub fn batch_update_and_commit(
        &self,
        keys: &[Vec<u8>],
        values: &[Option<Vec<u8>>],
    ) -> Result<[u8; 32]> {
        if keys.len() != values.len() {
            return Err(anyhow::anyhow!(
                "JMT batch_update: keys/values length mismatch ({} vs {})",
                keys.len(),
                values.len()
            ));
        }

        if keys.is_empty() {
            return Ok(self.root_hash());
        }

        // 1. Prepare the value set for JMT
        let value_set: Vec<(KeyHash, Option<OwnedValue>)> = keys
            .iter()
            .zip(values.iter())
            .map(|(key, value)| {
                let key_hash = Self::hash_key(key);
                (key_hash, value.clone())
            })
            .collect();

        // 2. Get current version and compute next
        let mut version_guard = self.version.write().unwrap();
        let new_version = *version_guard + 1;

        // 3. Apply to JMT → get new root hash + update batch
        let jmt = Sha256Jmt::new(&self.tree_store);
        let (new_root, update_batch) = jmt.put_value_set(value_set, new_version)?;

        // 4. Persist JMT nodes + versioned values to MDBX
        self.tree_store
            .write_node_batch(&update_batch.node_batch)?;

        // 5. Persist flat key-value pairs for O(1) reads
        let flat_pairs: Vec<(Vec<u8>, Vec<u8>)> = keys
            .iter()
            .zip(values.iter())
            .map(|(key, value)| {
                let key_hash = Self::hash_key(key);
                let db_key = MdbxTreeStore::encode_value_key(&key_hash);
                let db_value = value.as_deref().unwrap_or(&[]).to_vec();
                (db_key, db_value)
            })
            .collect();

        let flat_refs: Vec<(&[u8], &[u8])> = flat_pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        self.tree_store.store.batch_put(&flat_refs)?;

        // 6. Update version and root hash atomically
        let old_version = *version_guard;
        *version_guard = new_version;
        *self.root_hash.write().unwrap() = new_root;

        info!(
            "✅ [JMT] Committed v{} → v{}: {} entries, root={}",
            old_version,
            new_version,
            keys.len(),
            hex::encode(&new_root.0[..8])
        );

        Ok(new_root.0)
    }

    /// Get all key-value pairs from the flat value store.
    /// Used for state replication to sub-nodes (GetCommitBatch equivalent).
    pub fn get_all(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.tree_store.store.prefix_scan(JMT_VALUE_PREFIX)
    }

    /// Verify a Merkle proof for a key-value pair against a root hash.
    /// Static method — can be called without tree instance (e.g., light client).
    pub fn verify_proof(
        root_hash: [u8; 32],
        key: &[u8],
        expected_value: Option<&[u8]>,
        proof: &jmt::proof::SparseMerkleProof<Sha256>,
    ) -> Result<()> {
        let key_hash = Self::hash_key(key);
        let expected = expected_value.map(|v| v.to_vec());
        proof
            .verify(RootHash(root_hash), key_hash, expected)
            .map_err(|e| anyhow::anyhow!("JMT proof verification failed: {}", e))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_trie() -> (JmtStateTrie, TempDir) {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let store = MdbxStore::open(dir.path(), 1).expect("Failed to open MDBX");
        let trie = JmtStateTrie::new(store);
        (trie, dir)
    }

    #[test]
    fn test_empty_trie() {
        let (trie, _dir) = create_test_trie();
        assert_eq!(trie.version(), 0);
        assert_eq!(trie.root_hash(), [0u8; 32]);
    }

    #[test]
    fn test_single_put_and_get() {
        let (trie, _dir) = create_test_trie();

        let root = trie
            .batch_update_and_commit(
                &[b"account_1".to_vec()],
                &[Some(b"balance_100".to_vec())],
            )
            .unwrap();

        assert_ne!(root, [0u8; 32]);
        assert_eq!(trie.version(), 1);

        let value = trie.get(b"account_1").unwrap();
        assert_eq!(value, Some(b"balance_100".to_vec()));
    }

    #[test]
    fn test_batch_update() {
        let (trie, _dir) = create_test_trie();

        let keys = vec![b"key_1".to_vec(), b"key_2".to_vec(), b"key_3".to_vec()];
        let values = vec![
            Some(b"val_1".to_vec()),
            Some(b"val_2".to_vec()),
            Some(b"val_3".to_vec()),
        ];

        let root = trie.batch_update_and_commit(&keys, &values).unwrap();
        assert_ne!(root, [0u8; 32]);
        assert_eq!(trie.version(), 1);

        assert_eq!(trie.get(b"key_1").unwrap(), Some(b"val_1".to_vec()));
        assert_eq!(trie.get(b"key_2").unwrap(), Some(b"val_2".to_vec()));
        assert_eq!(trie.get(b"key_3").unwrap(), Some(b"val_3".to_vec()));
    }

    #[test]
    fn test_multiple_versions() {
        let (trie, _dir) = create_test_trie();

        let root1 = trie
            .batch_update_and_commit(&[b"key_1".to_vec()], &[Some(b"v1".to_vec())])
            .unwrap();

        let root2 = trie
            .batch_update_and_commit(&[b"key_1".to_vec()], &[Some(b"v2".to_vec())])
            .unwrap();

        assert_ne!(root1, root2);
        assert_eq!(trie.version(), 2);
        assert_eq!(trie.get(b"key_1").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn test_deterministic_root_hash() {
        // Same key-value set must produce same root hash (fork safety)
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let trie1 = JmtStateTrie::new(MdbxStore::open(dir1.path(), 1).unwrap());
        let trie2 = JmtStateTrie::new(MdbxStore::open(dir2.path(), 1).unwrap());

        let keys = vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()];
        let values = vec![
            Some(b"1".to_vec()),
            Some(b"2".to_vec()),
            Some(b"3".to_vec()),
        ];

        let root1 = trie1.batch_update_and_commit(&keys, &values).unwrap();
        let root2 = trie2.batch_update_and_commit(&keys, &values).unwrap();

        assert_eq!(
            root1, root2,
            "Same state must produce same root hash (fork safety)"
        );
    }

    #[test]
    fn test_merkle_proof() {
        let (trie, _dir) = create_test_trie();

        trie.batch_update_and_commit(
            &[b"key_1".to_vec(), b"key_2".to_vec()],
            &[Some(b"val_1".to_vec()), Some(b"val_2".to_vec())],
        )
        .unwrap();

        let root = trie.root_hash();

        // Get with proof
        let (value, proof) = trie.get_with_proof(b"key_1").unwrap();
        assert_eq!(value, Some(b"val_1".to_vec()));

        // Verify proof
        JmtStateTrie::verify_proof(root, b"key_1", Some(b"val_1"), &proof).unwrap();
    }

    #[test]
    fn test_merkle_proof_non_existence() {
        let (trie, _dir) = create_test_trie();

        trie.batch_update_and_commit(&[b"key_1".to_vec()], &[Some(b"val_1".to_vec())])
            .unwrap();

        let root = trie.root_hash();

        let (value, proof) = trie.get_with_proof(b"nonexistent").unwrap();
        assert_eq!(value, None);

        JmtStateTrie::verify_proof(root, b"nonexistent", None, &proof).unwrap();
    }

    #[test]
    fn test_large_batch() {
        let (trie, _dir) = create_test_trie();

        let keys: Vec<Vec<u8>> = (0..1000u32)
            .map(|i| format!("key_{:06}", i).into_bytes())
            .collect();
        let values: Vec<Option<Vec<u8>>> = (0..1000u32)
            .map(|i| Some(format!("value_{:06}", i).into_bytes()))
            .collect();

        let root = trie.batch_update_and_commit(&keys, &values).unwrap();
        assert_ne!(root, [0u8; 32]);
        assert_eq!(trie.version(), 1);

        assert_eq!(
            trie.get(b"key_000000").unwrap(),
            Some(b"value_000000".to_vec())
        );
        assert_eq!(
            trie.get(b"key_000999").unwrap(),
            Some(b"value_000999".to_vec())
        );
    }

    #[test]
    fn test_empty_batch() {
        let (trie, _dir) = create_test_trie();
        let root = trie.batch_update_and_commit(&[], &[]).unwrap();
        assert_eq!(root, [0u8; 32]);
        assert_eq!(trie.version(), 0);
    }
}
