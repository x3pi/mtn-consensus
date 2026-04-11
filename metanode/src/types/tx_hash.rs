// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use prost::Message;
use sha3::{Digest, Keccak256};
use tracing::warn;

// Include generated protobuf code
#[allow(dead_code)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/transaction.rs"));
}

use proto::{AccessTuple, Transaction, Transactions};

/// Hash một single Transaction bytes — KHÔNG thử decode array.
///
/// Dùng khi biết chắc `tx_data` là pb.Transaction đơn (ví dụ: đã được
/// zero-copy extract từ pb.Transactions trong tx_socket_server.rs).
/// Loại bỏ hoàn toàn false-positive decode.
pub fn calculate_transaction_hash_single(tx_data: &[u8]) -> Vec<u8> {
    if let Ok(tx) = Transaction::decode(tx_data) {
        return calculate_single_transaction_hash(&tx);
    }
    warn!("Failed to parse single Transaction protobuf, using raw data hash");
    Keccak256::digest(tx_data).to_vec()
}

/// Calculate hash for a single Transaction using TransactionHashData
/// This is the official hash calculation that matches Go implementation
fn calculate_single_transaction_hash(tx: &Transaction) -> Vec<u8> {
    // Create TransactionHashData from Transaction
    let hash_data = proto::TransactionHashData {
        from_address: tx.from_address.clone(),
        to_address: tx.to_address.clone(),
        amount: tx.amount.clone(),
        max_gas: tx.max_gas,
        max_gas_price: tx.max_gas_price,
        max_time_use: tx.max_time_use,
        data: tx.data.clone(),
        r#type: tx.r#type,
        last_device_key: tx.last_device_key.clone(),
        new_device_key: tx.new_device_key.clone(),
        nonce: tx.nonce.clone(),
        chain_id: tx.chain_id,
        r: tx.r.clone(),
        s: tx.s.clone(),
        v: tx.v.clone(),
        gas_tip_cap: tx.gas_tip_cap.clone(),
        gas_fee_cap: tx.gas_fee_cap.clone(),
        access_list: tx
            .access_list
            .iter()
            .map(|at| AccessTuple {
                address: at.address.clone(),
                storage_keys: at.storage_keys.clone(),
            })
            .collect(),
    };

    // Encode TransactionHashData to protobuf bytes
    let mut buf = Vec::new();
    if let Err(e) = hash_data.encode(&mut buf) {
        warn!("Failed to encode TransactionHashData: {}", e);
        // Fallback: hash the raw transaction data
        let hash = Keccak256::digest(&tx.data);
        return hash.to_vec();
    }

    // Calculate Keccak256 hash of encoded TransactionHashData
    let hash = Keccak256::digest(&buf);
    hash.to_vec()
}

/// Calculate single transaction hash and return hex string (first 8 bytes)
/// Dùng `calculate_transaction_hash_single` — KHÔNG thử decode array.
pub fn calculate_transaction_hash_single_hex(tx_data: &[u8]) -> String {
    let hash = calculate_transaction_hash_single(tx_data);
    hex::encode(&hash[..8.min(hash.len())])
}

/// Verify that transaction data is valid protobuf (Transaction or Transactions)
/// Returns true if data can be parsed as protobuf with valid fields, false otherwise
///
/// STRICT VALIDATION: After decoding, we check that at least one transaction has
/// a non-empty `from_address`. This prevents false positives from permissive
/// protobuf decoding (e.g. a raw Transaction being incorrectly decoded as Transactions).
pub fn verify_transaction_protobuf(tx_data: &[u8]) -> bool {
    // Try to parse as Transactions (multiple transactions)
    if let Ok(txs) = Transactions::decode(tx_data) {
        // Strict: at least one transaction must have a non-empty from_address
        if txs
            .transactions
            .iter()
            .any(|tx| !tx.from_address.is_empty())
        {
            return true;
        }
        // Decoded but no valid from_address — likely a false positive
    }

    // Try to parse as single Transaction
    if let Ok(tx) = Transaction::decode(tx_data) {
        // Strict: from_address must be present (valid user transaction)
        if !tx.from_address.is_empty() {
            return true;
        }
    }

    // Not valid protobuf or missing required fields
    false
}
