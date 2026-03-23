#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  RESET NODE — Stop + Clean data for a single node
#  Usage: ./reset_node.sh <node_id>
#  After reset, use ./resume_node.sh <node_id> to start fresh
# ═══════════════════════════════════════════════════════════════
set -e

NODE_ID="${1:?Usage: $0 <node_id> (0-4)}"
if [[ ! "$NODE_ID" =~ ^[0-4]$ ]]; then
    echo "❌ Invalid node_id: $NODE_ID (must be 0-4)"
    exit 1
fi

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

GO_DATA=("node0" "node1" "node2" "node3" "node4")
DATA="${GO_DATA[$NODE_ID]}"

echo ""
echo -e "${RED}═══════════════════════════════════════════════════${NC}"
echo -e "${RED}  🗑️  RESET Node $NODE_ID — Stop + Delete ALL data ${NC}"
echo -e "${RED}═══════════════════════════════════════════════════${NC}"
echo ""

# ─── Step 1: Stop node ───────────────────────────────────────
echo -e "${BLUE}[1/3] 🛑 Stopping node $NODE_ID...${NC}"
"$SCRIPT_DIR/stop_node.sh" "$NODE_ID" 2>/dev/null || true
sleep 3

# Force kill if still running
for sess in "go-master-$NODE_ID" "go-sub-$NODE_ID" "metanode-$NODE_ID"; do
    tmux kill-session -t "$sess" 2>/dev/null || true
done

# Clean sockets
rm -f "/tmp/executor${NODE_ID}.sock" "/tmp/rust-go-node${NODE_ID}-master.sock" "/tmp/metanode-tx-${NODE_ID}.sock" 2>/dev/null || true
echo -e "${GREEN}  ✅ Node $NODE_ID stopped${NC}"

# ─── Step 2: Delete Go data ─────────────────────────────────
echo -e "${BLUE}[2/3] 🗑️  Cleaning Go data ($DATA)...${NC}"
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data-write" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write" 2>/dev/null || true

# Recreate empty dirs
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
echo -e "${GREEN}  ✅ Go data cleaned${NC}"

# ─── Step 3: Delete Rust storage + logs ──────────────────────
echo -e "${BLUE}[3/3] 🗑️  Cleaning Rust storage + logs...${NC}"
rm -rf "$METANODE_ROOT/config/storage/node_$NODE_ID" 2>/dev/null || true
rm -rf "$LOG_DIR/node_$NODE_ID" 2>/dev/null || true
mkdir -p "$METANODE_ROOT/config/storage"
mkdir -p "$LOG_DIR/node_$NODE_ID"
echo -e "${GREEN}  ✅ Rust storage + logs cleaned${NC}"

# ─── Done ────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  ✅ Node $NODE_ID reset complete${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo -e "  Start fresh:  ${BLUE}./resume_node.sh $NODE_ID${NC}"
echo ""
