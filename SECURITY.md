# Security Guide — MetaNode Consensus (Rust Layer)

This document describes the security configuration and hardening measures for the
Rust consensus layer (`mtn-consensus`).

## Table of Contents

1. [Network Security](#network-security)
2. [IPC / UDS Security](#ipc--uds-security)
3. [Fork Safety](#fork-safety)
4. [Memory Safety](#memory-safety)
5. [Consensus Attack Mitigation](#consensus-attack-mitigation)

---

## Network Security

### Ports and Protocols

| Port | Protocol | Purpose | Authentication |
|---|---|---|---|
| gRPC (consensus) | TCP + TLS | Mysticeti DAG consensus | tonic TLS |
| `peer_rpc_port` | TCP | Custom peer RPC (TX forward, peer discovery) | None |
| `rust_tx_socket_path` | UDS | Transaction submission from Go | Filesystem perms |

### Peer RPC

The custom peer RPC server (`network/peer_rpc/server.rs`) uses plain TCP without TLS.
This is acceptable when nodes run on a **private network** or **VPN**.

**Recommendation**: Use WireGuard or iptables to restrict peer RPC ports to known validators.

---

## IPC / UDS Security

### Socket Permissions

| Socket | Mode | Set By |
|---|---|---|
| Transaction UDS (`metanode-tx-{N}.sock`) | `0o660` | `tx_socket_server.rs:94` |
| Notification UDS (`metanode-notification-{N}.sock`) | `0o660` | `notification_server.rs:47` |
| Executor Send (`executor{N}.sock`) | OS default | Go creates |
| Executor Receive (`rust-go.sock_1`) | OS default | Go creates |

**Ensure** Go and Rust processes run under the **same user or group** for UDS
communication to work with `0o660` permissions.

---

## Fork Safety

The Rust layer implements critical fork-prevention mechanisms:

### Fatal Halts (process::exit instead of panic)

The following conditions cause **immediate process halt** via `process::exit(1)` to
prevent state divergence:

| File | Condition | Message |
|---|---|---|
| `commit_processor.rs` | Missing committee data for epoch | `[HALTING] No committee data` |
| `commit_processor.rs` | Leader index out of range | `[HALTING] Leader index out of range` |
| `commit_processor.rs` | Invalid ETH address length | `[HALTING] Invalid ETH address` |
| `metrics.rs` | Prometheus metrics initialization failure | `[HALTING] Metrics init failed` |

### Design Principles

1. **Consensus-derived timestamps**: All block timestamps come from Rust DAG consensus
2. **Deterministic leader selection**: Leader computed from committee data + round number
3. **Epoch boundary coordination**: Go notifies Rust of epoch transitions via UDS RPC
4. **Transaction queuing**: TXs queued (not dropped) during epoch transitions

---

## Memory Safety

### Custom Code

- **0 `unsafe` blocks** in `metanode/src/` (custom code)
- All `.unwrap()` replaced with descriptive `.expect()` messages
- All bare `assert!()` hardened with descriptive messages

### Upstream Code

- `meta-consensus/core/` contains ~50 TODOs (upstream, not modified)
- `tower-0.5.2` has known lint warnings (pre-existing, not security-relevant)

---

## Consensus Attack Mitigation

| Attack Vector | Mitigation |
|---|---|
| **Leader manipulation** | Leader computed deterministically from committee + round |
| **Epoch abuse** | Epoch transitions require Go executor notification + committee update |
| **TX duplication** | TxRecycler tracks submitted TXs + committed_transaction_hashes dedup |
| **DAG equivocation** | Handled by Mysticeti core consensus protocol |
| **Block replay** | Block numbers monotonically increasing, verified by Go executor |
| **Mempool flooding** | Backpressure at 8M TX pool size; lock-free queuing during transitions |
