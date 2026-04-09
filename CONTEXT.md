# `mtn-consensus` Context (Rust Consensus Engine)

## Overview
This is the **Rust Layer** of the MetaNode architecture. It acts as the DAG-BFT consensus engine.

## Relationship to the Go Execution Engine (`mtn-simple-2025`)
This process MUST run alongside the Go Master and Go Sub nodes to form a complete MetaNode validator.
- It receives raw transactions from the Go layer (often forwarded from Go Sub).
- It packages them into a DAG and achieves consensus.
- It sends strictly ordered blocks back to the **Go Master** via IPC (Protobuf over Unix Domain Sockets).

## Core Responsibilities
- **Transaction Ordering**: The `linearizer.rs` converts internal DAG blocks into a strict sequential order.
- **Epoch Management**: Triggers all epoch transitions. The Go layer only advances its epoch state when explicitly told by this Rust process (via **AdvanceEpoch** IPC call).

## Context Management Tip
Whenever you (Antigravity AI) work in this repository, always remember the output here drives the Execution Layer. Changes to the RPC protocol MUST be mirrored in `mtn-simple-2025`.

Please refer to `../META_NODE_ARCHITECTURE.md` for the overarching 3-part node topology.
