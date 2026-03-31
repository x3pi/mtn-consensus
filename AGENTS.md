# agent.md — MTN-Consensus (Rust Consensus Engine)

## Project Identity

**Name**: `mtn-consensus` (MetaNode Consensus)  
**Language**: Rust (edition 2021)  
**Purpose**: DAG-based BFT consensus engine for the MetaNode blockchain, forked and extended from Sui's Mysticeti protocol.  
**Role in System**: This is the **Consensus Layer** — it orders transactions, manages epochs, and drives the Go Execution Layer via IPC (Unix Domain Sockets or TCP).

---

## Architecture Overview

The MetaNode system is a **two-process architecture**:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Rust Process (this repo)                     │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │   Consensus   │  │  Linearizer  │  │   CommitProcessor    │  │
│  │  (DAG-BFT)   │──│ (Sub-DAG →   │──│ (Send ordered blocks │  │
│  │              │  │  linear seq) │  │  to Go via UDS/TCP)  │  │
│  └──────────────┘  └──────────────┘  └──────────┬───────────┘  │
│                                                  │              │
│  ┌──────────────┐  ┌──────────────┐              │              │
│  │ TxSocketServer│  │ EpochMonitor │              │              │
│  │ (receive txs │  │ (manage epoch│              │              │
│  │  from Go Sub)│  │  transitions)│              │              │
│  └──────────────┘  └──────────────┘              │              │
└──────────────────────────────────────────────────┼──────────────┘
                                                   │ UDS / TCP
┌──────────────────────────────────────────────────┼──────────────┐
│                   Go Process (mtn-simple-2025)   │              │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────▼──────────┐  │
│  │  Transaction  │  │    State     │  │   SocketExecutor     │  │
│  │    Pool      │  │  (MPT Tries) │  │ (receive ordered     │  │
│  │              │  │              │  │  blocks from Rust)   │  │
│  └──────────────┘  └──────────────┘  └──────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### Key Design Principles
1. **Rust is Authority**: Rust decides transaction ordering and block finality. Go only executes.
2. **Deterministic Transitions**: All epoch boundaries, timestamps, and committee data are derived from blockchain state (block headers), never from `time.Now()`.
3. **Advance-First Protocol (Pillar 47)**: During epoch transitions, Rust first notifies Go to advance its epoch, then fetches the new committee.
4. **Deferred Commit (Pillar 31.3)**: Non-blocking sub-DAG commitment — if ancestor blocks are missing, the commit is deferred without blocking the core thread.
5. **Ancestor-Only Linearization (Pillar 31.6)**: Only blocks reachable via explicit causal traversal from the leader are committed (ensures bit-perfect determinism).

---

## Repository Structure

```
mtn-consensus/
├── metanode/                        # Main application (Rust binary)
│   ├── Cargo.toml                   # Workspace root, version 0.9.42
│   ├── src/
│   │   ├── main.rs                  # Entry point (CLI: start / generate)
│   │   ├── config.rs                # NodeConfig loading (TOML)
│   │   ├── consensus/               # High-level consensus orchestration
│   │   │   ├── commit_processor.rs  # Sends committed blocks to Go ExecutorClient
│   │   │   ├── epoch_transition.rs  # Handles EndOfEpoch system transactions
│   │   │   ├── tx_recycler.rs       # Recycles uncommitted txs after leader change
│   │   │   └── clock_sync.rs        # NTP-based clock synchronization
│   │   ├── node/                    # Node lifecycle & state machine
│   │   │   ├── consensus_node.rs    # Core ConsensusNode struct (holds authority, sync, mode)
│   │   │   ├── startup.rs           # InitializedNode, bootstrapping logic
│   │   │   ├── epoch_monitor.rs     # Polls Go for epoch changes, triggers transitions
│   │   │   ├── block_coordinator.rs # Dual-Stream deduplication (Consensus vs Sync blocks)
│   │   │   ├── committee.rs         # Committee construction from Go validator data
│   │   │   ├── committee_source.rs  # Peer discovery for committee data
│   │   │   ├── transition/          # Epoch transition state machine
│   │   │   ├── executor_client/     # IPC client to Go (UDS/TCP, Protobuf)
│   │   │   ├── dual_stream.rs       # Manages concurrent Consensus + Sync data streams
│   │   │   ├── epoch_checkpoint.rs  # Crash-recovery checkpointing for transitions
│   │   │   ├── sync.rs              # SyncOnly mode logic
│   │   │   └── catchup.rs           # CatchupManager for peer block fetching
│   │   ├── network/
│   │   │   └── tx_socket_server.rs  # Receives transaction batches from Go Sub-node
│   │   └── types/                   # Shared type definitions
│   │
│   ├── meta-consensus/              # Core consensus library
│   │   ├── core/src/
│   │   │   ├── authority_node.rs    # AuthorityNode (consensus participant)
│   │   │   ├── linearizer.rs        # DAG → linear sequence converter
│   │   │   ├── commit_observer.rs   # Monitors commits, attaches reputation scores
│   │   │   ├── commit_syncer.rs     # Syncs commits from peers (SyncOnly mode)
│   │   │   ├── commit_finalizer.rs  # Finalizes commit sub-DAGs
│   │   │   ├── core.rs / core_thread.rs  # Core consensus loop
│   │   │   ├── synchronizer.rs      # Block synchronization with peers
│   │   │   ├── block_manager.rs     # Manages DAG block storage
│   │   │   ├── block_verifier.rs    # Validates incoming blocks
│   │   │   ├── authority_service.rs # RPC service for serving blocks to peers
│   │   │   ├── leader_schedule.rs   # Leader election per round
│   │   │   ├── legacy_store.rs      # LegacyEpochStoreManager for cross-epoch sync
│   │   │   └── transaction.rs       # Transaction handling
│   │   ├── types/                   # Core consensus types (Block, Commit, etc.)
│   │   └── config/                  # Consensus parameters
│   │
│   ├── proto/                       # Protobuf definitions for Go ↔ Rust IPC
│   ├── config/                      # TOML configuration files per node
│   ├── scripts/
│   │   ├── mtn-orchestrator.sh      # Main cluster orchestration script
│   │   ├── test_restart_loop.sh     # Automated restart/recovery testing
│   │   └── generate_genesis_from_rust_keys.sh
│   └── deploy/                      # Systemd service files for production
│
├── crates/                          # Shared utility libraries
│   ├── shared-crypto/               # Cryptographic primitives (BLS12-381)
│   ├── typed-store/                 # RocksDB wrapper
│   ├── mysten-network/              # Network abstractions (anemo)
│   ├── mysten-metrics/              # Prometheus metrics
│   └── meta-protocol-config/        # Protocol version configuration
│
├── client/                          # Client SDK / tools
├── config/                          # Global configurations
└── docs/                            # MkDocs documentation site
```

---

## Key Concepts

### Node Modes
A MetaNode operates in one of two modes:
- **`Validator`**: Active consensus participant — proposes blocks, votes, commits.
- **`SyncOnly`**: Passive observer — fetches committed blocks from peers, waits for committee promotion.

### Epoch Lifecycle
1. An epoch lasts for a configurable number of blocks.
2. The `EpochMonitor` polls the Go layer for epoch changes.
3. When an `EndOfEpoch` system transaction is committed:
   - Rust calls `advance_epoch` RPC to Go (Pillar 47 — Advance-First).
   - Rust fetches the new committee from Go.
   - Old `ConsensusAuthority` is stopped; new one is started.
4. Crash recovery via `EpochCheckpoint` (atomic write-then-rename).

### Communication Protocol (Rust ↔ Go)
- **Transport**: Unix Domain Sockets (local) or TCP (distributed).
- **Encoding**: Protobuf (`proto/validator_rpc.proto`).
- **Key RPCs**:
  - `GetEpochBoundaryData` — Fetch epoch boundary info from Go.
  - `AdvanceEpoch` — Notify Go of epoch change.
  - `GetValidatorsAtBlock` — Fetch committee for a given block height.
  - `SendCommittedSubDag` — Send ordered transactions to Go for execution.
  - `SyncBlocks` — Bulk block sync for SyncOnly mode.

### Block Coordinator (Dual-Stream)
Two sources feed blocks to Go:
1. **Consensus Stream** — Blocks from local consensus (strict sequential).
2. **Sync Stream** — Blocks from peer sync (gap-filling).

The `BlockCoordinator` in `block_coordinator.rs` deduplicates and sequences them using `next_expected_index` and a `BTreeMap` buffer.

---

## Build & Run

### Build
```bash
cd metanode
cargo build --release
# Binary: target/release/metanode
```

### Run a single node
```bash
./target/release/metanode start --config config/node-0.toml
```

### Run the full cluster (5 validators)
```bash
cd scripts
bash mtn-orchestrator.sh start
```

### Generate configs for N nodes
```bash
./target/release/metanode generate --nodes 5 --output config/
```

---

## Storage
- **Consensus DB**: RocksDB, stored per-epoch at `<storage_path>/epochs/epoch_{N}/consensus_db`.
- **Epoch Checkpoint**: `<storage_path>/epoch_transition.checkpoint` (crash recovery).

---

## Key Dependencies
| Crate | Purpose |
|-------|---------|
| `tokio` 1.47.1 | Async runtime |
| `prost` / `prost-build` | Protobuf serialization |
| `fastcrypto` (Mysten) | BLS12-381 signatures |
| `typed-store` (local) | RocksDB abstraction |
| `anemo` (via mysten-network) | Authenticated peer-to-peer networking |
| `socket2` | TCP keepalive for distributed deployment |

---

## Critical Invariants (Fork Safety)
1. **No `time.Now()` for consensus-critical timestamps** — All timestamps derived from block headers.
2. **Canonical validator sorting** — Both Go and Rust sort by `AuthorityKey` bytes (BLS public key).
3. **Deterministic sub-DAG linearization** — Ancestor-only traversal, no round-wide quorum.
4. **Boundary block determinism** — Epoch boundaries are explicit, consensus-certified values.
5. **Blocking send to Go** — No dropped blocks, ever.

---

## Environment Variables
```bash
RUST_LOG=metanode=info,consensus_core=info   # Logging level
```

---

## Testing
```bash
cargo test                          # Unit tests
bash scripts/test_restart_loop.sh   # Integration: restart recovery
bash scripts/test_system.sh         # Full system integration test
```

---

## Common Patterns

### Error Handling
- Production code uses `anyhow::Result` and `.context()` — no `.unwrap()` in hot paths.
- Critical failures (state divergence) use `panic!()` deliberately (fail-fast safety).

### Epoch Transition Pattern
```
EpochMonitor detects Go epoch > current
  → advance_epoch RPC to Go (Pillar 47)
  → fetch_committee from Go
  → stop old AuthorityNode
  → start new AuthorityNode with new committee
  → checkpoint to disk
```

### Transaction Flow
```
Go Sub-node pool → UDS batch → Rust TxSocketServer
  → pending_transactions_queue → DAG Block proposal
  → Consensus rounds → Leader elected → Linearizer
  → CommittedSubDag → CommitProcessor → ExecutorClient
  → UDS/TCP → Go Master (execute & commit state)
```
