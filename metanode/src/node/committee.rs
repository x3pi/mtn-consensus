// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use crate::node::executor_client::ExecutorClient;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use consensus_config::{
    Authority, AuthorityPublicKey, Committee, NetworkPublicKey, ProtocolPublicKey,
};
use fastcrypto::traits::ToFromBytes;
use fastcrypto::{bls12381, ed25519};
use mysten_network::Multiaddr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

#[allow(dead_code)]
pub async fn build_committee_from_go_validators_at_block_with_epoch(
    executor_client: &Arc<ExecutorClient>,
    requested_block: u64,
    epoch: u64,
) -> Result<Committee> {
    let mut attempt = 0;
    let max_attempts = 10; // Prevent infinite loop
    let mut current_block = requested_block;

    loop {
        attempt += 1;

        match executor_client.get_validators_at_block(current_block).await {
            Ok((validators, _, _)) => {
                if !validators.is_empty() {
                    tracing::info!(
                        "✅ [COMMITTEE] Successfully got {} validators for block {} (epoch {})",
                        validators.len(),
                        current_block,
                        epoch
                    );
                    return build_committee_from_validator_list(validators, epoch);
                } else {
                    tracing::warn!("⏳ [COMMITTEE] Go returned 0 validators for block {}. Retrying... (attempt {}/{})",
                        current_block, attempt, max_attempts);
                }
            }
            Err(e) => {
                // Check if it's the specific "block not committed" error
                if e.to_string().contains("not been committed to DB yet")
                    || e.to_string().contains("not committed")
                {
                    // Try to get the last committed block and use that instead
                    if let Ok((last_committed, _)) = executor_client.get_last_block_number().await {
                        if last_committed < current_block
                            && last_committed > 0
                            && current_block == requested_block
                        {
                            // Only fallback once, to prevent infinite fallback loop
                            tracing::warn!("⚠️ [COMMITTEE] Block {} not committed yet, falling back to last committed block {} (attempt {}/{})",
                                current_block, last_committed, attempt, max_attempts);
                            current_block = last_committed;
                            continue; // Retry with the last committed block
                        }
                    }
                }

                tracing::error!(
                    "❌ [COMMITTEE] Failed to get validators for block {}: {} (attempt {}/{})",
                    current_block,
                    e,
                    attempt,
                    max_attempts
                );
            }
        }

        if attempt >= max_attempts {
            return Err(anyhow::anyhow!(
                "Failed to get committee after {} attempts for block {} (epoch {})",
                max_attempts,
                current_block,
                epoch
            ));
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Build committee directly from ValidatorInfo list (no RPC call needed)
/// Used by get_epoch_boundary_data() which already has the validators
pub async fn build_committee_from_validator_info_list(
    validators: &[crate::node::executor_client::proto::ValidatorInfo],
    epoch: u64,
) -> Result<Committee> {
    build_committee_from_validator_list(validators.to_vec(), epoch)
}

pub fn build_committee_from_validator_list(
    validators: Vec<crate::node::executor_client::proto::ValidatorInfo>,
    epoch: u64,
) -> Result<Committee> {
    let mut sorted_validators: Vec<_> = validators.into_iter().collect();
    // CRITICAL FIX: Sort by authority_key (BLS public key) for DETERMINISTIC ordering
    // Previously sorted by 'address' (P2P multiaddr like /ip4/127.0.0.1/tcp/9000)
    // but Go sorts by 'Address().Hex()' (wallet address like 0x1234...)
    // These are DIFFERENT fields causing different ordering → different genesis hashes → FORK!
    // Now we sort by authority_key which is unique per validator and stable across contexts
    sorted_validators.sort_by(|a, b| a.authority_key.cmp(&b.authority_key));

    let mut authorities = Vec::new();

    for (idx, validator) in sorted_validators.iter().enumerate() {
        let stake = validator.stake.parse::<u64>()?;
        // CHANGED: Use p2p_address for network communication (address is now Ethereum wallet address)
        let address: Multiaddr = validator.p2p_address.parse()?;

        // Auth Key
        let authority_key_bytes = if validator.authority_key.starts_with("0x") {
            hex::decode(&validator.authority_key[2..])?
        } else {
            match STANDARD.decode(&validator.authority_key) {
                Ok(b) => b,
                Err(_) => hex::decode(&validator.authority_key)?,
            }
        };
        let authority_pubkey =
            bls12381::min_sig::BLS12381PublicKey::from_bytes(&authority_key_bytes)?;
        let authority_key = AuthorityPublicKey::new(authority_pubkey);

        // Protocol Key
        let protocol_key_bytes = if validator.protocol_key.starts_with("0x") {
            hex::decode(&validator.protocol_key[2..])?
        } else {
            match STANDARD.decode(&validator.protocol_key) {
                Ok(b) => b,
                Err(_) => hex::decode(&validator.protocol_key)?,
            }
        };
        let protocol_pubkey = ed25519::Ed25519PublicKey::from_bytes(&protocol_key_bytes)?;
        let protocol_key = ProtocolPublicKey::new(protocol_pubkey);

        // Network Key
        let network_key_bytes = if validator.network_key.starts_with("0x") {
            hex::decode(&validator.network_key[2..])?
        } else {
            match STANDARD.decode(&validator.network_key) {
                Ok(b) => b,
                Err(_) => hex::decode(&validator.network_key)?,
            }
        };
        let network_pubkey = ed25519::Ed25519PublicKey::from_bytes(&network_key_bytes)?;
        let network_key = NetworkPublicKey::new(network_pubkey);

        let hostname = if !validator.name.is_empty() {
            validator.name.clone()
        } else {
            format!("node-{}", idx)
        };

        authorities.push(Authority {
            stake,
            address,
            hostname,
            authority_key,
            protocol_key,
            network_key,
        });
    }

    info!(
        "📊 Built committee with {} authorities for epoch {}",
        authorities.len(),
        epoch
    );
    Ok(Committee::new(epoch, authorities))
}

/// Build committee from validator list and also return Ethereum addresses for leader lookup
/// Returns (Committee, Vec<eth_address_bytes>) where eth_addresses are in the same order as authorities
pub fn build_committee_with_eth_addresses(
    validators: Vec<crate::node::executor_client::proto::ValidatorInfo>,
    epoch: u64,
) -> Result<(Committee, Vec<Vec<u8>>)> {
    let mut sorted_validators: Vec<_> = validators.into_iter().collect();
    // CRITICAL: Sort by authority_key for DETERMINISTIC ordering (same as build_committee_from_validator_list)
    sorted_validators.sort_by(|a, b| a.authority_key.cmp(&b.authority_key));

    let mut authorities = Vec::new();
    let mut eth_addresses = Vec::new(); // Collect eth addresses in same order as authorities

    for (idx, validator) in sorted_validators.iter().enumerate() {
        let stake = validator.stake.parse::<u64>()?;
        let address: Multiaddr = validator.p2p_address.parse()?;

        // Parse Ethereum address (0x prefixed hex string → 20 bytes)
        let eth_addr_bytes = if validator.address.starts_with("0x") && validator.address.len() == 42
        {
            match hex::decode(&validator.address[2..]) {
                Ok(bytes) if bytes.len() == 20 => bytes,
                _ => {
                    warn!(
                        "⚠️ [COMMITTEE] Invalid eth address for validator {}: {}",
                        idx, validator.address
                    );
                    vec![]
                }
            }
        } else {
            warn!(
                "⚠️ [COMMITTEE] Missing or invalid eth address for validator {}: {}",
                idx, validator.address
            );
            vec![]
        };
        eth_addresses.push(eth_addr_bytes);

        // Auth Key
        let authority_key_bytes = if validator.authority_key.starts_with("0x") {
            hex::decode(&validator.authority_key[2..])?
        } else {
            match STANDARD.decode(&validator.authority_key) {
                Ok(b) => b,
                Err(_) => hex::decode(&validator.authority_key)?,
            }
        };
        let authority_pubkey =
            bls12381::min_sig::BLS12381PublicKey::from_bytes(&authority_key_bytes)?;
        let authority_key = AuthorityPublicKey::new(authority_pubkey);

        // Protocol Key
        let protocol_key_bytes = if validator.protocol_key.starts_with("0x") {
            hex::decode(&validator.protocol_key[2..])?
        } else {
            match STANDARD.decode(&validator.protocol_key) {
                Ok(b) => b,
                Err(_) => hex::decode(&validator.protocol_key)?,
            }
        };
        let protocol_pubkey = ed25519::Ed25519PublicKey::from_bytes(&protocol_key_bytes)?;
        let protocol_key = ProtocolPublicKey::new(protocol_pubkey);

        // Network Key
        let network_key_bytes = if validator.network_key.starts_with("0x") {
            hex::decode(&validator.network_key[2..])?
        } else {
            match STANDARD.decode(&validator.network_key) {
                Ok(b) => b,
                Err(_) => hex::decode(&validator.network_key)?,
            }
        };
        let network_pubkey = ed25519::Ed25519PublicKey::from_bytes(&network_key_bytes)?;
        let network_key = NetworkPublicKey::new(network_pubkey);

        let hostname = if !validator.name.is_empty() {
            validator.name.clone()
        } else {
            format!("node-{}", idx)
        };

        authorities.push(Authority {
            stake,
            address,
            hostname,
            authority_key,
            protocol_key,
            network_key,
        });
    }

    info!(
        "📊 Built committee with {} authorities and {} eth_addresses for epoch {}",
        authorities.len(),
        eth_addresses.len(),
        epoch
    );
    Ok((Committee::new(epoch, authorities), eth_addresses))
}
