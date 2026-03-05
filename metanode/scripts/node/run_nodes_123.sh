#!/bin/bash
# Usage: ./run_nodes_123.sh
# Fresh start Nodes 1, 2, and 3 (Master, Sub, and Rust) with clean data

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

# Config Maps
GO_MASTER_CONFIG=("" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json")
GO_SUB_CONFIG=("" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json")
GO_DATA_DIR=("" "node1" "node2" "node3")
GO_MASTER_SESSION=("" "go-master-1" "go-master-2" "go-master-3")
GO_SUB_SESSION=("" "go-sub-1" "go-sub-2" "go-sub-3")
RUST_SESSION=("" "metanode-1" "metanode-2" "metanode-3")
GO_MASTER_SOCKET=("" "/tmp/rust-go-node1-master.sock" "/tmp/rust-go-node2-master.sock" "/tmp/rust-go-node3-master.sock")
RUST_CONFIG=("" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml")

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
echo -e "${GREEN}  🚀 FRESH START NODES 1, 2, 3 (Master, Sub, Rust)${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# Step 1: Stop Nodes 1, 2, 3 processes
echo -e "${BLUE}📋 Step 1: Stop Nodes 1, 2, 3 processes...${NC}"
for id in 1 2 3; do
    tmux kill-session -t "${GO_MASTER_SESSION[$id]}" 2>/dev/null || true
    tmux kill-session -t "${GO_SUB_SESSION[$id]}" 2>/dev/null || true
    tmux kill-session -t "${RUST_SESSION[$id]}" 2>/dev/null || true
done
sleep 2

# Step 1.5: Build Rust and Go binaries
echo -e "${BLUE}📋 Step 1.5: Build Rust and Go binaries...${NC}"
echo "  🔄 Building Rust metanode..."
export PATH="/home/abc/protoc3/bin:$PATH"
cd "$METANODE_ROOT" && cargo +nightly build --release --bin metanode
echo "  🔄 Building Go simple_chain..."
cd "$GO_SIMPLE_ROOT" && go build -o simple_chain .
echo -e "${GREEN}  ✅ Binaries ready${NC}"

# Step 2: Clean data for Nodes 1, 2, 3
echo -e "${BLUE}📋 Step 2: Clean data for Nodes 1, 2, 3...${NC}"
for id in 1 2 3; do
    DATA="${GO_DATA_DIR[$id]}"
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data-write" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write" 2>/dev/null || true
    rm -rf "$GO_SIMPLE_ROOT/snapshot_data_node$id"* 2>/dev/null || true
    rm -rf "$METANODE_ROOT/config/storage/node_$id" 2>/dev/null || true
    rm -rf "$LOG_DIR/node_$id" 2>/dev/null || true

    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
    mkdir -p "$LOG_DIR/node_$id"
    mkdir -p "$METANODE_ROOT/config/storage/node_$id"

    rm -f "/tmp/executor$id.sock" "/tmp/rust-go-node$id-master.sock" "/tmp/metanode-tx-$id.sock" 2>/dev/null || true
done

echo -e "${GREEN}  ✅ Nodes 1, 2, 3 data cleaned${NC}"

# Step 3: Start Go Masters 1, 2, 3
echo -e "${BLUE}📋 Step 3: Start Go Masters 1, 2, 3...${NC}"
cd "$GO_SIMPLE_ROOT"
for id in 1 2 3; do
    DATA="${GO_DATA_DIR[$id]}"
    XAPIAN="sample/$DATA/data/data/xapian_node"
    echo -e "${GREEN}  🚀 Starting Go Master $id...${NC}"
    tmux new-session -d -s "${GO_MASTER_SESSION[$id]}" -c "$GO_SIMPLE_ROOT" \
        "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN' && ./simple_chain -config=${GO_MASTER_CONFIG[$id]} >> \"$LOG_DIR/node_$id/go-master-stdout.log\" 2>&1"
    sleep 1
done

# Wait for sockets
for id in 1 2 3; do
    wait_for_socket "${GO_MASTER_SOCKET[$id]}" "Go Master $id" 120
done

# Step 4: Start Go Subs 1, 2, 3
echo -e "${BLUE}📋 Step 4: Start Go Subs 1, 2, 3...${NC}"
for id in 1 2 3; do
    DATA="${GO_DATA_DIR[$id]}"
    XAPIAN_SUB="sample/$DATA/data-write/data/xapian_node"
    echo -e "${GREEN}  🚀 Starting Go Sub $id...${NC}"
    tmux new-session -d -s "${GO_SUB_SESSION[$id]}" -c "$GO_SIMPLE_ROOT" \
        "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN_SUB' && ./simple_chain -config=${GO_SUB_CONFIG[$id]} >> \"$LOG_DIR/node_$id/go-sub-stdout.log\" 2>&1"
    sleep 1
done

# Step 5: Start Rust Nodes 1, 2, 3
echo -e "${BLUE}📋 Step 5: Start Rust Nodes 1, 2, 3...${NC}"
cd "$METANODE_ROOT"
for id in 1 2 3; do
    echo -e "${GREEN}  🚀 Starting Rust Node $id...${NC}"
    tmux new-session -d -s "${RUST_SESSION[$id]}" -c "$METANODE_ROOT" \
        "ulimit -n 100000; export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; $BINARY start --config ${RUST_CONFIG[$id]} >> \"$LOG_DIR/node_$id/rust.log\" 2>&1"
    sleep 1
done

echo -e "${GREEN}  🎉 NODES 1, 2, 3 STARTED!${NC}"
