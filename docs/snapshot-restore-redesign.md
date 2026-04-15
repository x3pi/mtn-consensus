# Snapshot Restore Redesign: "Slow Node" Architecture

**Date**: 2026-04-15
**Status**: IMPLEMENTED (2026-04-15)
**Goal**: Simplify snapshot restore by treating restored nodes as slow-starting nodes. Eliminate cold_start machinery. Ensure no fork.

---

## 1. Node Types

Two node types with distinct behavior:

| Type | Role | Block Source | Executes Blocks? |
|------|------|-------------|-----------------|
| **Validator** | Consensus participant | DAG commits + P2P sync (epoch catch-up) | YES — always executes |
| **SyncOnly** | Passive observer | P2P sync only | YES — always executes |

**Key difference from current design**: ALL received blocks are EXECUTED (NOMT state transition). No more "store in LevelDB without execution" — that was the root cause of GEI inflation and forks.

---

## 2. Validator Node

### 2.1 Startup Detection

On startup, Rust queries Go for current state:

```
from_go:
  go_epoch          = Go's current epoch
  go_gei            = Go's last executed GEI (from NOMT, not inflated)
  go_block_number   = Go's last block number

from_network:
  net_epoch         = Network's current epoch (from peers)
  net_block         = Network's latest block number (from peers)
```

Decision logic:

```
if go_epoch < net_epoch:
    → EPOCH BEHIND (Case 1: Epoch Catch-up)
else:
    → SAME EPOCH (Case 2: DAG Catch-up)
```

---

### 2.2 Case 1: Epoch Behind (Slow Start / Old Snapshot)

**Situation**: Node is behind by one or more epochs.
Example: go_epoch=0, net_epoch=2.

**Flow**:

```
┌─────────────────────────────────────────────────────────┐
│  EPOCH CATCH-UP PHASE                                   │
│                                                         │
│  For each missing epoch (go_epoch → net_epoch):         │
│    1. Fetch committed blocks from peers (P2P sync)      │
│    2. EXECUTE each block immediately (NOMT state)       │
│    3. When epoch boundary reached → advance_epoch on Go │
│    4. Move to next epoch                                │
│                                                         │
│  Key: Every block is EXECUTED, not just stored.         │
│  Go's GEI advances naturally with each execution.      │
│  NOMT state is always consistent with GEI.             │
└──────────────────────────┬──────────────────────────────┘
                           │
                           │ Caught up to current epoch
                           ▼
                    → Enter Case 2
```

**Critical rule**: Blocks received during epoch catch-up are **executed immediately** by Go (NOMT state transition). This is different from the current design where P2P sync stores blocks in LevelDB without execution.

**Implementation**: Rust fetches blocks from peers, sends to Go via a NEW RPC that triggers execution (not HandleSyncBlocksRequest which only stores). Or modify HandleSyncBlocksRequest to execute when in catch-up mode.

---

### 2.3 Case 2: Same Epoch (DAG Catch-up)

**Situation**: Node is at the same epoch as the network but may have missing blocks within the epoch. This happens after:
- Epoch catch-up (Case 1) completes
- Snapshot restore within same epoch
- Normal slow restart

**Flow**:

```
┌─────────────────────────────────────────────────────────┐
│  DAG CATCH-UP + CONSENSUS PHASE                         │
│                                                         │
│  1. Join consensus (Validator mode, amnesia recovery)   │
│  2. DAG synchronizer fetches missing blocks from peers  │
│  3. Linearizer produces commits from DAG                │
│  4. CommitProcessor sends commits to Go:                │
│     - GEI ≤ go_gei → SKIP (already executed by NOMT)   │
│     - GEI > go_gei → EXECUTE (Go advances state)       │
│  5. Node participates in consensus (voting, proposing)  │
│                                                         │
│  PRIMARY source: DAG commits                            │
│  FALLBACK: If DAG can't provide blocks (detected as     │
│            falling behind epoch), switch to P2P sync    │
│            to fetch and execute missing blocks.         │
└─────────────────────────────────────────────────────────┘
```

**Lag detection mechanism** (uses GEI, NOT block number):

```
┌──────────────────────────────────────────────────────┐
│  PERIODIC CHECK (every N seconds):                   │
│                                                      │
│  dag_commit_gei  = last GEI from DAG commits         │
│  go_gei          = Go's executed GEI                  │
│  net_gei         = network's latest GEI (from peers) │
│                                                      │
│  gap = net_gei - go_gei                              │
│                                                      │
│  if gap > THRESHOLD && dag not making progress:      │
│    → Switch to P2P sync (fetch + execute blocks)     │
│    → When caught up, switch back to DAG commits      │
│                                                      │
│  if gap ≤ THRESHOLD && dag is making progress:       │
│    → Stay on DAG commits (normal consensus)          │
└──────────────────────────────────────────────────────┘
```

**Why GEI, not block number?**
- Block number can be equal (e.g., both at #100) while GEI differs wildly
- Empty commits advance GEI but NOT block number
- A node "caught up" in block number could be hundreds of empty commits behind
- Only GEI captures true execution progress

**Catch-up detection** (switch back to DAG):

```
When P2P sync brings go_gei close to net_gei (gap ≤ SMALL_THRESHOLD):
  → Stop P2P sync execution
  → DAG commits resume as primary source
  → CommitProcessor NORMAL PATH handles dedup (skip GEI ≤ go_gei)
```

---

### 2.4 Detailed Lag/Catch-Up Mechanism

**State Machine for Source Switching:**

```
┌─────────────────┐     gap > HIGH_THRESHOLD      ┌─────────────────┐
│   DAG_PRIMARY   │ ─────────────────────────────→│  P2P_FALLBACK   │
│   (normal)      │  AND dag stalled for N secs   │  (catching up)  │
└────────┬────────┘                                 └────────┬────────┘
         │                                                 │
         │ gap ≤ SMALL_THRESHOLD                           │
         │ AND consecutive progress for M secs             │
         │←────────────────────────────────────────────────┘
         ▼
┌─────────────────┐
│   DAG_PRIMARY   │
│   (resumed)     │
└─────────────────┘
```

**Parameters:**

| Parameter | Value | Purpose |
|-----------|-------|---------|
| `HIGH_THRESHOLD` | 100-200 GEI | Trigger P2P fallback when lag is significant |
| `SMALL_THRESHOLD` | 10-20 GEI | Safe to switch back to DAG |
| `DAG_STALL_TIMEOUT` | 5-10 seconds | DAG considered "not making progress" if no new commits |
| `PROGRESS_WINDOW` | 3-5 consecutive checks | Confirm DAG is truly progressing before switching back |
| `CHECK_INTERVAL` | 1-2 seconds | How often to evaluate lag |

**Algorithm (Rust side - ConsensusNode/EpochMonitor):**

```rust
struct LagMonitor {
    last_dag_gei: u64,
    last_check_time: Instant,
    dag_stall_count: u32,
    consecutive_progress: u32,
    current_source: BlockSource,
}

enum BlockSource {
    DagPrimary,      // Normal: DAG commits go to Go
    P2pFallback,     // Catch-up: P2P blocks execute in Go
}

impl LagMonitor {
    fn check_and_switch(&mut self, go_gei: u64, net_gei: u64, dag_gei: u64) {
        let gap = net_gei.saturating_sub(go_gei);
        let dag_making_progress = dag_gei > self.last_dag_gei;

        match self.current_source {
            BlockSource::DagPrimary => {
                // Check if we need to switch TO P2P fallback
                if gap > HIGH_THRESHOLD && !dag_making_progress {
                    self.dag_stall_count += 1;
                    if self.dag_stall_count >= DAG_STALL_TIMEOUT / CHECK_INTERVAL {
                        // DAG is stalled and we're far behind
                        self.switch_to_p2p_fallback(go_gei, net_gei);
                    }
                } else {
                    self.dag_stall_count = 0; // Reset if DAG is progressing
                }
            }
            BlockSource::P2pFallback => {
                // Check if we can switch BACK to DAG
                if gap <= SMALL_THRESHOLD && dag_making_progress {
                    self.consecutive_progress += 1;
                    if self.consecutive_progress >= PROGRESS_WINDOW {
                        // Confirmed: caught up and DAG is flowing
                        self.switch_to_dag_primary();
                    }
                } else {
                    self.consecutive_progress = 0; // Reset if conditions not met
                }
            }
        }

        self.last_dag_gei = dag_gei;
    }

    fn switch_to_p2p_fallback(&mut self, go_gei: u64, net_gei: u64) {
        self.current_source = BlockSource::P2pFallback;
        // Start P2P sync task that fetches and executes blocks
        // from go_gei+1 to net_gei (or until switched back)
        start_p2p_sync_execution(go_gei + 1);
    }

    fn switch_to_dag_primary(&mut self) {
        self.current_source = BlockSource::DagPrimary;
        // Stop P2P sync task
        // DAG commits will naturally flow to Go
        stop_p2p_sync_execution();
    }
}
```

**P2P Sync Execution Mode (Go side):**

When in P2P_FALLBACK mode, HandleSyncBlocksRequest (or a new RPC) must:
1. Receive blocks from Rust
2. Execute each block immediately through NOMT (not just store)
3. Update GEI after each execution
4. Acknowledge completion so Rust can track progress

**Race Condition Handling:**

```
Scenario: Switching from P2P_FALLBACK back to DAG_PRIMARY

1. Rust stops P2P sync
2. Some P2P blocks may still be in-flight to Go
3. DAG commits may start arriving before P2P finishes

Solution:
- Go's NORMAL PATH (GEI-based skip) handles this automatically
- If a DAG commit arrives with GEI that P2P hasn't reached yet,
  Go will execute it (GEI > go_gei)
- If a P2P block arrives after DAG already executed that GEI,
  Go will skip it (GEI ≤ go_gei)
- No special synchronization needed — the GEI guard is idempotent
```

**Metrics to Monitor:**

```
# HELP mtn_lag_detected_total Total number of times lag was detected
# TYPE mtn_lag_detected_total counter
mtn_lag_detected_total{node="validator-1"} 3

# HELP mtn_catch_up_duration_seconds Time spent in P2P fallback mode
# TYPE mtn_catch_up_duration_seconds histogram
mtn_catch_up_duration_seconds_bucket{le="10"} 12
mtn_catch_up_duration_seconds_bucket{le="60"} 5
mtn_catch_up_duration_seconds_bucket{le="300"} 2

# HELP mtn_gei_gap_current Current GEI gap vs network
# TYPE mtn_gei_gap_current gauge
mtn_gei_gap_current{node="validator-1"} 47

# HELP mtn_block_source_current Current block source (0=DAG, 1=P2P)
# TYPE mtn_block_source_current gauge
mtn_block_source_current{node="validator-1"} 0
```

---

## 3. SyncOnly Node

### 3.1 Normal Operation

SyncOnly nodes do NOT participate in consensus. They only receive blocks from P2P sync and execute them.

```
┌─────────────────────────────────────────────────────────┐
│  SYNCONLY MODE                                          │
│                                                         │
│  1. Receive blocks from peers via P2P sync              │
│  2. EXECUTE each block (NOMT state transition)          │
│  3. Advance GEI and block number naturally              │
│  4. Serve RPC queries from clients                      │
│                                                         │
│  NO consensus participation.                            │
│  NO DAG interaction.                                    │
└─────────────────────────────────────────────────────────┘
```

### 3.2 Promotion: SyncOnly → Validator

When a SyncOnly node is promoted to Validator (e.g., staking change):

```
┌─────────────────────────────────────────────────────────┐
│  PROMOTION FLOW                                         │
│                                                         │
│  1. Detect promotion (EpochMonitor sees node in         │
│     new committee)                                      │
│  2. Check epoch status:                                 │
│     - Behind epoch? → Case 1 (Epoch Catch-up)           │
│     - Same epoch?   → Case 2 (DAG Catch-up)             │
│  3. Stop SyncOnly P2P sync                              │
│  4. Start Validator mode (join consensus)                │
│  5. Same flow as Validator startup                       │
│                                                         │
│  Key: promotion reuses the same Validator startup logic. │
│  No special "transition" code needed.                    │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Core Design Principles

### 4.1 EXECUTE ALL BLOCKS

**Every block received (from any source) is EXECUTED by Go (NOMT state).**

No more "store without execute". This eliminates:
- GEI inflation (GEI only advances with execution)
- State mismatch (NOMT always matches GEI)
- cold_start guards (not needed — Go's GEI is always accurate)
- State-awareness guards (not needed — no stale state)

### 4.2 Single GEI Source of Truth

```
Go's GEI = number of commits EXECUTED by NOMT
         = always accurate
         = never inflated by P2P sync (because P2P sync also executes)

Rust queries Go's GEI → always gets the real executed value
CommitProcessor skips GEI ≤ go_gei → correct deduplication
```

**CRITICAL**: Progress tracking uses GEI, NOT block number. Block number and GEI are decoupled:
- Block number only increases for non-empty commits (with transactions)
- GEI increases for EVERY commit (including empty commits)

For sync and lag detection, always use GEI. Block number can be identical between nodes while GEI differs significantly due to empty commits.

### 4.3 Two Block Sources, One Execution Path

```
Source 1: DAG commits (Validator, same-epoch)
  ↓
  CommitProcessor → send_committed_subdag → Go executes

Source 2: P2P sync blocks (epoch catch-up, or DAG lag fallback)
  ↓
  NEW: sync_and_execute → Go executes (NOT just store)

Both sources → Go EXECUTES → NOMT advances → GEI advances
```

### 4.4 Automatic Fallback

```
DAG commits (primary for Validator)
    │
    ├── Making progress? → Continue with DAG
    │
    └── Falling behind? → Switch to P2P sync + execute
                              │
                              └── Caught up? → Switch back to DAG
```

---

## 5. Comparison with Current Design

| Aspect | Current Design | New Design |
|--------|---------------|------------|
| P2P sync blocks | Stored in LevelDB, NOT executed | **EXECUTED by NOMT** |
| GEI after P2P sync | Inflated (LevelDB GEI > NOMT GEI) | **Accurate** (GEI = NOMT) |
| cold_start flag | Required (complex, 7 layers) | **Not needed** |
| cold_start_snapshot_gei | Required (immutable field) | **Not needed** |
| State-awareness guards | Required (L5, L6) | **Not needed** |
| Fork risk | Multiple vectors (GEI inflation, stale state) | **Minimal** (single execution path) |
| Code complexity | ~150 lines of guards | **~0 lines of guards** |
| Catch-up speed | Fast (store only) then slow (execute) | **Steady (execute as received)** |

---

## 6. State Machine

```
                    ┌──────────────┐
                    │   STARTUP    │
                    └──────┬───────┘
                           │
                    Check committee membership
                           │
              ┌────────────┴────────────┐
              │                         │
        IN COMMITTEE              NOT IN COMMITTEE
              │                         │
              ▼                         ▼
    ┌─────────────────┐      ┌──────────────────┐
    │ Check epoch gap │      │    SYNCONLY       │
    └────────┬────────┘      │                  │
             │               │  P2P sync blocks │
    ┌────────┴────────┐      │  + execute all   │
    │                 │      │                  │
  Behind            Same     │  If promoted to  │
  epoch             epoch    │  committee:      │
    │                 │      │  → restart as    │
    ▼                 ▼      │    Validator     │
┌──────────┐  ┌───────────┐ └──────────────────┘
│ EPOCH    │  │ DAG       │
│ CATCH-UP │  │ CATCH-UP  │
│          │  │           │
│ P2P sync │  │ Join DAG  │
│ + execute│  │ consensus │
│ blocks   │  │ + execute │
│          │  │ commits   │
│ Per epoch│  │           │
│ boundary:│  │ Fallback: │
│ advance  │  │ if lag →  │
│ epoch    │  │ P2P sync  │
│          │  │ + execute │
└────┬─────┘  └───────────┘
     │              ▲
     │ Caught up    │
     │ to current   │
     │ epoch        │
     └──────────────┘
```

---

## 7. Implementation Notes

### 7.1 Rust Changes

1. **`consensus_node.rs`**: `start_as_validator = is_in_committee` (remove dag_has_history + is_lagging checks). Validator always starts ConsensusAuthority.

2. **`consensus_node.rs`**: Remove `cold_start`, `cold_start_snapshot_gei` fields from `ConsensusNode`.

3. **`executor.rs`**: Remove COLD-START PATH. Keep NORMAL PATH only (query Go GEI, skip already-executed).

4. **`mode_transition.rs`** + **`consensus_setup.rs`**: Remove cold_start propagation.

5. **`epoch_monitor.rs`**: Epoch catch-up sends blocks to Go for EXECUTION (not just storage).

6. **NEW**: Lag detection mechanism — monitor DAG progress vs network, switch to P2P sync+execute when DAG falls behind.

### 7.2 Go Changes

1. **NEW RPC or modified `HandleSyncBlocksRequest`**: P2P synced blocks must be EXECUTED by NOMT, not just stored in LevelDB. This is the most significant Go change.

2. **`block_processor_sync.go`**: Remove DB-SYNC gap jump logic (GEI is now always accurate).

3. **`block_processor_network.go`**: Remove TRANSITION SYNC state-awareness guard (no longer needed).

4. **`app_blockchain.go`**: SNAPSHOT FIX can be kept as safety net or removed (GEI won't inflate).

### 7.3 Key Invariant

```
AT ALL TIMES:
  storage.GetLastGlobalExecIndex() == NOMT's executed GEI
  
  This means:
  - Rust always gets accurate GEI from Go
  - CommitProcessor NORMAL PATH always makes correct skip decisions
  - No fork possible from GEI mismatch
```

---

## 8. Migration Path

## 8. Migration Path (Completed)

All phases successfully implemented:
1. ✅ **Phase 1**: Modify HandleSyncBlocksRequest to execute blocks (not just store) + Add `sync_and_execute` RPC.
2. ✅ **Phase 2**: Change Validator startup to always start ConsensusAuthority (removed dag_has_history check).
3. ✅ **Phase 3**: Add lag detection mechanism (DAG → P2P fallback → DAG).
4. ✅ **Phase 4**: Remove cold_start machinery entirely (cleaned up 7 Rust files).
5. ✅ **Phase 5**: Remove/mark legacy Go-side guards (L5, L6, SNAPSHOT FIX).
6. ✅ **Post-Phase 5 (Fork Fix)**: Eliminate `broadcastBackupToSub` from sync handlers.

---

## 9. Critical Network Architecture: Sub Node Block Feeds

During the implementation, a severe architectural fork was discovered where the Go Sub node advanced its block counter ahead of the Master's consensus goroutine (e.g., Sub at #275 while Master consensus was at #150).

### Root Cause
The `HandleSyncBlocksRequest` handler was immediately pushing every synced block to Sub nodes via `broadcastBackupToSub(backupBytes, blockNum)`.
- Rust P2P catchup → fetch from peers → send to local Go Master
- Go Master `HandleSyncBlocksRequest` → Stores to LevelDB + **Broadcasts to Sub**
- Sub receives block and updates state *before* Master's consensus has processed it!

### The Fix

```
✅ Correct Architecture for Master -> Sub Push:

Master Consensus (ONLY path to Sub):
  CommitWorker → GenerateBlock → CommitToDb → BroadcastToNetwork → Sub 🟢

Master Sync (stores only, NEVER broadcasts to Sub):
  HandleSyncBlocksRequest → Store LevelDB → PersistBackup → STOP 🟢
```

By removing `broadcastBackupToSub` from both the store-only and execute-mode paths of `HandleSyncBlocksRequest`, the Sub node is guaranteed to strictly follow the Master's single source of truth (the consensus pipeline). Synced blocks are merely written to PebbleDB (`block_data_topic-N`) so the Sub can fetch them gracefully via the 3-tier recovery (backup → PebbleDB → peer) if needed. This preserves the single writer principle and eliminates the race condition fork.
