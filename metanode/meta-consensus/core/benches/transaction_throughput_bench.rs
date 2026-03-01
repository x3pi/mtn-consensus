// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! TPS Benchmark: Transaction submission and consumption throughput.
//!
//! Run with:
//!   cd /home/abc/chain-n/Mysticeti/metanode/meta-consensus/core
//!   cargo bench --bench transaction_throughput_bench
//!
//! This benchmark measures:
//! 1. TransactionClient.submit_no_wait() throughput (how fast txs can be queued)
//! 2. TransactionConsumer.next() throughput (how fast txs can be fetched for blocks)
//! 3. Combined submit→consume pipeline throughput
//! 4. Submit throughput at varying transaction sizes

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use meta_protocol_config::ProtocolConfig;
use tokio::runtime::Runtime;

use consensus_core::{
    context::Context,
    transaction::{TransactionClient, TransactionConsumer},
};

/// Create a test context with configurable limits
fn create_context(max_tx_size: u64, max_block_bytes: u64, max_block_count: u64) -> Arc<Context> {
    let _guard = ProtocolConfig::apply_overrides_for_testing(|_, mut config| {
        config.set_consensus_max_transaction_size_bytes_for_testing(max_tx_size);
        config.set_consensus_max_transactions_in_block_bytes_for_testing(max_block_bytes);
        config.set_consensus_max_num_transactions_in_block_for_testing(max_block_count);
        config
    });
    Arc::new(Context::new_for_test(4).0)
}

/// Benchmark: TransactionClient.submit_no_wait() throughput
///
/// Measures how fast transactions can be enqueued into the consensus pipeline.
/// This is the entry point for all transactions from the Go executor.
fn bench_submit_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_submit");

    let rt = Runtime::new().unwrap();

    for tx_count in [1, 10, 50, 100, 512] {
        group.bench_with_input(
            BenchmarkId::new("submit_no_wait", tx_count),
            &tx_count,
            |b, &tx_count| {
                let _guard = ProtocolConfig::apply_overrides_for_testing(|_, mut config| {
                    config.set_consensus_max_transaction_size_bytes_for_testing(1_000_000);
                    config.set_consensus_max_transactions_in_block_bytes_for_testing(100_000_000);
                    config.set_consensus_max_num_transactions_in_block_for_testing(10_000);
                    config
                });
                let context = Arc::new(Context::new_for_test(4).0);

                b.to_async(&rt).iter_batched(
                    || {
                        let (client, rx) = TransactionClient::new(context.clone());
                        let transactions: Vec<Vec<u8>> = (0..tx_count)
                            .map(|i| format!("bench_tx_{}", i).into_bytes())
                            .collect();
                        (client, rx, transactions)
                    },
                    |(client, _rx, transactions)| async move {
                        let _ = client.submit_no_wait(transactions).await;
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: TransactionConsumer.next() throughput  
///
/// Measures how fast transactions can be pulled from the queue for block proposals.
/// This is called by Core.try_new_block() during block creation.
fn bench_consume_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_consume");

    let rt = Runtime::new().unwrap();

    for tx_count in [10, 100, 512] {
        group.bench_with_input(
            BenchmarkId::new("consume_next", tx_count),
            &tx_count,
            |b, &tx_count| {
                let _guard = ProtocolConfig::apply_overrides_for_testing(|_, mut config| {
                    config.set_consensus_max_transaction_size_bytes_for_testing(1_000_000);
                    config.set_consensus_max_transactions_in_block_bytes_for_testing(100_000_000);
                    config.set_consensus_max_num_transactions_in_block_for_testing(10_000);
                    config
                });
                let context = Arc::new(Context::new_for_test(4).0);

                b.to_async(&rt).iter_batched(
                    || {
                        let (client, rx) = TransactionClient::new(context.clone());
                        let mut consumer = TransactionConsumer::new(rx, context.clone());

                        // Pre-fill with transactions
                        let client_clone = client.clone();
                        let handle = tokio::runtime::Handle::current();
                        handle.block_on(async {
                            for i in 0..tx_count {
                                let tx = format!("bench_tx_{}", i).into_bytes();
                                let _ = client_clone.submit_no_wait(vec![tx]).await;
                            }
                        });

                        (consumer, client) // keep client alive so channel stays open
                    },
                    |(mut consumer, _client)| async move {
                        let (txs, ack, _limit) = consumer.next();
                        // Acknowledge to prevent warnings
                        ack(consensus_types::block::BlockRef::MIN);
                        txs
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: Full submit→consume pipeline
///
/// Measures the combined throughput of submitting and consuming transactions.
/// This represents the full Rust-side transaction ingestion pipeline.
fn bench_pipeline_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_pipeline");

    let rt = Runtime::new().unwrap();

    for tx_count in [10, 100, 512] {
        group.bench_with_input(
            BenchmarkId::new("submit_then_consume", tx_count),
            &tx_count,
            |b, &tx_count| {
                let _guard = ProtocolConfig::apply_overrides_for_testing(|_, mut config| {
                    config.set_consensus_max_transaction_size_bytes_for_testing(1_000_000);
                    config.set_consensus_max_transactions_in_block_bytes_for_testing(100_000_000);
                    config.set_consensus_max_num_transactions_in_block_for_testing(10_000);
                    config
                });
                let context = Arc::new(Context::new_for_test(4).0);

                b.to_async(&rt).iter_batched(
                    || {
                        let (client, rx) = TransactionClient::new(context.clone());
                        let consumer = TransactionConsumer::new(rx, context.clone());
                        let transactions: Vec<Vec<u8>> = (0..tx_count)
                            .map(|i| format!("bench_tx_{}", i).into_bytes())
                            .collect();
                        (client, consumer, transactions)
                    },
                    |(client, mut consumer, transactions)| async move {
                        // Submit all transactions
                        let _ = client.submit_no_wait(transactions).await;

                        // Consume all transactions
                        let (txs, ack, _limit) = consumer.next();
                        ack(consensus_types::block::BlockRef::MIN);
                        txs
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: Transaction size impact
///
/// Measures how transaction size affects submit throughput.
/// Important for understanding the byte-size bottleneck vs count bottleneck.
fn bench_tx_size_impact(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_size_impact");

    let rt = Runtime::new().unwrap();

    for tx_size in [32, 256, 1024, 4096] {
        group.bench_with_input(
            BenchmarkId::new("submit_by_size", tx_size),
            &tx_size,
            |b, &tx_size| {
                let _guard = ProtocolConfig::apply_overrides_for_testing(|_, mut config| {
                    config.set_consensus_max_transaction_size_bytes_for_testing(1_000_000);
                    config.set_consensus_max_transactions_in_block_bytes_for_testing(100_000_000);
                    config.set_consensus_max_num_transactions_in_block_for_testing(10_000);
                    config
                });
                let context = Arc::new(Context::new_for_test(4).0);

                b.to_async(&rt).iter_batched(
                    || {
                        let (client, rx) = TransactionClient::new(context.clone());
                        let transaction = vec![0u8; tx_size];
                        (client, rx, transaction)
                    },
                    |(client, _rx, transaction)| async move {
                        let _ = client.submit_no_wait(vec![transaction]).await;
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group! {
    name = transaction_benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(std::time::Duration::from_secs(2))
        .measurement_time(std::time::Duration::from_secs(5));
    targets =
        bench_submit_throughput,
        bench_consume_throughput,
        bench_pipeline_throughput,
        bench_tx_size_impact,
}

criterion_main!(transaction_benches);
