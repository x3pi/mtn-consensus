#!/bin/bash
# Usage: ./resume_all.sh
# Resume ALL nodes (0-4) keeping data
# - Stops old processes gracefully 
# - Starts Go Masters → waits for sockets → Go Subs → Rust Nodes

set -e
set -o pipefail

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"
LOG_DIR="$METANODE_ROOT/logs"
BINARY="$METANODE_ROOT/target/release/metanode"

# Config Maps
GO_MASTER_CONFIG=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json" "config-master-node4.json")
GO_SUB_CONFIG=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json" "config-sub-node4.json")
GO_DATA_DIR=("node0" "node1" "node2" "node3" "node4")
GO_MASTER_SESSION=("go-master-0" "go-master-1" "go-master-2" "go-master-3" "go-master-4")
GO_SUB_SESSION=("go-sub-0" "go-sub-1" "go-sub-2" "go-sub-3" "go-sub-4")
RUST_SESSION=("metanode-0" "metanode-1" "metanode-2" "metanode-3" "metanode-4")
GO_MASTER_SOCKET=("/tmp/rust-go-node0-master.sock" "/tmp/rust-go-node1-master.sock" "/tmp/rust-go-node2-master.sock" "/tmp/rust-go-node3-master.sock" "/tmp/rust-go-node4-master.sock")
RUST_CONFIG=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml" "config/node_4.toml")

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
echo -e "${GREEN}  🔄 RESUME ALL NODES (0-4) — keep data${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# ==============================================================================
# Step 1: Stop everything
# ==============================================================================
echo -e "${BLUE}📋 Step 1: Stop all processes...${NC}"
"$SCRIPT_DIR/stop_all.sh"
sleep 2

# ==============================================================================
# Step 2: Check binary + configs
# ==============================================================================
echo -e "${BLUE}📋 Step 2: Check binary and configs...${NC}"
# Always rebuild if source changed (or binary missing)
NEEDS_BUILD=false
if [ ! -f "$BINARY" ]; then
    echo "  ⚠️ Binary not found, building..."
    NEEDS_BUILD=true
elif [ -n "$(find "$METANODE_ROOT/src" -name '*.rs' -newer "$BINARY" 2>/dev/null | head -1)" ]; then
    echo "  🔄 Source changed, rebuilding..."
    NEEDS_BUILD=true
fi
if [ "$NEEDS_BUILD" = true ]; then
    cd "$METANODE_ROOT" && cargo build --release --bin metanode
    echo -e "${GREEN}  ✅ Binary rebuilt${NC}"
fi

# Ensure log/data directories exist
for id in 0 1 2 3 4; do
    DATA="${GO_DATA_DIR[$id]}"
    mkdir -p "$LOG_DIR/node_$id"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
    mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
done
echo -e "${GREEN}  ✅ Binary and configs ready${NC}"

# ==============================================================================
# Step 3: Start ALL Go Masters
# ==============================================================================
echo -e "${BLUE}📋 Step 3: Start all Go Masters...${NC}"
cd "$GO_SIMPLE_ROOT"

for id in 0 1 2 3 4; do
    DATA="${GO_DATA_DIR[$id]}"
    XAPIAN="sample/$DATA/data/data/xapian_node"
    
    echo -e "${GREEN}  🚀 Go Master $id (${GO_MASTER_SESSION[$id]})${NC}"
    tmux new-session -d -s "${GO_MASTER_SESSION[$id]}" -c "$GO_SIMPLE_ROOT" \
        "export GOTOOLCHAIN=go1.23.5 && export XAPIAN_BASE_PATH='$XAPIAN' && go run . -config=${GO_MASTER_CONFIG[$id]} >> \"$LOG_DIR/node_$id/go-master-stdout.log\" 2>&1"
    
    sleep 2
done

# Wait for ALL Go Master sockets
echo -e "${BLUE}📋 Step 4: Wait for Go Master sockets...${NC}"
for id in 0 1 2 3 4; do
    wait_for_socket "${GO_MASTER_SOCKET[$id]}" "Go Master $id" 120
done

# ==============================================================================
# Step 5: Start ALL Go Subs
# ==============================================================================
echo -e "${BLUE}📋 Step 5: Start all Go Subs...${NC}"
cd "$GO_SIMPLE_ROOT"

for id in 0 1 2 3 4; do
    DATA="${GO_DATA_DIR[$id]}"
    XAPIAN="sample/$DATA/data-write/data/xapian_node"
    
    echo -e "${GREEN}  🚀 Go Sub $id (${GO_SUB_SESSION[$id]})${NC}"
    tmux new-session -d -s "${GO_SUB_SESSION[$id]}" -c "$GO_SIMPLE_ROOT" \
        "export GOTOOLCHAIN=go1.23.5 && export XAPIAN_BASE_PATH='$XAPIAN' && go run . -config=${GO_SUB_CONFIG[$id]} >> \"$LOG_DIR/node_$id/go-sub-stdout.log\" 2>&1"
    
    sleep 1
done

echo "  ⏳ Waiting 5s for Go Subs..."
sleep 5

# ==============================================================================
# Step 6: Start ALL Rust Metanodes
# ==============================================================================
echo -e "${BLUE}📋 Step 6: Start all Rust Metanodes...${NC}"
cd "$METANODE_ROOT"

for id in 0 1 2 3 4; do
    echo -e "${GREEN}  🚀 Rust Node $id (${RUST_SESSION[$id]})${NC}"
    tmux new-session -d -s "${RUST_SESSION[$id]}" -c "$METANODE_ROOT" \
        "export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; $BINARY start --config ${RUST_CONFIG[$id]} >> \"$LOG_DIR/node_$id/rust.log\" 2>&1"
    
    sleep 1
done

echo "  ⏳ Waiting 5s for Rust nodes..."
sleep 5

# ==============================================================================
# Summary
# ==============================================================================
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🎉 ALL NODES RESUMED (0-4)!${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""
for id in 0 1 2 3 4; do
    echo -e "${GREEN}  Node $id:${NC} tmux attach -t metanode-$id | go-master-$id | go-sub-$id"
done
echo ""
echo -e "${GREEN}  📁 Logs: $LOG_DIR/node_N/${NC}"
echo -e "${GREEN}  🔍 Check: tmux ls${NC}"
echo ""
