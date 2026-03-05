#!/bin/bash
# Usage: ./run_node.sh <node_id>
# Fresh start a single node (clean data, keep keys/config)
# Resets genesis timestamp, cleans Go/Rust data, then starts all processes.

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
RED='\033[0;31m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"
LOG_DIR="$METANODE_ROOT/logs"

# ─── Config Maps ─────────────────────────────────────────────
GO_DATA_DIR=("node0" "node1" "node2" "node3" "node4")

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🚀 FRESH START Node $NODE_ID (clean data, keep keys)${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""

# ─── Step 1: Stop node ───────────────────────────────────────
echo -e "${BLUE}📋 Step 1: Stop node $NODE_ID...${NC}"
"$SCRIPT_DIR/stop_node.sh" "$NODE_ID" 2>/dev/null || true
sleep 2

# ─── Step 2: Clean data (keep config/keys) ───────────────────
echo -e "${BLUE}📋 Step 2: Clean data for node $NODE_ID...${NC}"

DATA="${GO_DATA_DIR[$NODE_ID]}"

# Clean Go data
echo -e "${YELLOW}  🗑️  Cleaning Go data (sample/$DATA)...${NC}"
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data-write" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up" 2>/dev/null || true
rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write" 2>/dev/null || true

# Clean Rust storage
echo -e "${YELLOW}  🗑️  Cleaning Rust storage (node_$NODE_ID)...${NC}"
rm -rf "$METANODE_ROOT/config/storage/node_$NODE_ID" 2>/dev/null || true

# Clean logs
echo -e "${YELLOW}  🗑️  Cleaning logs (node_$NODE_ID)...${NC}"
rm -rf "$LOG_DIR/node_$NODE_ID" 2>/dev/null || true

# Recreate directories
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
mkdir -p "$METANODE_ROOT/config/storage/node_$NODE_ID"
mkdir -p "$LOG_DIR/node_$NODE_ID"

echo -e "${GREEN}  ✅ Data cleaned${NC}"

# ─── Step 3: Start via resume_node.sh ────────────────────────
echo -e "${BLUE}📋 Step 3: Starting node $NODE_ID...${NC}"
"$SCRIPT_DIR/resume_node.sh" "$NODE_ID"
