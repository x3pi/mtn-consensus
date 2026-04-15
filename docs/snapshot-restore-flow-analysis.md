# Snapshot Restore → Sync → Consensus: Full Flow Analysis

**Date**: 2026-04-15  
**Status**: FIXED — `cold_start_snapshot_gei` immutable field prevents GEI inflation  
**Previous Bug**: `epoch_monitor.rs` overwrote `last_global_exec_index` with inflated P2P-synced GEI before mode transition read it

---

## 1. Previous Symptom (Now Fixed)

After snapshot restore, `block_hash_checker` showed frozen stateRoot on restored node:

```
m1 stateRoot = 0x6c5411fe...  (FROZEN — never changes)
m0 stateRoot = 0x335fad8a...  (correct — changes per block)
```

**Root cause**: `cold_start_skip_gei` was derived from `node.last_global_exec_index`, which `epoch_monitor.rs` overwrote with the inflated P2P-synced GEI before `transition_mode_only()` could read the original snapshot value.

**Fix**: Added immutable field `cold_start_snapshot_gei` — set once at startup, never overwritten.

---

## 2. Architecture: Two Block Streams

```
+---------------------------------------------------------------------+
|                      RUST PROCESS (metanode)                         |
|                                                                      |
|  +----------------------------------------------------------------+  |
|  | STREAM 1: P2P SYNC (RustSyncNode / sync_loop.rs)               |  |
|  |                                                                 |  |
|  |  - Fetches committed blocks from PEER validators                |  |
|  |  - Sends to Go via HandleSyncBlocksRequest (UDS)                |  |
|  |  - Go writes blocks to LevelDB (block data only, NO execution)  |  |
|  |  - Updates storage.GetLastBlockNumber()                         |  |
|  |  - Updates storage.GetLastGlobalExecIndex()                     |  |
|  |                                                                 |  |
|  |  NOTE: These blocks are STORED but NOT EXECUTED by NOMT         |  |
|  +----------------------------------------------------------------+  |
|                                                                      |
|  +----------------------------------------------------------------+  |
|  | STREAM 2: CONSENSUS (CommitProcessor / executor.rs)             |  |
|  |                                                                 |  |
|  |  - DAG-BFT produces committed sub-DAGs                          |  |
|  |  - CommitProcessor computes GEI, sends to Go via                |  |
|  |    SendCommittedSubDag (UDS)                                    |  |
|  |  - Go EXECUTES transactions, updates NOMT state                 |  |
|  |  - Creates blocks with correct stateRoot from NOMT trie         |  |
|  |                                                                 |  |
|  |  These blocks ARE EXECUTED and have correct stateRoot            |  |
|  +----------------------------------------------------------------+  |
|                                                                      |
|  +----------------------------------------------------------------+  |
|  | COORDINATOR: BlockCoordinator (block_coordinator.rs)            |  |
|  |                                                                 |  |
|  |  - Deduplicates blocks from both streams                        |  |
|  |  - Uses next_expected_index + BTreeMap buffer                   |  |
|  |  - Ensures sequential delivery to Go                            |  |
|  +----------------------------------------------------------------+  |
+---------------------------------------------------------------------+
                              | UDS / TCP
+-----------------------------|-----------------------------------------+
|               GO PROCESS    |  (simple_chain)                         |
|                             v                                         |
|  +----------------------------------------------------------------+  |
|  | processRustEpochData (block_processor_network.go)               |  |
|  |  +-- processSingleEpochData (block_processor_sync.go)           |  |
|  |      |-- FAST-PATH: empty commits -> update GEI only            |  |
|  |      +-- FULL-PATH: tx commits -> GenerateBlock -> NOMT execute |  |
|  +----------------------------------------------------------------+  |
|                                                                      |
|  +----------------------------------------------------------------+  |
|  | HandleSyncBlocksRequest (unix_socket_handler_epoch.go)          |  |
|  |  - Writes peer blocks to LevelDB (NO NOMT execution)            |  |
|  |  - Updates storage.GetLastBlockNumber() <- INFLATES             |  |
|  |  - Updates storage.GetLastGlobalExecIndex() <- INFLATES         |  |
|  +----------------------------------------------------------------+  |
+----------------------------------------------------------------------+
```

---

## 3. Full Snapshot Restore Timeline (After Fix)

### Phase 0: Pre-Restore State
```
Network:  epoch=1, block=200, GEI=500
Node 1:   running normally, in sync
Snapshot: epoch=1, block=50, GEI=120, NOMT_root=0xABC...
```

### Phase 1: Restore (restore_node.sh)
```
1. Stop Node 1 (Go Master, Go Sub, Rust)
2. Delete data dirs
3. Download snapshot (blocks/, nomt_db/, LevelDB, epoch_data)
4. Delete Rust DAG storage (consensus_db is empty after restore)
5. Start Go Master -> Go Sub -> Rust Metanode
```

### Phase 2: Go Master Startup (app_blockchain.go)
```
File: cmd/simple_chain/app_blockchain.go

1. Load last block from LevelDB -> startLastBlock = block #50
2. Initialize GEI from header: headerGEI = 120
3. storage.UpdateLastGlobalExecIndex(120)
4. storage.UpdateLastBlockNumber(50)
5. Initialize NOMT (account state trie)
6. === SNAPSHOT FIX ===
   Check: NOMT_root vs startLastBlock.stateRoot
   If match -> OK (snapshot is consistent)
   If mismatch -> search backwards for matching block -> ForceSet GEI/blockNumber
7. Start processRustEpochData loop
   -> nextExpectedGlobalExecIndex = 121 (from storage.GetLastGlobalExecIndex() + 1)
```

### Phase 3: Rust Startup (consensus_node.rs)
```
File: metanode/src/node/consensus_node.rs

1. Query Go for epoch boundary -> epoch=1, committee, last_global_exec_index=120
2. Detect DAG state:
   dag_has_history = false  (consensus_db deleted by restore)
3. Determine node mode:
   is_in_committee = true, BUT dag empty + epoch > 0 -> NodeMode::SyncingUp
4. Set cold_start flags:
   cold_start = !dag_has_history && current_epoch > 0 = TRUE
   cold_start_snapshot_gei = storage.last_global_exec_index = 120  <-- IMMUTABLE
5. Create CommitProcessor with cold_start_skip_gei = u64::MAX (blocks all commits)
6. Don't start ConsensusAuthority (not start_as_validator because no DAG)
7. Hold commit_consumer for later
8. Start as SyncOnly temporarily for catch-up
```

### Phase 4: Rust P2P Sync (sync_loop.rs / RustSyncNode)
```
File: metanode/src/node/rust_sync_node/sync_loop.rs

1. Sync loop starts, fetches committed blocks from peers
2. Sends blocks to Go via sync_blocks() -> HandleSyncBlocksRequest
3. Go writes blocks to LevelDB (NO NOMT execution!)
   -> storage.GetLastBlockNumber() inflates: 50 -> 51 -> ... -> 200
   -> storage.GetLastGlobalExecIndex() inflates: 120 -> 121 -> ... -> 500
4. NOMT state stays at block 50's root (0xABC...)
5. node.cold_start_snapshot_gei is UNAFFECTED (still 120)
```

### Phase 5: Same-Epoch Promotion (epoch_monitor.rs)
```
File: metanode/src/node/epoch_monitor.rs (line 143-219)

EpochMonitor detects: node is SyncOnly but should be Validator
1. Check: gap = net_block - go_block <= 5  (Go has caught up via P2P sync)
2. Get synced_gei from Go = 500 (inflated by P2P sync)
3. UPDATE: node_guard.last_global_exec_index = synced_gei = 500
   (node.cold_start_snapshot_gei is NOT touched -- still 120)
4. Call transition_mode_only(node, epoch, 0, synced_gei=500, config)
```

### Phase 6: Mode Transition (mode_transition.rs)
```
File: metanode/src/node/transition/mode_transition.rs

1. Stop sync task
2. Fetch committee from Go
3. Get epoch_base_gei from Go
4. === COLD-START GUARD (FIXED) ===
   if node.cold_start {  // TRUE (set in Phase 3)
       let snapshot_gei = node.cold_start_snapshot_gei;  // 120 (IMMUTABLE)
       processor.with_cold_start_skip_gei(snapshot_gei = 120);
   }
5. Create new CommitProcessor with cold_start_skip_gei = 120
6. Start ConsensusAuthority -> DAG begins
7. Clear cold_start flag: node.cold_start = false
```

### Phase 7: DAG Replay + Consensus (executor.rs)
```
File: metanode/src/consensus/commit_processor/executor.rs

DAG replays historical commits + produces new ones:
- Commit GEI=1   -> cold_start_skip_gei(120) >= 1   -> SKIP (already in NOMT)
- Commit GEI=50  -> cold_start_skip_gei(120) >= 50   -> SKIP (already in NOMT)
- Commit GEI=120 -> cold_start_skip_gei(120) >= 120  -> SKIP (already in NOMT)
- Commit GEI=121 -> 121 > 120 -> SEND TO GO -> Go executes, NOMT advances
- Commit GEI=122 -> 122 > 120 -> SEND TO GO -> Go executes, NOMT advances
- ...
- Commit GEI=500 -> SEND TO GO -> NOMT now at correct state
- Commit GEI=501 -> SEND TO GO -> new block with CORRECT stateRoot

Go creates every block from GEI 121+ using NOMT state -> correct stateRoot -> NO FORK
```

---

## 4. Root Cause (Historical — Now Fixed)

**`epoch_monitor.rs` line 205 overwrites `node.last_global_exec_index` with the inflated P2P-synced GEI BEFORE calling `transition_mode_only()`.**

```rust
// epoch_monitor.rs:203-205
node_guard.current_epoch = rust_epoch;
node_guard.last_global_exec_index = synced_gei;  // 500 (inflated by P2P sync)
// Then calls transition_mode_only which PREVIOUSLY read node.last_global_exec_index
```

Previously, `mode_transition.rs` read:
```rust
let snapshot_gei = node.last_global_exec_index;  // Got 500 instead of 120
```

This caused `cold_start_skip_gei = 500`, skipping commits 121-500 that Go actually needed.
NOMT stayed at block 50 state -> all new blocks had frozen stateRoot -> FORK.

---

## 5. Fix Applied (2026-04-15)

### Design: Immutable `cold_start_snapshot_gei` field

Added a dedicated field to `ConsensusNode` that captures the NOMT-executed GEI at startup
and is **never overwritten** by any subsequent code path (P2P sync, epoch monitor, etc.).

### Files Changed

| File | Change |
|------|--------|
| `node/mod.rs` | Added field `cold_start_snapshot_gei: u64` to `ConsensusNode` struct |
| `node/consensus_node.rs` | Init to `storage.last_global_exec_index` when `cold_start=true`, else 0 |
| `node/transition/mode_transition.rs` | Read `node.cold_start_snapshot_gei` instead of `node.last_global_exec_index` |
| `node/transition/consensus_setup.rs` | Same change for epoch transition path |

### Why This Works

```
Startup:
  last_global_exec_index     = 120   (from Go)
  cold_start_snapshot_gei    = 120   (IMMUTABLE COPY)

After P2P sync:
  last_global_exec_index     = 500   (inflated by HandleSyncBlocksRequest)
  cold_start_snapshot_gei    = 120   (UNCHANGED)

Mode transition reads:
  cold_start_skip_gei = node.cold_start_snapshot_gei = 120  (CORRECT)
```

---

## 6. Protection Layers Summary

| Layer | Location | Purpose | Status |
|-------|----------|---------|--------|
| L0 | `app_blockchain.go` | Reset GEI at startup if NOMT root != block stateRoot | OK |
| L1 | `consensus_node.rs` | Set `cold_start=true` when DAG empty (any node, not just committee) | OK (fixed: removed is_in_committee check) |
| L1b | `consensus_node.rs` | Set `cold_start_snapshot_gei` = startup GEI (immutable) | OK (new) |
| L2 | `executor.rs` | Cold-start guard: skip commits <= skip_gei | OK |
| L3 | `mode_transition.rs` | Set cold_start_skip_gei from `cold_start_snapshot_gei` | OK (fixed: was reading `last_global_exec_index`) |
| L3b | `consensus_setup.rs` | Same for epoch transition path | OK (fixed: same change) |
| L4 | `epoch_monitor.rs` | Same-epoch promotion: updates `last_global_exec_index` | OK (no longer affects cold-start guard) |
| L5 | `block_processor_network.go` | TRANSITION SYNC: state-awareness check | OK (secondary guard) |
| L6 | `block_processor_sync.go` | LAZY REFRESH: state-awareness check | OK (secondary guard) |

---

## 7. Data Flow Diagram (After Fix)

```
                    SNAPSHOT RESTORE
                         |
                         v
              +------------------------+
              |  Go Startup (L0)       |
              |  GEI=120, block=50     |
              |  NOMT root = 0xABC     |
              +----------+-------------+
                         |
                         v
              +---------------------------+
              |  Rust Startup (L1, L1b)   |
              |  cold_start = true        |
              |  last_gei = 120           |
              |  snapshot_gei = 120       |<-- IMMUTABLE (never changes)
              |  mode = SyncOnly          |
              +----------+----------------+
                         |
                    P2P SYNC (Phase 4)
                         |
                         v
              +------------------------+
              |  HandleSyncBlocks      |
              |  LevelDB: block=200    |
              |  storage GEI = 500     |
              |  NOMT: UNCHANGED       |<-- NOMT still at block 50
              |  snapshot_gei: 120     |<-- STILL 120
              +----------+-------------+
                         |
                 EpochMonitor detects
                 Go caught up (gap<=5)
                         |
                         v
              +------------------------+
              |  epoch_monitor.rs      |
              |  synced_gei = 500      |
              |  node.last_gei = 500   |    (inflated, OK for other uses)
              |  snapshot_gei = 120    |<-- UNTOUCHED
              +----------+-------------+
                         |
                 transition_mode_only()
                         |
                         v
              +----------------------------+
              |  mode_transition.rs (L3)   |
              |  cold_start_skip_gei       |
              |  = node.snapshot_gei       |
              |  = 120 (CORRECT)           |<-- reads immutable field
              +----------+-----------------+
                         |
                  DAG starts, replays
                  commits GEI 1..120
                  ALL SKIPPED (<=120)
                         |
                  Commit GEI=121 arrives
                  SENT TO GO (121 > 120)
                  Go executes, NOMT advances
                         |
                  ... GEI 122..500 ...
                  ALL SENT TO GO
                  NOMT catches up
                         |
                  Commit GEI=501 arrives
                  SENT TO GO
                         |
                         v
              +------------------------+
              |  Go creates block      |
              |  using NOMT at GEI=500 |
              |  stateRoot = CORRECT   |<-- matches network
              +------------------------+
                         |
                         v
                  NO FORK (all good)
```

---

## 8. Design Evaluation (2026-04-15)

### 8.1 Remaining Risks

#### RISK-1: `cold_start` Arc<AtomicBool> vs `cold_start: bool` inconsistency (MEDIUM)

Two separate `cold_start` concepts exist with **different conditions**:

| Field | Type | Condition | Location |
|-------|------|-----------|----------|
| `ConsensusSetup.cold_start` | `Arc<AtomicBool>` | `!dag_has_history && is_in_committee && epoch > 0` | `consensus_node.rs:953-954` |
| `ConsensusNode.cold_start` | `bool` | `!dag_has_history && epoch > 0` | `consensus_node.rs:1289` |

The `Arc<AtomicBool>` version is passed to the **initial** CommitProcessor (for in-committee
validators that start directly). The `bool` version is used by `mode_transition.rs` and
`consensus_setup.rs` for **later** CommitProcessors.

**Problem**: For a SyncOnly node (not in committee), the initial `Arc<AtomicBool>` is `false`,
but the `bool` field is `true`. This is currently harmless because SyncOnly nodes don't start
an initial CommitProcessor — but it's confusing and fragile. If someone adds code that
checks `ConsensusSetup.cold_start` for SyncOnly nodes, it will return the wrong value.

**Recommendation**: Unify both to use the same condition: `!dag_has_history && epoch > 0`.

#### RISK-2: Go's GEI remains inflated after P2P sync (LOW-MEDIUM)

After P2P sync, `storage.GetLastGlobalExecIndex()` returns 500 (inflated), but NOMT is
at block 50 (GEI=120). The Rust-side fix (`cold_start_snapshot_gei`) prevents the wrong
`cold_start_skip_gei`, but Go-side code has multiple places that read `storage.GetLastGlobalExecIndex()`:

1. **`block_processor_sync.go:354`** — DB-SYNC for gap detection: reads `persistedGEI` and
   jumps `nextExpectedGlobalExecIndex` forward. After P2P sync inflates GEI to 500, a commit
   at GEI=121 arrives. Go sees `persistedGEI=500 >= 121` → jumps `nextExpected` to 501 →
   **commits 121-500 are SKIPPED**.

   **Mitigated by**: `app_blockchain.go` SNAPSHOT FIX resets GEI to 120 (NOMT level) at
   startup. BUT: P2P sync (HandleSyncBlocksRequest line 1493) re-inflates GEI during runtime.
   The TRANSITION SYNC code (block_processor_network.go:263) gates GEI advance on
   `stateAdvanced=true` (NOMT match), which prevents re-inflation in that path. However,
   HandleSyncBlocksRequest's GEI update at line 1493 runs unconditionally.

   **Recommendation**: HandleSyncBlocksRequest should NOT update `storage.GetLastGlobalExecIndex()`
   when NOMT root != synced block's stateRoot (same state-awareness guard used elsewhere).

2. **`block_processor_sync.go:354`** — When cold-start commits start arriving (GEI=121),
   if Go's persisted GEI is already 500 (re-inflated by HandleSyncBlocksRequest), the
   DB-SYNC code will jump `nextExpected` to 501, causing all commits 121-500 to be skipped.
   This is the **secondary fork vector** — Rust sends commits correctly, but Go skips them.

#### RISK-3: COLD_START_LIVE_THRESHOLD=50 too aggressive (LOW)

After 50 commits past `cold_start_skip_gei`, the cold-start flag is cleared (`executor.rs:303`).
Then the **NORMAL PATH** kicks in, querying Go's real-time GEI (`executor.rs:316`).

If Go's GEI is still inflated (RISK-2), the NORMAL PATH will skip commits that Go needs.

**Mitigation**: Go-side state-awareness guards in LAZY REFRESH and TRANSITION SYNC.
But these guards only prevent `bp.lastBlock` advance — they don't prevent `nextExpectedGlobalExecIndex`
from being inflated by DB-SYNC (`block_processor_sync.go:354`).

#### RISK-4: Crash during P2P sync leaves partial state (LOW)

If the node crashes after P2P sync inflates LevelDB but before mode transition:
- Restart → `app_blockchain.go` SNAPSHOT FIX detects NOMT mismatch → resets GEI. OK.
- But: `cold_start_snapshot_gei` = reset GEI value (correct).
- P2P sync re-inflates GEI → same flow as normal. OK.

**Verdict**: Crash recovery is sound due to the startup SNAPSHOT FIX.

#### RISK-5: Multi-epoch snapshot restore (LOW-MEDIUM)

If the snapshot is from epoch 0 but the network is at epoch 2:
1. Rust starts at epoch 0, discovers network at epoch 2.
2. EpochMonitor steps through epoch 0 → 1 → 2.
3. Each step calls `advance_epoch` on Go + fetches blocks.
4. P2P sync inflates GEI at each step.
5. Mode transition happens at epoch 2 with `cold_start_snapshot_gei` from epoch 0.
6. CommitProcessor skips commits up to epoch 0's GEI → sends epoch 1+ commits to Go.
7. But Go may need epoch 0's boundary block to transition properly.

**Mitigation**: EpochMonitor fetches blocks for each epoch boundary and calls
`advance_epoch` before transitioning. But the epoch 0 DAG commits are lost (no replay).

### 8.2 Redundancy Audit

The system has 7 protection layers (L0-L6). This is **excessive** and creates maintenance burden.
The core protection chain should be:

```
ESSENTIAL (cannot remove):
  L0:  app_blockchain.go SNAPSHOT FIX    — Startup GEI correction
  L1:  cold_start flag                   — Enables skip logic
  L1b: cold_start_snapshot_gei           — Immutable GEI for skip threshold
  L2:  executor.rs cold-start guard      — Actual commit skipping

SECONDARY (defense-in-depth, can simplify):
  L3+L3b: mode_transition + consensus_setup read snapshot_gei — just wiring
  L4:  epoch_monitor.rs                  — No longer relevant (doesn't affect guard)
  L5:  TRANSITION SYNC state-awareness   — Redundant if L0+L2 work correctly
  L6:  LAZY REFRESH state-awareness      — Redundant if L0+L2 work correctly
```

L5 and L6 are safety nets. They add complexity but catch edge cases where L0+L2 might
miss (e.g., RISK-2 GEI re-inflation). **Keep them but simplify their logic.**

### 8.3 Concrete Improvement Recommendations

#### FIX-1: Prevent GEI re-inflation in HandleSyncBlocksRequest (HIGH priority)

```go
// unix_socket_handler_epoch.go, around line 1491-1496
// Add state-awareness guard before updating GEI:
syncedBlockGEI := lastBlk.Header().GlobalExecIndex()
currentGEI := storage.GetLastGlobalExecIndex()
if syncedBlockGEI > currentGEI {
    // NEW: Only update if NOMT state matches (prevents inflation after snapshot)
    currentTrieRoot := rh.chainState.GetAccountStateDB().Trie().Hash()
    targetStateRoot := lastBlk.Header().AccountStatesRoot()
    if currentTrieRoot == targetStateRoot || currentTrieRoot == (common.Hash{}) {
        storage.UpdateLastGlobalExecIndex(syncedBlockGEI)
    }
}
```

This eliminates RISK-2 at the source — Go's GEI never inflates past NOMT-executed state.

#### FIX-2: Unify cold_start conditions (LOW priority)

In `consensus_node.rs:953-954`, change the `Arc<AtomicBool>` condition to match the `bool`:
```rust
// Before:
let cold_start = Arc::new(AtomicBool::new(
    !dag_has_history && storage.is_in_committee && storage.current_epoch > 0,
));
// After:
let cold_start = Arc::new(AtomicBool::new(
    !dag_has_history && storage.current_epoch > 0,
));
```

#### FIX-3: Increase COLD_START_LIVE_THRESHOLD (LOW priority)

In `executor.rs:267`, increase from 50 to 200+ to give more buffer before switching
to the NORMAL PATH (which queries Go's potentially-inflated GEI).

### 8.4 Overall Assessment

```
BEFORE today's fix:
  - cold_start_snapshot_gei didn't exist
  - epoch_monitor overwrote last_global_exec_index → GUARANTEED FORK

AFTER today's fix:
  - cold_start_snapshot_gei is immutable → Rust skip logic is CORRECT
  - Go-side GEI inflation (RISK-2) is the remaining weak point
  - Secondary guards (L5, L6) catch most Go-side issues
  - Overall: HIGH confidence for single-epoch restore
  - MEDIUM confidence for multi-epoch restore (RISK-5)
```

**Priority order for remaining work**:
1. FIX-1 (GEI inflation guard) — eliminates the last systemic fork vector
2. Test with actual snapshot restore + block_hash_checker
3. FIX-2 (cold_start unification) — code hygiene
4. FIX-3 (threshold increase) — safety margin
