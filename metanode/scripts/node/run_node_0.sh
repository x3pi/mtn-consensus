#!/bin/bash
# Usage: ./run_node_0.sh
# Fresh start Node 0 (Master, Sub, and Rust) with clean data, keep keys

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

# Config Maps (Node 0 only)
id=0
GO_MASTER_CONFIG="config-master-node0.json"
GO_SUB_CONFIG="config-sub-node0.json"
GO_DATA_DIR="node0"
GO_MASTER_SESSION="go-master-0"
GO_SUB_SESSION="go-sub-0"
RUST_SESSION="metanode-0"
GO_MASTER_SOCKET="/tmp/rust-go-node0-master.sock"
RUST_CONFIG="config/node_0.toml"

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
echo -e "${GREEN}  🚀 FRESH START NODE 0 (Master, Sub, Rust) — keep keys, clean data${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# Step 1: Stop Node 0 processes
echo -e "${BLUE}📋 Step 1: Stop Node 0 processes...${NC}"
tmux kill-session -t "$GO_MASTER_SESSION" 2>/dev/null || true
tmux kill-session -t "$GO_SUB_SESSION" 2>/dev/null || true
tmux kill-session -t "$RUST_SESSION" 2>/dev/null || true
sleep 2

# Step 1.5: Build Rust and Go binaries
echo -e "${BLUE}📋 Step 1.5: Build Rust and Go binaries...${NC}"
echo "  🔄 Building Rust metanode..."
export PATH="/home/abc/protoc3/bin:$PATH"
cd "$METANODE_ROOT" && cargo +nightly build --release --bin metanode
echo "  🔄 Building Go simple_chain..."
cd "$GO_SIMPLE_ROOT" && go build -o simple_chain .
echo -e "${GREEN}  ✅ Binaries ready${NC}"

# Step 2: Clean Node 0 data
echo -e "${BLUE}📋 Step 2: Clean Node 0 data...${NC}"
DATA="$GO_DATA_DIR"
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data-write" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/snapshot_data_node0"* 2>/dev/null || true
rm -rf "$METANODE_ROOT/config/storage/node_0" 2>/dev/null || true
rm -rf "$LOG_DIR/node_0" 2>/dev/null || true

mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
mkdir -p "$LOG_DIR/node_0"
mkdir -p "$METANODE_ROOT/config/storage/node_0"

rm -f /tmp/executor0.sock /tmp/rust-go-node0-master.sock /tmp/metanode-tx-0.sock 2>/dev/null || true

echo -e "${GREEN}  ✅ Node 0 data cleaned${NC}"

# Step 4: Start Go Master 0
echo -e "${BLUE}📋 Step 3: Start Go Master 0...${NC}"
cd "$GO_SIMPLE_ROOT"
XAPIAN="sample/$DATA/data/data/xapian_node"
PPROF_ARG="--pprof-addr=localhost:6060"
tmux new-session -d -s "$GO_MASTER_SESSION" -c "$GO_SIMPLE_ROOT" \
    "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN' && ./simple_chain -config=$GO_MASTER_CONFIG $PPROF_ARG >> \"$LOG_DIR/node_0/go-master-stdout.log\" 2>&1"

wait_for_socket "$GO_MASTER_SOCKET" "Go Master 0" 120

# Step 5: Start Go Sub 0
echo -e "${BLUE}📋 Step 5: Start Go Sub 0...${NC}"
XAPIAN_SUB="sample/$DATA/data-write/data/xapian_node"
tmux new-session -d -s "$GO_SUB_SESSION" -c "$GO_SIMPLE_ROOT" \
    "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN_SUB' && ./simple_chain -config=$GO_SUB_CONFIG >> \"$LOG_DIR/node_0/go-sub-stdout.log\" 2>&1"

# Step 6: Start Rust Node 0
echo -e "${BLUE}📋 Step 6: Start Rust Node 0...${NC}"
cd "$METANODE_ROOT"
tmux new-session -d -s "$RUST_SESSION" -c "$METANODE_ROOT" \
    "ulimit -n 100000; export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; $BINARY start --config $RUST_CONFIG >> \"$LOG_DIR/node_0/rust.log\" 2>&1"

echo -e "${GREEN}  🎉 NODE 0 STARTED!${NC}"
