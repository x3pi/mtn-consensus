// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

mod consensus;
mod network;
mod types;
mod config;
mod node;

use config::NodeConfig;
use node::startup::{StartupConfig, InitializedNode};

#[derive(Parser)]
#[command(name = "metanode")]
#[command(about = "MetaNode Consensus Engine - Multi-node consensus based on Sui Mysticeti")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a consensus node
    Start {
        /// Path to node configuration file
        #[arg(short, long, default_value = "config/node.toml")]
        config: PathBuf,
    },
    /// Generate node configuration files for multiple nodes
    Generate {
        /// Number of nodes to generate
        #[arg(short, long, default_value = "4")]
        nodes: usize,
        /// Output directory for config files
        #[arg(short, long, default_value = "config")]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metanode=info,consensus_core=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config } => {
            info!("Starting MetaNode Consensus Engine...");
            info!("Loading configuration from: {:?}", config);

            let node_config = NodeConfig::load(&config)?;
            info!("Node ID: {}", node_config.node_id);
            info!("Network address: {}", node_config.network_address);

            // Create registry for metrics
            let registry = prometheus::Registry::new();

            // Initialize and start the node
            let startup_config = StartupConfig::new(node_config, registry, None);
            let initialized_node = InitializedNode::initialize(startup_config).await?;

            // Run the main event loop
            initialized_node.run_main_loop().await?;
        }
        Commands::Generate { nodes, output } => {
            info!("Generating configuration for {} nodes...", nodes);
            NodeConfig::generate_multiple(nodes, &output).await?;
            info!("Configuration files generated in: {:?}", output);
        }
    }

    Ok(())
}