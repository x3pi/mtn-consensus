#!/bin/bash

# ═══════════════════════════════════════════════════════════════
# Snapshot Restore via HTTP
# ═══════════════════════════════════════════════════════════════
# Downloads snapshot from source node's HTTP server and restores
# to destination node. Source node stays running — no downtime!
#
# Usage: ./test_snapshot_restore.sh [SRC_NODE] [DST_NODE] [SRC_IP]
#   SRC_NODE  — source node index (default: 0)
#   DST_NODE  — destination node index (default: 2)
#   SRC_IP    — source node IP (default: 127.0.0.1 for local test)
#
# Snapshot server ports: 8700 + node_index
#   Node 0 → :8700, Node 1 → :8701, Node 2 → :8702, ...
#
# Snapshot structure (flat):
#   account_state/, blocks/, mapping/, receipts/, ...  → LevelDB dirs
#   back_up/          → PebbleDB (backup_db shards)
#   data-write/       → Sub node LevelDB
#   back_up_write/    → Sub node PebbleDB
#   metadata.json     → Snapshot metadata
#
# Go Master directory layout:
#   sample/nodeX/data/data/       → LevelDB dirs go here
#   sample/nodeX/back_up/         → PebbleDB goes here
#   sample/nodeX/data-write/      → Sub LevelDB goes here
#   sample/nodeX/back_up_write/   → Sub PebbleDB goes here
# ═══════════════════════════════════════════════════════════════

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SIMPLE_CHAIN_DIR="/home/abc/chain-n/mtn-simple-2025/cmd/simple_chain"
METANODE_ROOT="/home/abc/chain-n/mtn-consensus/metanode"
LOG_DIR="$METANODE_ROOT/logs"

SRC_NODE=${1:-0}
DST_NODE=${2:-2}
SRC_IP=${3:-127.0.0.1}

SNAPSHOT_PORT=$((8700 + SRC_NODE))
SNAPSHOT_URL="http://${SRC_IP}:${SNAPSHOT_PORT}"

# LevelDB dirs that go into data/data/
LEVELDB_DIRS="account_state blocks receipts transaction_state mapping smart_contract_code smart_contract_storage stake_db trie_database backup_device_key_storage xapian xapian_node"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

echo ""
echo -e "${BOLD}╔═══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  📸 SNAPSHOT RESTORE (HTTP): Node $SRC_NODE → Node $DST_NODE              ║${NC}"
echo -e "${BOLD}╚═══════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  📡 Source:  ${BOLD}${SNAPSHOT_URL}${NC}"
echo ""

# ── Step 1: Query available snapshots ──────────────────────
echo -e "${YELLOW}📡 Step 1: Querying snapshots from Node $SRC_NODE...${NC}"

SNAPSHOTS_JSON=$(curl -sf "${SNAPSHOT_URL}/api/snapshots" 2>/dev/null || echo "null")

if [ "$SNAPSHOTS_JSON" = "null" ] || [ -z "$SNAPSHOTS_JSON" ]; then
    echo -e "${RED}  ❌ No snapshots available on Node $SRC_NODE (${SNAPSHOT_URL}/api/snapshots)${NC}"
    echo -e "${RED}     Make sure Node $SRC_NODE is running and has created at least one snapshot.${NC}"
    echo -e "${RED}     Snapshots are created automatically after epoch transitions.${NC}"
    exit 1
fi

SNAPSHOT_COUNT=$(echo "$SNAPSHOTS_JSON" | jq 'length')
if [ "$SNAPSHOT_COUNT" -eq 0 ] 2>/dev/null; then
    echo -e "${RED}  ❌ Snapshot list is empty. Wait for an epoch transition on Node $SRC_NODE.${NC}"
    exit 1
fi

LATEST=$(echo "$SNAPSHOTS_JSON" | jq -r '.[-1]')
SNAP_NAME=$(echo "$LATEST" | jq -r '.snapshot_name')
SNAP_EPOCH=$(echo "$LATEST" | jq -r '.epoch')
SNAP_BLOCK=$(echo "$LATEST" | jq -r '.block_number')
SNAP_TIME=$(echo "$LATEST" | jq -r '.created_at')

echo -e "${GREEN}  ✅ Found $SNAPSHOT_COUNT snapshot(s). Using latest:${NC}"
echo "     Name:  $SNAP_NAME"
echo "     Epoch: $SNAP_EPOCH"
echo "     Block: $SNAP_BLOCK"
echo "     Time:  $SNAP_TIME"
echo ""

DOWNLOAD_URL="${SNAPSHOT_URL}/files/${SNAP_NAME}/"

# ── Step 2: Stop destination node ──────────────────────────
echo -e "${YELLOW}🛑 Step 2: Stopping Node $DST_NODE...${NC}"
cd "$SCRIPT_DIR"
./stop_node.sh "$DST_NODE" 2>/dev/null || true
sleep 3
echo -e "${GREEN}  ✅ Node $DST_NODE stopped${NC}"

# ── Step 3: Wipe destination data ──────────────────────────
echo -e "${YELLOW}🗑️  Step 3: Wiping Node $DST_NODE data...${NC}"
DST="$SIMPLE_CHAIN_DIR/sample/node$DST_NODE"

rm -rf "$DST/data"
rm -rf "$DST/data-write"
rm -rf "$DST/back_up"
rm -rf "$DST/back_up_write"
rm -rf "$LOG_DIR/node_$DST_NODE"
rm -rf "$METANODE_ROOT/config/storage/node_$DST_NODE"
echo -e "${GREEN}  ✅ Node $DST_NODE data wiped${NC}"

# ── Step 4: Download snapshot via HTTP ─────────────────────
echo ""
echo -e "${YELLOW}📥 Step 4: Downloading snapshot via HTTP...${NC}"
echo "     URL: $DOWNLOAD_URL"
echo ""

DOWNLOAD_DIR="/tmp/snapshot_download_node${DST_NODE}"
rm -rf "$DOWNLOAD_DIR"
mkdir -p "$DOWNLOAD_DIR"

wget -c -r -np -nH --cut-dirs=2 \
    -P "$DOWNLOAD_DIR" \
    --reject="index.html*" \
    "$DOWNLOAD_URL" 2>&1 | tail -5

echo ""
echo -e "${GREEN}  ✅ Download complete${NC}"

# ── Step 5: Map snapshot dirs to correct Go Master layout ──
echo ""
echo -e "${YELLOW}📂 Step 5: Restoring data to Node $DST_NODE...${NC}"

# Create target directory structure
# Create target directory structure
mkdir -p "$DST/data/data"
mkdir -p "$DST/back_up"
mkdir -p "$DST/data-write/data"
mkdir -p "$DST/back_up_write"

# 5a. Move LevelDB dirs to BOTH Master (data/data) and Sub (data-write/data)
echo "  📁 Mapping LevelDB & Xapian dirs..."
for dir_name in $LEVELDB_DIRS; do
    if [ -d "$DOWNLOAD_DIR/$dir_name" ]; then
        # Copy to Master
        cp -a "$DOWNLOAD_DIR/$dir_name" "$DST/data/data/"
        # Copy to Sub
        cp -a "$DOWNLOAD_DIR/$dir_name" "$DST/data-write/data/"
        echo -e "${GREEN}    ✅ $dir_name/${NC}"
    fi
done

# 5b. Move back_up/ (Master PebbleDB) → back_up/
if [ -d "$DOWNLOAD_DIR/back_up" ]; then
    cp -r "$DOWNLOAD_DIR/back_up/"* "$DST/back_up/" 2>/dev/null || true
    SIZE=$(du -sh "$DST/back_up" 2>/dev/null | cut -f1)
    echo -e "${GREEN}    ✅ back_up/ → $SIZE${NC}"
else
    echo -e "${YELLOW}    ⚠️  back_up/ not in snapshot${NC}"
fi

# 5c. Move back_up_write/ (Sub PebbleDB) → back_up_write/
if [ -d "$DOWNLOAD_DIR/back_up_write" ]; then
    cp -r "$DOWNLOAD_DIR/back_up_write/"* "$DST/back_up_write/" 2>/dev/null || true
    SIZE=$(du -sh "$DST/back_up_write" 2>/dev/null | cut -f1)
    echo -e "${GREEN}    ✅ back_up_write/ → $SIZE${NC}"
else
    echo -e "${YELLOW}    ⚠️  back_up_write/ not in snapshot${NC}"
fi

# 5d. Move JMT State (MDBX) -> Rust config/storage/node_X/jmt_state/
if [ -d "$DOWNLOAD_DIR/jmt_state" ]; then
    mkdir -p "$METANODE_ROOT/config/storage/node_$DST_NODE/jmt_state"
    cp -a "$DOWNLOAD_DIR/jmt_state/"* "$METANODE_ROOT/config/storage/node_$DST_NODE/jmt_state/" 2>/dev/null || true
    SIZE=$(du -sh "$METANODE_ROOT/config/storage/node_$DST_NODE/jmt_state" 2>/dev/null | cut -f1)
    echo -e "${GREEN}    ✅ jmt_state/ → $SIZE${NC}"
else
    echo -e "${YELLOW}    ⚠️  jmt_state/ not in snapshot${NC}"
fi

# Show restored sizes
echo ""
echo -e "${GREEN}  📊 Restored data:${NC}"
echo "     data/data/:     $(du -sh "$DST/data/data/" 2>/dev/null | cut -f1)"
echo "     data-write/data/: $(du -sh "$DST/data-write/data/" 2>/dev/null | cut -f1)"
echo "     back_up/:       $(du -sh "$DST/back_up/" 2>/dev/null | cut -f1)"
echo "     back_up_write/: $(du -sh "$DST/back_up_write/" 2>/dev/null | cut -f1)"

# ── Step 5e: Clean PebbleDB LOCK files ─────────────────────
echo ""
echo -e "${YELLOW}🔓 Step 5e: Cleaning PebbleDB LOCK files...${NC}"
find "$DST" -name "LOCK" -delete 2>/dev/null || true
echo -e "${GREEN}  ✅ LOCK files removed${NC}"

# ── Step 5f: Clean Rust consensus storage ──────────────────
echo -e "${YELLOW}🗑️  Step 5f: Cleaning Rust storage for Node $DST_NODE...${NC}"
mkdir -p "$LOG_DIR/node_$DST_NODE"
mkdir -p "$METANODE_ROOT/config/storage/node_$DST_NODE"
rm -rf "$METANODE_ROOT/config/storage/node_$DST_NODE/epochs"
rm -rf "$METANODE_ROOT/config/storage/node_$DST_NODE/last_block_number.bin"
rm -rf "$METANODE_ROOT/config/storage/node_$DST_NODE/last_index.json"
echo -e "${GREEN}  ✅ Rust storage cleaned${NC}"

# Clean up temp download
rm -rf "$DOWNLOAD_DIR"

# ── Step 6: Restart destination node ───────────────────────
echo ""
echo -e "${YELLOW}🚀 Step 6: Restarting Node $DST_NODE...${NC}"
./resume_node.sh "$DST_NODE"
echo -e "${GREEN}  ✅ Node $DST_NODE restarted${NC}"

# ── Step 7: Monitor block sync ─────────────────────────────
echo ""
echo -e "${YELLOW}⏳ Step 7: Monitoring Node $DST_NODE block sync (60s)...${NC}"
for t in 10 20 30 40 50 60; do
    sleep 10
    BLOCK=$(grep 'last_committed_block' "$LOG_DIR/node_$DST_NODE/go-master-stdout.log" 2>/dev/null | tail -n 1 | grep -oP 'last_committed_block=\d+' || echo "N/A")
    GEI=$(grep 'gei=' "$LOG_DIR/node_$DST_NODE/go-master-stdout.log" 2>/dev/null | tail -n 1 | grep -oP 'gei=\d+' || echo "N/A")
    echo "  ⏱️ +${t}s: $BLOCK $GEI"
done

echo ""
echo -e "${BOLD}╔═══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  ✅ SNAPSHOT RESTORE (HTTP) COMPLETE                     ║${NC}"
echo -e "${BOLD}╚═══════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  📸 Restored from: ${BOLD}${SNAP_NAME}${NC} (epoch $SNAP_EPOCH, block $SNAP_BLOCK)"
echo -e "  Use: ${BOLD}tmux attach -t metanode-$DST_NODE${NC} to monitor"
echo ""
