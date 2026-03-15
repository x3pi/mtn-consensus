#!/bin/bash
# Usage: ./resume_node_ai.sh <node_id>
# Resume a single node keeping all data (AI-friendly, uses nohup instead of tmux)

set -e
set -o pipefail

NODE_ID="${1:?Usage: $0 <node_id> (0-4)}"

if [[ ! "$NODE_ID" =~ ^[0-4]$ ]]; then
    echo "❌ Invalid node_id: $NODE_ID (must be 0-4)"
    exit 1
fi

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"
LOG_DIR="$METANODE_ROOT/logs"
BINARY="$METANODE_ROOT/target/release/metanode"

# Configs
GO_MASTER_CONFIG="config-master-node${NODE_ID}.json"
GO_SUB_CONFIG="config-sub-node${NODE_ID}.json"
RUST_CONFIG="config/node_${NODE_ID}.toml"
DATA="node${NODE_ID}"

GO_MASTER_SOCKET="/tmp/rust-go-node${NODE_ID}-master.sock"

# Helper
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
echo -e "${GREEN}  🔄 RESUME Node $NODE_ID (keep data, AI nohup mode)${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""

echo -e "${BLUE}📋 Step 1: Stop node $NODE_ID if running...${NC}"
"$SCRIPT_DIR/stop_node_ai.sh" "$NODE_ID" 2>/dev/null || true
sleep 2

echo -e "${BLUE}📋 Step 2: Verify configs...${NC}"
if [ ! -f "$BINARY" ] || [ -n "$(find "$METANODE_ROOT/src" -name '*.rs' -newer "$BINARY" 2>/dev/null | head -1)" ]; then
    echo "  🔄 Building Rust Metanode..."
    cd "$METANODE_ROOT" && cargo build --release --bin metanode
    echo -e "${GREEN}  ✅ Binary rebuilt${NC}"
fi

mkdir -p "$LOG_DIR/node_$NODE_ID"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"

echo -e "${BLUE}📋 Step 3: Start Go Master...${NC}"
cd "$GO_SIMPLE_ROOT"
XAPIAN_MASTER="sample/$DATA/data/data/xapian_node"
export GOTOOLCHAIN=go1.23.5
export GOMEMLIMIT=4GiB
export XAPIAN_BASE_PATH="$XAPIAN_MASTER"
nohup ./simple_chain -config="$GO_MASTER_CONFIG" > "$LOG_DIR/node_$NODE_ID/go-master-stdout.log" 2>&1 &
echo -e "${GREEN}  🚀 Go Master started (nohup)${NC}"

echo -e "${BLUE}📋 Step 4: Start Go Sub...${NC}"
XAPIAN_SUB="sample/$DATA/data-write/data/xapian_node"
export XAPIAN_BASE_PATH="$XAPIAN_SUB"
nohup ./simple_chain -config="$GO_SUB_CONFIG" > "$LOG_DIR/node_$NODE_ID/go-sub-stdout.log" 2>&1 &
echo -e "${GREEN}  🚀 Go Sub started (nohup)${NC}"

echo -e "${BLUE}📋 Step 5: Waiting for Go Master socket...${NC}"
wait_for_socket "$GO_MASTER_SOCKET" "Go Master $NODE_ID" 120

echo -e "${BLUE}📋 Step 6: Start Rust Metanode...${NC}"
cd "$METANODE_ROOT"
export RUST_LOG=info,consensus_core=debug
export DB_WRITE_BUFFER_SIZE_MB=256
export DB_WAL_SIZE_MB=256
nohup $BINARY start --config "$RUST_CONFIG" > "$LOG_DIR/node_$NODE_ID/rust.log" 2>&1 &
echo -e "${GREEN}  🚀 Rust Metanode started (nohup)${NC}"

sleep 3
echo ""
echo -e "${GREEN}✅ Node $NODE_ID RESUMED${NC}"
echo -e "  📁 Logs: $LOG_DIR/node_$NODE_ID/"
echo ""
