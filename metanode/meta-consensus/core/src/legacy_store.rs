// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Legacy Epoch Store Manager
//!
//! Keeps RocksDB stores from previous epochs open in read-only mode
//! so lagging nodes can still fetch commits/blocks for synchronization.

use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc};
use tracing::info;

use crate::storage::Store;

/// Manages stores from previous epochs for legacy sync support.
///
/// When a node transitions to a new epoch, the old store is moved here
/// so that lagging nodes can still fetch commits and blocks from the
/// previous epoch. Only the most recent N epochs are kept.
pub struct LegacyEpochStoreManager {
    stores: RwLock<HashMap<u64, Arc<dyn Store>>>,
    max_epochs: usize,
}

impl LegacyEpochStoreManager {
    /// Create a new manager that keeps up to `max_epochs` previous epoch stores.
    pub fn new(max_epochs: usize) -> Self {
        info!(
            "📦 [LEGACY STORE] Creating LegacyEpochStoreManager with max_epochs={}",
            max_epochs
        );
        Self {
            stores: RwLock::new(HashMap::new()),
            max_epochs,
        }
    }

    /// Add a store for a completed epoch.
    /// If we exceed max_epochs, the oldest store is dropped.
    pub fn add_store(&self, epoch: u64, store: Arc<dyn Store>) {
        let mut stores = self.stores.write();

        info!(
            "📦 [LEGACY STORE] Adding store for epoch {} (current stores: {})",
            epoch,
            stores.len()
        );

        stores.insert(epoch, store);

        // Cleanup: keep only the most recent max_epochs
        // max_epochs == 0: archive mode — keep all stores
        if self.max_epochs > 0 && stores.len() > self.max_epochs {
            let oldest_epoch = *stores.keys().min().expect("stores checked non-empty above");
            stores.remove(&oldest_epoch);
            info!(
                "🗑️ [LEGACY STORE] Removed oldest epoch {} store",
                oldest_epoch
            );
        }
    }

    /// Get a store for a specific epoch if available.
    pub fn get_store(&self, epoch: u64) -> Option<Arc<dyn Store>> {
        self.stores.read().get(&epoch).cloned()
    }

    /// Get all available legacy stores.
    pub fn get_all_stores(&self) -> Vec<(u64, Arc<dyn Store>)> {
        self.stores
            .read()
            .iter()
            .map(|(e, s)| (*e, s.clone()))
            .collect()
    }

    /// Remove a specific epoch's store.
    pub fn remove_store(&self, epoch: u64) -> Option<Arc<dyn Store>> {
        let mut stores = self.stores.write();
        let removed = stores.remove(&epoch);
        if removed.is_some() {
            info!("🗑️ [LEGACY STORE] Removed store for epoch {}", epoch);
        }
        removed
    }

    /// Check if a store exists for a given epoch.
    pub fn has_store(&self, epoch: u64) -> bool {
        self.stores.read().contains_key(&epoch)
    }

    /// Get the number of stored epochs.
    pub fn store_count(&self) -> usize {
        self.stores.read().len()
    }
}

impl Default for LegacyEpochStoreManager {
    fn default() -> Self {
        Self::new(2) // Reduced: keep 2 previous epochs (saves ~6GB RAM)
    }
}
