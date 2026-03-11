// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

fn main() {
    // Build protobuf files
    let mut protos = Vec::new();

    // Build transaction.proto
    let tx_proto = std::path::Path::new("proto/transaction.proto");
    if tx_proto.exists() {
        protos.push("proto/transaction.proto");
        println!("cargo:rerun-if-changed=proto/transaction.proto");
    } else {
        eprintln!("Warning: proto/transaction.proto not found, skipping");
    }

    // Build executor.proto
    let executor_proto = std::path::Path::new("proto/executor.proto");
    if executor_proto.exists() {
        protos.push("proto/executor.proto");
        println!("cargo:rerun-if-changed=proto/executor.proto");
    } else {
        eprintln!("Warning: proto/executor.proto not found, skipping");
    }

    // Build validator_rpc.proto (for Request/Response to Go executor)
    let validator_rpc_proto = std::path::Path::new("proto/validator_rpc.proto");
    if validator_rpc_proto.exists() {
        protos.push("proto/validator_rpc.proto");
        println!("cargo:rerun-if-changed=proto/validator_rpc.proto");
    } else {
        eprintln!("Warning: proto/validator_rpc.proto not found, skipping");
    }

    if !protos.is_empty() {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        // Keep build output clean: avoid emitting `cargo:warning=` for normal progress logs.
        // (Using `cargo:warning=` makes Cargo print these as "warning: <crate>: ...",
        // which looks like a compiler warning even though it's not.)
        prost_build::Config::new()
            .out_dir(&out_dir)
            .compile_protos(&protos, &["proto"])
            .unwrap_or_else(|e| {
                panic!("Failed to compile protobuf: {}", e);
            });
    } else {
        panic!("No protobuf files found to compile!");
    }
}
