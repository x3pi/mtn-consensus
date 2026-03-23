#!/bin/bash
# Snapshot Restore via HTTP (AI-friendly, no tmux)
# Usage: ./test_snapshot_restore_ai.sh [SRC_NODE] [DST_NODE] [SRC_IP]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SIMPLE_CHAIN_DIR="$(cd "$SCRIPT_DIR/../../../../mtn-simple-2025/cmd/simple_chain" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOG_DIR="$METANODE_ROOT/logs"

SRC_NODE=${1:-0}
DST_NODE=${2:-2}
SRC_IP=${3:-127.0.0.1}

SNAPSHOT_PORT=$((8700 + SRC_NODE))
SNAPSHOT_URL="http://${SRC_IP}:${SNAPSHOT_PORT}"
LEVELDB_DIRS="account_state blocks receipts transaction_state mapping smart_contract_code smart_contract_storage stake_db trie_database backup_device_key_storage xapian xapian_node"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

echo -e "${BOLD}╔═══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  📸 SNAPSHOT RESTORE (AI): Node $SRC_NODE → Node $DST_NODE                ║${NC}"
echo -e "${BOLD}╚═══════════════════════════════════════════════════════════╝${NC}"

# Query snapshots
echo -e "${YELLOW}📡 Step 1: Querying snapshots...${NC}"
SNAPSHOTS_JSON=$(curl -sf "${SNAPSHOT_URL}/api/snapshots" 2>/dev/null || echo "null")

if [ "$SNAPSHOTS_JSON" = "null" ] || [ -z "$SNAPSHOTS_JSON" ]; then
    echo -e "${RED}  ❌ No snapshots available on Node $SRC_NODE${NC}"
    exit 1
fi
SNAPSHOT_COUNT=$(echo "$SNAPSHOTS_JSON" | jq 'length')
if [ "$SNAPSHOT_COUNT" -eq 0 ] 2>/dev/null; then
    echo -e "${RED}  ❌ Snapshot list is empty.${NC}"
    exit 1
fi

LATEST=$(echo "$SNAPSHOTS_JSON" | jq -r '.[-1]')
SNAP_NAME=$(echo "$LATEST" | jq -r '.snapshot_name')
echo -e "${GREEN}  ✅ Found $SNAPSHOT_COUNT snapshot(s). Using latest: $SNAP_NAME${NC}"

DOWNLOAD_URL="${SNAPSHOT_URL}/files/${SNAP_NAME}/"

# Stop Node
echo -e "${YELLOW}🛑 Step 2: Stopping Node $DST_NODE...${NC}"
cd "$SCRIPT_DIR"
./stop_node_ai.sh "$DST_NODE" 2>/dev/null || true
echo -e "${GREEN}  ✅ Node $DST_NODE stopped${NC}"

# Wipe Data
echo -e "${YELLOW}🗑️  Step 3: Wiping Node $DST_NODE data...${NC}"
DST="$SIMPLE_CHAIN_DIR/sample/node$DST_NODE"
rm -rf "$DST/data" "$DST/data-write" "$DST/back_up" "$DST/back_up_write"
rm -rf "$LOG_DIR/node_$DST_NODE" "$METANODE_ROOT/config/storage/node_$DST_NODE"
echo -e "${GREEN}  ✅ Wiped${NC}"

# Download
echo -e "${YELLOW}📥 Step 4: Downloading snapshot (silently in background)...${NC}"
DOWNLOAD_DIR="/tmp/snapshot_download_node${DST_NODE}"
rm -rf "$DOWNLOAD_DIR"
mkdir -p "$DOWNLOAD_DIR"
wget -q -c -r -np -nH --cut-dirs=2 -P "$DOWNLOAD_DIR" --reject="index.html*" "$DOWNLOAD_URL"
echo -e "${GREEN}  ✅ Download complete${NC}"

# Restore
echo -e "${YELLOW}📂 Step 5: Restoring data...${NC}"
mkdir -p "$DST/data/data" "$DST/back_up" "$DST/data-write" "$DST/back_up_write"
for dir_name in $LEVELDB_DIRS; do
    if [ -d "$DOWNLOAD_DIR/$dir_name" ]; then
        mv "$DOWNLOAD_DIR/$dir_name" "$DST/data/data/$dir_name"
    fi
done

if [ -d "$DOWNLOAD_DIR/back_up" ]; then
    cp -r "$DOWNLOAD_DIR/back_up/"* "$DST/back_up/" 2>/dev/null || true
    rm -rf "$DOWNLOAD_DIR/back_up"
fi
if [ -d "$DOWNLOAD_DIR/data-write" ]; then
    cp -r "$DOWNLOAD_DIR/data-write/"* "$DST/data-write/" 2>/dev/null || true
    rm -rf "$DOWNLOAD_DIR/data-write"
fi
if [ -d "$DOWNLOAD_DIR/back_up_write" ]; then
    cp -r "$DOWNLOAD_DIR/back_up_write/"* "$DST/back_up_write/" 2>/dev/null || true
    rm -rf "$DOWNLOAD_DIR/back_up_write"
fi
find "$DST" -name "LOCK" -delete 2>/dev/null || true
mkdir -p "$LOG_DIR/node_$DST_NODE" "$METANODE_ROOT/config/storage/node_$DST_NODE"
rm -rf "$DOWNLOAD_DIR"
echo -e "${GREEN}  ✅ Data restored${NC}"

# Restart
echo -e "${YELLOW}🚀 Step 6: Restarting Node $DST_NODE...${NC}"
./resume_node_ai.sh "$DST_NODE"

# Monitor
echo -e "${YELLOW}⏳ Step 7: Monitoring block sync (30s)...${NC}"
for t in 10 20 30; do
    sleep 10
    BLOCK=$(grep 'last_committed_block' "$LOG_DIR/node_$DST_NODE/go-master-stdout.log" 2>/dev/null | tail -n 1 | grep -oP 'last_committed_block=\d+' || echo "N/A")
    echo "  ⏱️ +${t}s: $BLOCK"
done
echo -e "${GREEN}✅ SNAPSHOT RESTORE COMPLETE${NC}"
