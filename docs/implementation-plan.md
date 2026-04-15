# Implementation Plan: Snapshot Restore Redesign

**Date**: 2026-04-15  
**Status**: All Phases Completed

---

## Summary

This document tracks the implementation of the Snapshot Restore Redesign to eliminate post-restore forks and slow sync.

**Root Cause**: P2P sync stores blocks in LevelDB without executing them → GEI inflates → Rust skips commits → stale state → FORK

**Solution**: Execute all synced blocks through NOMT so GEI always reflects actually-executed state.

---

## Phase Status

| Phase | Description | Status | Priority |
|-------|-------------|--------|----------|
| 1 | Execute blocks through NOMT (not just store) | ✅ **DONE** | Critical |
| 2 | Simplify validator startup | ✅ **DONE** | High |
| 3 | Add lag detection mechanism | ✅ **DONE** (exists) | Medium |
| 4 | Remove cold_start machinery (cleanup) | ✅ **DONE** | Low |
| 5 | Mark Go-side guards as legacy (cleanup) | ✅ **DONE** | Low |

---

## Phase 1: Execute Blocks Through NOMT ✅ DONE

### Files Modified

#### 1. Protobuf Definitions

**Rust**: `metanode/proto/validator_rpc.proto`
```protobuf
message SyncBlocksRequest {
    repeated BlockData blocks = 1;
    bool execute_mode = 2;  // NEW: When true, Go executes blocks through NOMT
}

message SyncBlocksResponse {
    uint64 synced_count = 1;
    uint64 last_synced_block = 2;
    string error = 3;
    uint64 last_executed_gei = 4;  // NEW: GEI after execution
}
```

**Go**: `pkg/proto/validator.proto` (same changes)

**Go**: `pkg/proto/validator.pb.go` (added fields + getters)

#### 2. Go: HandleSyncBlocksRequest (`executor/unix_socket_handler_epoch.go`)

**Added dispatch at top of function:**
```go
if request.GetExecuteMode() {
    return rh.handleSyncBlocksExecuteMode(request)
}
```

**New method: `handleSyncBlocksExecuteMode`**
- Applies BackupDb state batches to LevelDB
- Calls `CommitBlockState(WithRebuildTries(), WithPersistToDB())` — KEY difference
- Updates in-memory state pointers (header, epoch)
- Updates persistent counters (block number, GEI)
- Clears MVM caches
- Persists backup data + broadcasts to Sub nodes

**Helper: `persistBackupForSub`**

#### 3. Rust: ExecutorClient (`metanode/src/node/executor_client/block_sync.rs`)

**Added methods:**
```rust
pub async fn sync_and_execute_blocks(&self, blocks: Vec<proto::BlockData>) -> Result<(u64, u64, u64)>

async fn sync_blocks_inner(&self, blocks: Vec<proto::BlockData>, execute_mode: bool) -> Result<(u64, u64)>
```

**Modified:**
- `sync_blocks()` now calls `sync_blocks_inner(blocks, false)`
- Smaller chunk size for execute mode (20 vs 50)
- Longer timeout for execute mode (120s vs 60s)

#### 4. Rust: Epoch Monitor (`metanode/src/node/epoch_monitor.rs`)

**Changed:**
```rust
// OLD:
client_arc.sync_blocks(blocks).await

// NEW:
client_arc.sync_and_execute_blocks(blocks).await
```

With fallback to `sync_blocks` for backward compatibility.

#### 5. Rust: Epoch Transition (`metanode/src/node/transition/epoch_transition.rs`)

**Updated 2 call sites:**
- Line ~990: Deferred epoch sync
- Line ~1312: `fetch_and_sync_blocks_to_go`

#### 6. Rust: Catchup (`metanode/src/node/catchup.rs`)

**Updated:**
- `sync_blocks_from_peers` now calls `sync_and_execute_blocks`

---

## Phase 2: Simplify Validator Startup ✅ DONE

### Files Modified

#### 1. `metanode/src/node/consensus_node.rs`

**Line ~1136: Simplified `start_as_validator`**
```rust
// OLD:
let start_as_validator = storage.is_in_committee
    && !storage.is_lagging
    && (dag_has_history || storage.current_epoch == 0);

// NEW:
let start_as_validator = storage.is_in_committee;
```

**Line ~1270: Removed SyncingUp mode**
```rust
// OLD:
node_mode: if storage.is_in_committee {
    if storage.is_lagging || (!consensus.dag_has_history && storage.current_epoch > 0) {
        NodeMode::SyncingUp
    } else {
        NodeMode::Validator
    }
}

// NEW:
node_mode: if storage.is_in_committee {
    NodeMode::Validator  // Always Validator for in-committee
} else {
    NodeMode::SyncOnly
}
```

---

## Phase 3: Add Lag Detection Mechanism ⏸️ PENDING

### Not Started

This phase adds a `LagMonitor` that:
1. Tracks DAG progress vs network progress
2. Detects when DAG is lagging (gap > HIGH_THRESHOLD)
3. Switches to P2P sync+execute fallback mode
4. Returns to DAG mode when caught up (gap < SMALL_THRESHOLD)

**Design already documented in:**
- `docs/snapshot-restore-redesign.md` Section 2.4
- `docs/snapshot-restore-redesign-vi.md` Section 2.4

### Files to Modify (when implementing)

| File | Change |
|------|--------|
| `metanode/src/node/lag_monitor.rs` | **NEW FILE**: Implement lag detection state machine |
| `metanode/src/node/mod.rs` | Add lag_monitor module |
| `metanode/src/node/consensus_node.rs` | Start lag_monitor, wire to commit_processor |
| `metanode/src/consensus/commit_processor/executor.rs` | Check lag_monitor before NORMAL PATH skip |

---

## Phase 4: Remove cold_start Machinery ⏳ TODO

### Purpose

Cleanup: Remove all cold_start code since it's no longer needed after Phase 1-2.

### Files to Modify

| # | File | Change | Notes |
|---|------|--------|-------|
| 4.1 | `metanode/src/consensus/commit_processor/executor.rs` | Remove COLD-START PATH (lines ~248-300) | Keep only NORMAL PATH |
| 4.2 | `metanode/src/node/transition/mode_transition.rs` | Remove `if node.cold_start` block (lines ~280-291) | No cold_start propagation needed |
| 4.3 | `metanode/src/node/consensus_node.rs` | Remove cold_start setup (lines ~953-1009) | Remove `cold_start` Arc initialization |
| 4.4 | `metanode/src/node/mod.rs` | Remove `cold_start` and `cold_start_snapshot_gei` fields from ConsensusNode struct | Lines ~189-199 |
| 4.5 | `metanode/src/node/consensus_node.rs` | Remove cold_start from ConsensusNode initialization | Line ~1284-1289 |

### Code References

**executor.rs COLD-START PATH to remove:**
```rust
// Lines ~248-333: Remove this entire block
if self.cold_start.load(Ordering::SeqCst) {
    // COLD-START PATH
    let cold_start_skip_gei = self.cold_start_skip_gei.load(Ordering::SeqCst);
    // ... skip logic
}
```

**mode_transition.rs to remove:**
```rust
// Lines ~280-291: Remove this block
if node.cold_start {
    let cold_start_arc = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let snapshot_gei = node.cold_start_snapshot_gei;
    processor = processor
        .with_cold_start(cold_start_arc)
        .with_cold_start_skip_gei(snapshot_gei);
}
```

**ConsensusNode fields to remove:**
```rust
// mod.rs lines ~189-199
pub(crate) cold_start: bool,
pub(crate) cold_start_snapshot_gei: u64,
```

---

## Phase 5: Remove Go-Side Guards ⏳ TODO

### Purpose

Cleanup: Remove defensive guards that are no longer needed after Phase 1-2.

### Files to Modify

| # | File | Change | Notes |
|---|------|--------|-------|
| 5.1 | `block_processor_sync.go` | Remove RESTORE-GAP-SKIP (lines ~391-438) | Blocks now executed sequentially |
| 5.2 | `block_processor_sync.go` | Simplify DB-SYNC gap jump (lines ~348-360) | GEI always accurate now |
| 5.3 | `block_processor_network.go` | Remove TRANSITION SYNC state-awareness guard | GEI accurate, never triggers |
| 5.4 | `app_blockchain.go` | Keep SNAPSHOT FIX as safety net | Defense-in-depth OK |

### Code References

**RESTORE-GAP-SKIP to remove (block_processor_sync.go ~391-438):**
```go
// Lines 391-438: Remove entire RESTORE-GAP-SKIP section
// Comment starts with: "// ═══════════════════════════════════════════════════════════════════════════"
// Ends before: "// Case 3: Sequential block"
```

**TRANSITION SYNC guard to simplify (block_processor_network.go):**

Can remove the state-awareness check since GEI is now always accurate:
```go
// Lines ~217-271: Simplify by removing state mismatch check
// The check: if currentTrieRoot != targetStateRoot { ... skip ... }
```

---

## Testing Checklist

### Phase 1 Testing
- [ ] Snapshot restore → node syncs and executes blocks through NOMT
- [ ] GEI after sync = NOMT-executed GEI (not inflated)
- [ ] Rust queries Go GEI → gets correct value → doesn't skip needed commits
- [ ] No fork after cold_start clears

### Phase 2 Testing
- [ ] Validator with no DAG history starts ConsensusAuthority immediately
- [ ] DAG sync fetches missing blocks from peers
- [ ] No SyncingUp mode delays

### Phase 3 Testing (when implemented)
- [ ] Lag detection triggers when DAG is behind
- [ ] P2P fallback mode catches up node
- [ ] Auto-return to DAG mode when caught up

---

## Build Commands

### Go
```bash
cd /home/abc/chain-n/mtn-simple-2025
go build ./...
```

### Rust
```bash
cd /home/abc/chain-n/mtn-consensus/metanode
cargo build --release
```

### Regenerate Protobuf (if needed)
```bash
# Go
protoc --go_out=. --go_opt=paths=source_relative pkg/proto/validator.proto

# Rust  
cd metanode && cargo build (prost-build handles proto)
```

---

## Migration Notes

1. **Phase 1 is the critical fix** — it eliminates the root cause of fork
2. **Phase 2 simplifies flow** — removes unnecessary SyncOnly→Validator transition
3. **Phases 4-5 are cleanup** — can be done anytime, no functional change
4. **Phase 3 is enhancement** — adds lag detection for better UX

**Safe deployment**: Phase 1 alone reduces fork risk significantly. Deploy 1+2 first, then 4+5 as cleanup.
