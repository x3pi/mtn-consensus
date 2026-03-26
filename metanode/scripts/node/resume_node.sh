#!/bin/bash
# Usage: ./resume_node.sh <node_id>
# Resume a single node keeping all data (Go Master + Go Sub + Rust Metanode)

set -e
set -o pipefail

NODE_ID="${1:?Usage: $0 <node_id> (0-4)}"

if [[ ! "$NODE_ID" =~ ^[0-4]$ ]]; then
    echo "❌ Invalid node_id: $NODE_ID (must be 0-4)"
    exit 1
fi

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

# ─── Config Maps ─────────────────────────────────────────────
GO_MASTER_CONFIG=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json" "config-master-node4.json")
GO_SUB_CONFIG=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json" "config-sub-node4.json")
GO_DATA_DIR=("node0" "node1" "node2" "node3" "node4")

GO_MASTER_SESSION=("go-master-0" "go-master-1" "go-master-2" "go-master-3" "go-master-4")
GO_SUB_SESSION=("go-sub-0" "go-sub-1" "go-sub-2" "go-sub-3" "go-sub-4")
RUST_SESSION=("metanode-0" "metanode-1" "metanode-2" "metanode-3" "metanode-4")

GO_MASTER_SOCKET=("/tmp/rust-go-node0-master.sock" "/tmp/rust-go-node1-master.sock" "/tmp/rust-go-node2-master.sock" "/tmp/rust-go-node3-master.sock" "/tmp/rust-go-node4-master.sock")
EXECUTOR_SOCKET=("/tmp/executor0.sock" "/tmp/executor1.sock" "/tmp/executor2.sock" "/tmp/executor3.sock" "/tmp/executor4.sock")

RUST_CONFIG=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml" "config/node_4.toml")

# ─── Helper ──────────────────────────────────────────────────
wait_for_socket() {
    local socket=$1
    local name=$2
    local timeout=${3:-120}
    local start=$(date +%s)
    while true; do
        if [ -S "$socket" ]; then
            local elapsed=$(( $(date +%s) - start ))
            echo -e "${GREEN}  ✅ $name ready (${elapsed}s)${NC}"
            return 0
        fi
        local elapsed=$(( $(date +%s) - start ))
        if [ $elapsed -ge $timeout ]; then
            echo -e "${YELLOW}  ⚠️ Timeout waiting for $name (${timeout}s)${NC}"
            return 1
        fi
        sleep 1
    done
}

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🔄 RESUME Node $NODE_ID (keep data)${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""

# ─── Step 1: Stop node if running ────────────────────────────
echo -e "${BLUE}📋 Step 1: Stop node $NODE_ID if running...${NC}"
"$SCRIPT_DIR/stop_node.sh" "$NODE_ID" 2>/dev/null || true
sleep 2

# ─── Step 2: Verify configs ─────────────────────────────────
echo -e "${BLUE}📋 Step 2: Verify configs...${NC}"

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

RUST_CFG="$METANODE_ROOT/${RUST_CONFIG[$NODE_ID]}"
GO_M_CFG="$GO_SIMPLE_ROOT/${GO_MASTER_CONFIG[$NODE_ID]}"
GO_S_CFG="$GO_SIMPLE_ROOT/${GO_SUB_CONFIG[$NODE_ID]}"

for f in "$RUST_CFG" "$GO_M_CFG" "$GO_S_CFG"; do
    if [ ! -f "$f" ]; then
        echo "❌ Config not found: $f"
        exit 1
    fi
done
echo -e "${GREEN}  ✅ All configs exist${NC}"

# ─── Step 3: Ensure log/data directories ─────────────────────
mkdir -p "$LOG_DIR/node_$NODE_ID"

DATA="${GO_DATA_DIR[$NODE_ID]}"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"

# ─── Step 4: Start Go Master ────────────────────────────────
echo -e "${BLUE}📋 Step 3: Start Go Master...${NC}"
cd "$GO_SIMPLE_ROOT"

XAPIAN_MASTER="sample/$DATA/data/data/xapian_node"
tmux new-session -d -s "${GO_MASTER_SESSION[$NODE_ID]}" -c "$GO_SIMPLE_ROOT" \
    "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN_MASTER' && export METANODE_STATE_SOCK='/tmp/metanode-state-'$NODE_ID'.sock' && export METANODE_JMT_STATE_PATH='$METANODE_ROOT/config/storage/node_'$NODE_ID'/jmt_state' && ./simple_chain -config=${GO_MASTER_CONFIG[$NODE_ID]} >> \"$LOG_DIR/node_$NODE_ID/go-master-stdout.log\" 2>&1"

echo -e "${GREEN}  🚀 Go Master started (${GO_MASTER_SESSION[$NODE_ID]})${NC}"

# ─── Step 5: Start Go Sub ───────────────────────────────────
echo -e "${BLUE}📋 Step 4: Start Go Sub...${NC}"

XAPIAN_SUB="sample/$DATA/data-write/data/xapian_node"
tmux new-session -d -s "${GO_SUB_SESSION[$NODE_ID]}" -c "$GO_SIMPLE_ROOT" \
    "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN_SUB' && export METANODE_STATE_SOCK='/tmp/metanode-state-'$NODE_ID'.sock' && export METANODE_JMT_STATE_PATH='$METANODE_ROOT/config/storage/node_'$NODE_ID'/jmt_state' && ./simple_chain -config=${GO_SUB_CONFIG[$NODE_ID]} >> \"$LOG_DIR/node_$NODE_ID/go-sub-stdout.log\" 2>&1"

echo -e "${GREEN}  🚀 Go Sub started (${GO_SUB_SESSION[$NODE_ID]})${NC}"

# ─── Step 6: Wait for Go Master socket ──────────────────────
echo -e "${BLUE}📋 Step 5: Waiting for Go Master socket...${NC}"
wait_for_socket "${GO_MASTER_SOCKET[$NODE_ID]}" "Go Master $NODE_ID" 120

# ─── Step 7: Start Rust Metanode ─────────────────────────────
echo -e "${BLUE}📋 Step 6: Start Rust Metanode...${NC}"
cd "$METANODE_ROOT"

tmux new-session -d -s "${RUST_SESSION[$NODE_ID]}" -c "$METANODE_ROOT" \
    "export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; $BINARY start --config ${RUST_CONFIG[$NODE_ID]} >> \"$LOG_DIR/node_$NODE_ID/rust.log\" 2>&1"

echo -e "${GREEN}  🚀 Rust Metanode started (${RUST_SESSION[$NODE_ID]})${NC}"

sleep 3

# ─── Summary ─────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  ✅ Node $NODE_ID RESUMED${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo -e "${GREEN}  📊 tmux sessions:${NC}"
echo "    Go Master: tmux attach -t ${GO_MASTER_SESSION[$NODE_ID]}"
echo "    Go Sub:    tmux attach -t ${GO_SUB_SESSION[$NODE_ID]}"
echo "    Rust:      tmux attach -t ${RUST_SESSION[$NODE_ID]}"
echo ""
echo -e "${GREEN}  📁 Logs: $LOG_DIR/node_$NODE_ID/${NC}"
echo ""
