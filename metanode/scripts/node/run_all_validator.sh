#!/bin/bash
# Usage: ./run_all_validator.sh
# Fresh start VALIDATOR nodes only (0-3), NO SyncOnly node 4
# - Cleans all Go/Rust data (keeps keys/configs)
# - Starts Go Masters → Go Subs → Rust Nodes in correct order

set -e
set -o pipefail
ulimit -n 100000 || true

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"
LOG_DIR="$METANODE_ROOT/logs"
BINARY="$METANODE_ROOT/target/release/metanode"

# Only validator nodes 0-3 (NO node 4 SyncOnly)
NODES=(0 1 2 3)

# Config Maps
GO_MASTER_CONFIG=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json")
GO_SUB_CONFIG=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json")
GO_DATA_DIR=("node0" "node1" "node2" "node3")
GO_MASTER_SESSION=("go-master-0" "go-master-1" "go-master-2" "go-master-3")
GO_SUB_SESSION=("go-sub-0" "go-sub-1" "go-sub-2" "go-sub-3")
RUST_SESSION=("metanode-0" "metanode-1" "metanode-2" "metanode-3")
GO_MASTER_SOCKET=("/tmp/rust-go-node0-master.sock" "/tmp/rust-go-node1-master.sock" "/tmp/rust-go-node2-master.sock" "/tmp/rust-go-node3-master.sock")
RUST_CONFIG=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml")

wait_for_socket() {
    local socket=$1 name=$2 timeout=${3:-120}
    local start=$(date +%s)
    while true; do
        if [ -S "$socket" ]; then
            echo -e "${GREEN}  ✅ $name ready ($(( $(date +%s) - start ))s)${NC}"
            return 0
        fi
        if [ $(( $(date +%s) - start )) -ge $timeout ]; then
            echo -e "${YELLOW}  ⚠️ Timeout waiting for $name (${timeout}s)${NC}"
            return 1
        fi
        sleep 1
    done
}

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🚀 FRESH START VALIDATORS (0-3) — NO SyncOnly node 4${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# ==============================================================================
# Step 1: Stop everything
# ==============================================================================
echo -e "${BLUE}📋 Step 1: Stop all processes...${NC}"
"$SCRIPT_DIR/stop_all.sh"
sleep 2

# ==============================================================================
# Step 2: Build binaries
# ==============================================================================
echo -e "${BLUE}📋 Step 2: Build Rust and Go binaries...${NC}"
echo "  🔄 Building Rust metanode..."
export PATH="/home/abc/protoc3/bin:$PATH"
cd "$METANODE_ROOT" && cargo +nightly build --release --bin metanode
echo "  🔄 Building Go simple_chain..."
cd "$GO_SIMPLE_ROOT" && go build -o simple_chain .
echo -e "${GREEN}  ✅ Binaries ready${NC}"

# ==============================================================================
# Step 3: Clean ALL data (keep keys/configs)
# ==============================================================================
echo -e "${BLUE}📋 Step 3: Clean all data...${NC}"

for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    DATA="${GO_DATA_DIR[$i]}"
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data-write" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write" 2>/dev/null || true
done

rm -rf "$GO_SIMPLE_ROOT/snapshot_data"* 2>/dev/null || true
rm -rf "$METANODE_ROOT/config/storage" 2>/dev/null || true
rm -rf "$LOG_DIR" 2>/dev/null || true

for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    DATA="${GO_DATA_DIR[$i]}"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
    mkdir -p "$LOG_DIR/node_$id"
done
mkdir -p "$METANODE_ROOT/config/storage"

rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true
rm -f /tmp/epoch_data_backup.json /tmp/epoch_data_backup_*.json 2>/dev/null || true

echo -e "${GREEN}  ✅ All data cleaned, keys/configs preserved${NC}"

# ==============================================================================
# Step 5: Start Go Masters
# ==============================================================================
for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    DATA="${GO_DATA_DIR[$i]}"
    XAPIAN="sample/$DATA/data/data/xapian_node"

    echo -e "${GREEN}  🚀 Starting Go Master $id (${GO_MASTER_SESSION[$i]})...${NC}"
    PPROF_ARG=""
    if [ "$id" -eq "0" ]; then
        PPROF_ARG="--pprof-addr=localhost:6060"
    fi
    tmux new-session -d -s "${GO_MASTER_SESSION[$i]}" -c "$GO_SIMPLE_ROOT" \
        "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOGC=400 && export XAPIAN_BASE_PATH='$XAPIAN' && ./simple_chain -config=${GO_MASTER_CONFIG[$i]} $PPROF_ARG >> \"$LOG_DIR/node_$id/go-master-stdout.log\" 2>&1"

    sleep 2
done

# Wait for Go Master sockets
echo -e "${BLUE}📋 Step 6: Wait for Go Master sockets...${NC}"
for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    wait_for_socket "${GO_MASTER_SOCKET[$i]}" "Go Master $id" 120
done

# ==============================================================================
# Step 7: Start Go Subs
# ==============================================================================
echo -e "${BLUE}📋 Step 7: Start all Go Subs...${NC}"
cd "$GO_SIMPLE_ROOT"

for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    DATA="${GO_DATA_DIR[$i]}"
    XAPIAN="sample/$DATA/data-write/data/xapian_node"

    echo -e "${GREEN}  🚀 Starting Go Sub $id (${GO_SUB_SESSION[$i]})...${NC}"
    tmux new-session -d -s "${GO_SUB_SESSION[$i]}" -c "$GO_SIMPLE_ROOT" \
        "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOGC=400 && export XAPIAN_BASE_PATH='$XAPIAN' && ./simple_chain -config=${GO_SUB_CONFIG[$i]} >> \"$LOG_DIR/node_$id/go-sub-stdout.log\" 2>&1"

    sleep 1
done

echo "  ⏳ Waiting 5s for Go Subs to initialize..."
sleep 5

# ==============================================================================
# Step 8: Start Rust Metanodes
# ==============================================================================
echo -e "${BLUE}📋 Step 8: Start all Rust Metanodes...${NC}"
cd "$METANODE_ROOT"

for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    echo -e "${GREEN}  🚀 Starting Rust Node $id (${RUST_SESSION[$i]})...${NC}"
    tmux new-session -d -s "${RUST_SESSION[$i]}" -c "$METANODE_ROOT" \
        "ulimit -n 100000; export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; $BINARY start --config ${RUST_CONFIG[$i]} >> \"$LOG_DIR/node_$id/rust.log\" 2>&1"

    sleep 1
done

echo "  ⏳ Waiting 5s for Rust nodes to start..."
sleep 5

# ==============================================================================
# Summary
# ==============================================================================
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🎉 ALL VALIDATORS STARTED (0-3)! No SyncOnly node.${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""
for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    echo -e "${GREEN}  Node $id:${NC} tmux attach -t metanode-$id | go-master-$id | go-sub-$id"
done
echo ""
echo -e "${GREEN}  📁 Logs: $LOG_DIR/node_N/${NC}"
echo -e "${GREEN}  🔍 Check: tmux ls${NC}"
echo ""
