// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

/// Calculate deterministic global execution index (Checkpoint Sequence Number)
/// This ensures all nodes compute the same value from consensus state
///
/// CRITICAL FIX FOR FORK PREVENTION:
/// Previous formula: global_exec_index = last_global_exec_index + 1
///   - Problem: Each node tracks last_global_exec_index locally
///   - When a new node joins mid-epoch, its local tracker is wrong → Fork!
///
/// New formula: global_exec_index = epoch_base_index + commit_index
///   - epoch_base_index: last_global_exec_index at the START of current epoch (from Go)
///   - commit_index: Consensus-agreed value from Mysticeti (same for all nodes)
///   - Result: Deterministic across all nodes, even for late joiners
///
/// Example:
/// - Epoch 0: base=0, commit 1 → global=1, commit 2 → global=2, ..., commit 4000 → global=4000
/// - Epoch 1: base=4000, commit 1 → global=4001, commit 2 → global=4002, ...
/// - Node-X joins at commit 500 of epoch 1: base=4000, commit 500 → global=4500 ✓
pub fn calculate_global_exec_index(
    _epoch: u64,           // Not used in formula but kept for logging/debugging
    commit_index: u32,     // Consensus-agreed value from Mysticeti
    epoch_base_index: u64, // last_global_exec_index at epoch START (not current!)
) -> u64 {
    // FORK-SAFE FORMULA: All nodes with same commit_index will get same global_exec_index
    // because commit_index is agreed upon by Mysticeti consensus
    epoch_base_index + commit_index as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_global_exec_index_consensus_based() {
        // FORK-SAFE: global_exec_index = epoch_base_index + commit_index
        // All nodes with same commit_index get same result

        // Epoch 0: base=0
        assert_eq!(calculate_global_exec_index(0, 1, 0), 1); // First commit
        assert_eq!(calculate_global_exec_index(0, 100, 0), 100); // 100th commit
        assert_eq!(calculate_global_exec_index(0, 4000, 0), 4000); // End of epoch 0

        // Epoch 1: base=4000 (last_global_exec_index at epoch 0 end)
        assert_eq!(calculate_global_exec_index(1, 1, 4000), 4001); // First commit of epoch 1
        assert_eq!(calculate_global_exec_index(1, 500, 4000), 4500); // Node joins at commit 500 → correct!
        assert_eq!(calculate_global_exec_index(1, 1000, 4000), 5000);

        // Epoch 2: base=5000
        assert_eq!(calculate_global_exec_index(2, 1, 5000), 5001);
    }

    #[test]
    fn test_global_exec_index_deterministic_across_nodes() {
        // CRITICAL: All nodes must compute same value for same commit_index
        // This is the key property that prevents fork

        let epoch_base = 10000; // Set at epoch start, same for all nodes
        let commit_index = 500; // From Mysticeti consensus, same for all nodes

        // Simulate different nodes computing the same commit
        let node_0_result = calculate_global_exec_index(1, commit_index, epoch_base);
        let node_1_result = calculate_global_exec_index(1, commit_index, epoch_base);
        let node_4_result = calculate_global_exec_index(1, commit_index, epoch_base); // Late joiner

        assert_eq!(node_0_result, node_1_result);
        assert_eq!(node_1_result, node_4_result);
        assert_eq!(node_0_result, 10500); // epoch_base + commit_index
    }
}
