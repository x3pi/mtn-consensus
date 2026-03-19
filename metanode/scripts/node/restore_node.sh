#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  RESTORE NODE TỪ SNAPSHOT MỚI NHẤT
#  Usage: ./restore_node.sh <node_id> [snapshot_name]
#    node_id:       0-4 (bắt buộc)
#    snapshot_name:  tên snapshot (tùy chọn, mặc định = tự detect mới nhất)
#
#  Ví dụ:
#    ./restore_node.sh 2                          # tự tìm snapshot mới nhất
#    ./restore_node.sh 2 snap_epoch_1_block_50    # chỉ định snapshot cụ thể
# ═══════════════════════════════════════════════════════════════
set -e

NODE_ID="${1:?❌ Usage: $0 <node_id> [snapshot_name]}"

if [[ ! "$NODE_ID" =~ ^[0-4]$ ]]; then
    echo "❌ node_id phải từ 0-4, nhận được: $NODE_ID"
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

# Snapshot source — mặc định từ Node 0
SNAP_BASE_DIR="$GO_SIMPLE_ROOT/snapshot_data_node0"
SNAP_API="http://localhost:8701/api/snapshots"

NODE_DATA="$GO_SIMPLE_ROOT/sample/node${NODE_ID}"

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  📸 RESTORE Node $NODE_ID từ Snapshot${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""

# ─── Tìm snapshot mới nhất ────────────────────────────────────
if [ -n "$2" ]; then
    SNAP_NAME="$2"
    echo -e "${BLUE}📸 Sử dụng snapshot chỉ định: ${NC}$SNAP_NAME"
else
    echo -e "${BLUE}🔍 Tự động tìm snapshot mới nhất...${NC}"
    
    # Thử qua API trước
    SNAP_NAME=$(curl -sf "$SNAP_API" 2>/dev/null \
        | python3 -c "import sys,json; snaps=json.load(sys.stdin); print(snaps[-1]['snapshot_name'])" 2>/dev/null) || true
    
    # Fallback: tìm trong thư mục
    if [ -z "$SNAP_NAME" ]; then
        SNAP_NAME=$(ls -1d "$SNAP_BASE_DIR"/snap_* 2>/dev/null | sort | tail -1 | xargs basename 2>/dev/null) || true
    fi
    
    if [ -z "$SNAP_NAME" ]; then
        echo -e "${RED}❌ Không tìm thấy snapshot nào!${NC}"
        echo "   Kiểm tra: curl $SNAP_API"
        echo "   Hoặc:     ls $SNAP_BASE_DIR/"
        exit 1
    fi
    
    echo -e "${GREEN}  ✅ Tìm thấy: ${NC}$SNAP_NAME"
fi

SNAP_DIR="$SNAP_BASE_DIR/$SNAP_NAME"
if [ ! -d "$SNAP_DIR" ]; then
    echo -e "${RED}❌ Thư mục snapshot không tồn tại: $SNAP_DIR${NC}"
    exit 1
fi

SNAP_SIZE=$(du -sh "$SNAP_DIR" 2>/dev/null | awk '{print $1}')
echo -e "${BLUE}  📦 Kích thước: ${NC}$SNAP_SIZE"

# ─── Xác nhận ─────────────────────────────────────────────────
echo ""
echo -e "${YELLOW}⚠️  Thao tác này sẽ:${NC}"
echo "   1. Dừng Node $NODE_ID"
echo "   2. Xóa TOÀN BỘ dữ liệu Node $NODE_ID"
echo "   3. Khôi phục từ snapshot: $SNAP_NAME"
echo "   4. Khởi động lại Node $NODE_ID"
echo ""
read -p "Tiếp tục? (y/N): " CONFIRM
if [[ "$CONFIRM" != "y" && "$CONFIRM" != "Y" ]]; then
    echo "Đã hủy."
    exit 0
fi

START_TIME=$(date +%s)

# ─── Step 1: Stop Node ────────────────────────────────────────
echo ""
echo -e "${BLUE}[1/4] 🛑 Dừng Node $NODE_ID...${NC}"
"$SCRIPT_DIR/stop_node.sh" "$NODE_ID" 2>/dev/null || true
sleep 3
echo -e "${GREEN}  ✅ Node $NODE_ID đã dừng${NC}"

# ─── Step 2: Xóa data ─────────────────────────────────────────
echo -e "${BLUE}[2/4] 🗑️  Xóa dữ liệu Node $NODE_ID...${NC}"
rm -rf "$NODE_DATA/data"
rm -rf "$NODE_DATA/data-write"
rm -rf "$NODE_DATA/back_up"
rm -rf "$NODE_DATA/back_up_write"
# Delete Rust DAG database so it can sync from scratch over the network
rm -rf "$METANODE_ROOT/config/storage/node_$NODE_ID"
echo -e "${GREEN}  ✅ Dữ liệu đã xóa${NC}"

# ─── Step 3: Restore từ Snapshot ──────────────────────────────
echo -e "${BLUE}[3/4] 📸 Khôi phục từ $SNAP_NAME (Copying 59GB)...${NC}"

# Tạo thư mục cha trước
mkdir -p "$NODE_DATA/data/data"
mkdir -p "$NODE_DATA/data-write/data"
mkdir -p "$NODE_DATA/back_up"
mkdir -p "$NODE_DATA/back_up_write"

# Copy LevelDB dirs to BOTH Master (data/data) and Sub (data-write/data)
# Because the snapshot was taken from Sub, putting it in Master makes Master fully synced.
# Putting it in Sub allows Sub to continue executing.
echo "  📁 Mapping LevelDB & Xapian dirs..."
for folder in account_state blocks mapping receipts smart_contract_code smart_contract_storage stake_db transaction_state trie_database backup_device_key_storage xapian_node; do
  if [ -d "$SNAP_DIR/$folder" ]; then
    cp -a "$SNAP_DIR/$folder" "$NODE_DATA/data/data/"
    cp -a "$SNAP_DIR/$folder" "$NODE_DATA/data-write/data/"
  fi
done

# Copy PebbleDB dirs
echo "  📁 Mapping PebbleDB dirs..."
if [ -d "$SNAP_DIR/back_up" ]; then 
    cp -a "$SNAP_DIR/back_up/"* "$NODE_DATA/back_up/" 2>/dev/null || true
fi
if [ -d "$SNAP_DIR/back_up_write" ]; then 
    cp -a "$SNAP_DIR/back_up_write/"* "$NODE_DATA/back_up_write/" 2>/dev/null || true
fi

# CRITICAL: Copy epoch_data_backup.json — without this, Go starts at epoch 0
# after restore and Rust gets stuck at wrong epoch forever
echo "  📁 Copying epoch data..."
EPOCH_RESTORED=false
for EPOCH_SRC in "$SNAP_DIR/back_up/epoch_data_backup.json" "$SNAP_DIR/back_up_write/epoch_data_backup.json"; do
    if [ -f "$EPOCH_SRC" ]; then
        cp -a "$EPOCH_SRC" "$NODE_DATA/back_up/epoch_data_backup.json"
        cp -a "$EPOCH_SRC" "$NODE_DATA/back_up_write/epoch_data_backup.json"
        EPOCH_RESTORED=true
        EPOCH_INFO=$(python3 -c "import json; d=json.load(open('$EPOCH_SRC')); print(f'epoch={d[\"current_epoch\"]}')" 2>/dev/null || echo "epoch=?")
        echo -e "${GREEN}  ✅ Epoch data restored: ${EPOCH_INFO}${NC}"
        break
    fi
done
if [ "$EPOCH_RESTORED" = false ]; then
    echo -e "${YELLOW}  ⚠️  epoch_data_backup.json NOT found in snapshot! Go will start at epoch 0.${NC}"
    echo -e "${YELLOW}      Node may need manual epoch sync after startup.${NC}"
fi

# Tạo các thư mục bắt buộc (và loại bỏ LOCK cũ rác nếu có)
find "$NODE_DATA" -name "LOCK" -delete 2>/dev/null
mkdir -p "$NODE_DATA/data/data/xapian_node"
mkdir -p "$NODE_DATA/data-write/data/xapian_node"

RESTORED_SIZE=$(du -sh "$NODE_DATA" 2>/dev/null | awk '{print $1}')
echo -e "${GREEN}  ✅ Đã khôi phục tổng cộng: $RESTORED_SIZE${NC}"

# ─── Step 4: Khởi động lại ────────────────────────────────────
echo -e "${BLUE}[4/4] 🚀 Khởi động Node $NODE_ID...${NC}"
"$SCRIPT_DIR/resume_node.sh" "$NODE_ID"

ELAPSED=$(( $(date +%s) - START_TIME ))

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  ✅ RESTORE HOÀN TẤT trong ${ELAPSED}s${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo -e "  ${BLUE}Snapshot:${NC}  $SNAP_NAME"
echo -e "  ${BLUE}Node:${NC}     $NODE_ID"
echo -e "  ${BLUE}Data:${NC}     $RESTORED_SIZE"
echo ""
echo -e "  ${YELLOW}⏳ Rust Metanode đang replay commits (~3 phút)${NC}"
echo -e "  ${BLUE}Theo dõi:${NC}"
echo "    tail -f $LOG_DIR/node_$NODE_ID/rust.log"
echo ""
