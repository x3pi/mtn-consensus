#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  RESTORE NODE TỪ SNAPSHOT — SEQUENTIAL & FORK-SAFE
#  Usage: ./restore_node.sh <node_id> [snapshot_name]
#    node_id:       0-4 (bắt buộc)
#    snapshot_name:  tên snapshot (tùy chọn, mặc định = tự detect mới nhất)
#
#  Ví dụ:
#    ./restore_node.sh 2                          # tự tìm snapshot mới nhất
#    ./restore_node.sh 2 snap_epoch_1_block_50    # chỉ định snapshot cụ thể
#
#  Fork-prevention measures:
#    1. Enhanced Rust DAG cleanup (epochs, last_block_number, last_index)
#    2. Pre-start snapshot data validation
#    3. Sequential startup (Go Master→Go Sub→verify→Rust)
#    4. Block sync monitoring (90s) — confirm sequential advance
#    5. Hash divergence check against a reference node
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
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"
LOG_DIR="$METANODE_ROOT/logs"
BINARY="$METANODE_ROOT/target/release/metanode"

# Snapshot source — mặc định từ Node 0
SNAP_BASE_DIR="$GO_SIMPLE_ROOT/snapshot_data_node${NODE_ID}"
SNAP_API="http://localhost:8701/api/snapshots"

NODE_DATA="$GO_SIMPLE_ROOT/sample/node${NODE_ID}"

# ─── RPC ports for hash divergence check ──────────────────────
# Master RPC ports per node (from config-master-nodeX.json)
MASTER_RPC_PORTS=(8757 10747 10749 10750 10748)

# Config maps (same as resume_node.sh)
GO_MASTER_CONFIG=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json" "config-master-node4.json")
GO_SUB_CONFIG=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json" "config-sub-node4.json")
GO_DATA_DIR=("node0" "node1" "node2" "node3" "node4")

GO_MASTER_SESSION=("go-master-0" "go-master-1" "go-master-2" "go-master-3" "go-master-4")
GO_SUB_SESSION=("go-sub-0" "go-sub-1" "go-sub-2" "go-sub-3" "go-sub-4")
RUST_SESSION=("metanode-0" "metanode-1" "metanode-2" "metanode-3" "metanode-4")

GO_MASTER_SOCKET=("/tmp/rust-go-node0-master.sock" "/tmp/rust-go-node1-master.sock" "/tmp/rust-go-node2-master.sock" "/tmp/rust-go-node3-master.sock" "/tmp/rust-go-node4-master.sock")
EXECUTOR_SOCKET=("/tmp/executor0.sock" "/tmp/executor1.sock" "/tmp/executor2.sock" "/tmp/executor3.sock" "/tmp/executor4.sock")

RUST_CONFIG=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml" "config/node_4.toml")

# ─── Helpers ─────────────────────────────────────────────────
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
            echo -e "${RED}  ❌ Timeout waiting for $name (${timeout}s)${NC}"
            return 1
        fi
        sleep 1
    done
}

# Find a reference node (any running node != NODE_ID) for hash comparison
find_reference_node() {
    for i in 0 1 2 3 4; do
        [ "$i" -eq "$NODE_ID" ] && continue
        local port=${MASTER_RPC_PORTS[$i]}
        local resp=$(curl -sf -X POST -H "Content-Type: application/json" \
            --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
            "http://127.0.0.1:$port" 2>/dev/null || echo "")
        if [ -n "$resp" ]; then
            local hex_block=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',''))" 2>/dev/null || echo "")
            if [ -n "$hex_block" ] && [ "$hex_block" != "None" ] && [ "$hex_block" != "" ]; then
                echo "$i"
                return 0
            fi
        fi
    done
    echo ""
    return 1
}

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  📸 RESTORE Node $NODE_ID — Sequential & Fork-Safe${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""

# ─── Tìm snapshot mới nhất ────────────────────────────────────
# Helper: find a snapshot dir across ALL nodes' snapshot directories
find_snap_dir() {
    local snap_name="$1"
    for i in 0 1 2 3 4; do
        local candidate="$GO_SIMPLE_ROOT/snapshot_data_node${i}/$snap_name"
        if [ -d "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

if [ -n "$2" ]; then
    SNAP_NAME="$2"
    echo -e "${BLUE}📸 Sử dụng snapshot chỉ định: ${NC}$SNAP_NAME"
else
    echo -e "${BLUE}🔍 Tự động tìm snapshot mới nhất...${NC}"
    
    # Thử qua API trước
    SNAP_NAME=$(curl -sf "$SNAP_API" 2>/dev/null \
        | python3 -c "import sys,json; snaps=json.load(sys.stdin); print(snaps[-1]['snapshot_name'])" 2>/dev/null) || true
    
    # Validate: kiểm tra snapshot tồn tại ở BẤT KỲ node nào (không chỉ target node)
    if [ -n "$SNAP_NAME" ]; then
        FOUND_DIR=$(find_snap_dir "$SNAP_NAME")
        if [ -z "$FOUND_DIR" ]; then
            echo -e "${YELLOW}⚠️  API trả về $SNAP_NAME nhưng thư mục chưa tồn tại ở bất kỳ node nào. Fallback sang filesystem...${NC}"
            SNAP_NAME=""
        fi
    fi
    
    # Fallback: tìm snapshot MỚI NHẤT từ TẤT CẢ nodes' snapshot dirs
    if [ -z "$SNAP_NAME" ]; then
        SNAP_NAME=$(find "$GO_SIMPLE_ROOT" -maxdepth 2 -type d -name "snap_*" 2>/dev/null \
            | xargs -I{} basename {} \
            | sort -t_ -k4 -n \
            | tail -1) || true
    fi
    
    if [ -z "$SNAP_NAME" ]; then
        echo -e "${RED}❌ Không tìm thấy snapshot nào!${NC}"
        echo "   Kiểm tra: curl $SNAP_API"
        echo "   Hoặc:     ls $GO_SIMPLE_ROOT/snapshot_data_node*/snap_*"
        exit 1
    fi
    
    echo -e "${GREEN}  ✅ Tìm thấy: ${NC}$SNAP_NAME"
fi

# Tìm snapshot dir — ưu tiên target node, nếu không có thì tìm ở node khác
SNAP_DIR="$SNAP_BASE_DIR/$SNAP_NAME"
if [ ! -d "$SNAP_DIR" ]; then
    SNAP_DIR=$(find_snap_dir "$SNAP_NAME")
    if [ -n "$SNAP_DIR" ]; then
        echo -e "${YELLOW}  ℹ️  Snapshot không có ở node $NODE_ID, dùng từ: $(basename $(dirname $SNAP_DIR))${NC}"
    fi
fi
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
echo "   2. Xóa TOÀN BỘ dữ liệu Node $NODE_ID (Go + Rust DAG)"
echo "   3. Khôi phục từ snapshot: $SNAP_NAME"
echo "   4. Validate dữ liệu snapshot"
echo "   5. Khởi động tuần tự (Go Master→Go Sub→Rust)"
echo "   6. Giám sát sync tuần tự 90s"
echo "   7. Kiểm tra hash divergence với mạng"
echo ""
read -p "Tiếp tục? (y/N): " CONFIRM
if [[ "$CONFIRM" != "y" && "$CONFIRM" != "Y" ]]; then
    echo "Đã hủy."
    exit 0
fi

START_TIME=$(date +%s)

# ══════════════════════════════════════════════════════════════
# Step 1: Stop Node
# ══════════════════════════════════════════════════════════════
echo ""
echo -e "${BLUE}[1/7] 🛑 Dừng Node $NODE_ID...${NC}"
"$SCRIPT_DIR/stop_node.sh" "$NODE_ID" 2>/dev/null || true
sleep 3
echo -e "${GREEN}  ✅ Node $NODE_ID đã dừng${NC}"

# ══════════════════════════════════════════════════════════════
# Step 2: Xóa data — ENHANCED: full Rust DAG cleanup
# ══════════════════════════════════════════════════════════════
echo -e "${BLUE}[2/7] 🗑️  Xóa dữ liệu Node $NODE_ID (Go + Rust DAG)...${NC}"

# Go data
rm -rf "$NODE_DATA/data"
rm -rf "$NODE_DATA/data-write"
rm -rf "$NODE_DATA/back_up"
rm -rf "$NODE_DATA/back_up_write"

# Rust DAG — CRITICAL: xóa toàn bộ để tránh replay stale commits
RUST_STORAGE="$METANODE_ROOT/config/storage/node_$NODE_ID"
rm -rf "$RUST_STORAGE"
echo -e "${GREEN}  ✅ Rust DAG storage đã xóa: ${NC}$RUST_STORAGE"

# Clear old logs for this node to avoid confusion
rm -f "$LOG_DIR/node_$NODE_ID/go-master-stdout.log" 2>/dev/null || true
rm -f "$LOG_DIR/node_$NODE_ID/go-sub-stdout.log" 2>/dev/null || true
rm -f "$LOG_DIR/node_$NODE_ID/rust.log" 2>/dev/null || true
mkdir -p "$LOG_DIR/node_$NODE_ID"
echo -e "${GREEN}  ✅ Dữ liệu và logs đã xóa sạch${NC}"

# ══════════════════════════════════════════════════════════════
# Step 3: Restore từ Snapshot
# ══════════════════════════════════════════════════════════════
echo -e "${BLUE}[3/7] 📸 Khôi phục từ $SNAP_NAME...${NC}"

# Tạo thư mục cha trước
mkdir -p "$NODE_DATA/data/data"
mkdir -p "$NODE_DATA/data-write/data"
mkdir -p "$NODE_DATA/back_up"
mkdir -p "$NODE_DATA/back_up_write"

# Copy LevelDB dirs to BOTH Master (data/data) and Sub (data-write/data)
echo "  📁 Mapping LevelDB & Xapian dirs..."
LEVELDB_COUNT=0
for folder in account_state blocks mapping receipts smart_contract_code smart_contract_storage stake_db transaction_state trie_database backup_device_key_storage xapian_node; do
  if [ -d "$SNAP_DIR/$folder" ]; then
    cp -a "$SNAP_DIR/$folder" "$NODE_DATA/data/data/"
    cp -a "$SNAP_DIR/$folder" "$NODE_DATA/data-write/data/"
    LEVELDB_COUNT=$((LEVELDB_COUNT + 1))
  fi
done
echo -e "${GREEN}  ✅ $LEVELDB_COUNT LevelDB dirs copied${NC}"

# Copy PebbleDB dirs
echo "  📁 Mapping PebbleDB dirs..."
if [ -d "$SNAP_DIR/back_up" ]; then 
    cp -a "$SNAP_DIR/back_up/"* "$NODE_DATA/back_up/" 2>/dev/null || true
fi
if [ -d "$SNAP_DIR/back_up_write" ]; then 
    cp -a "$SNAP_DIR/back_up_write/"* "$NODE_DATA/back_up_write/" 2>/dev/null || true
fi

# CRITICAL: Copy epoch_data_backup.json — without this, Go starts at epoch 0
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
    echo -e "${RED}  ❌ epoch_data_backup.json NOT found in snapshot!${NC}"
    echo -e "${RED}     Go sẽ bắt đầu từ epoch 0 → RẤT DỄ GÂY FORK!${NC}"
    echo -e "${YELLOW}     Tiếp tục nhưng cần theo dõi kỹ...${NC}"
fi

# Cleanup: remove stale LOCK files
find "$NODE_DATA" -name "LOCK" -delete 2>/dev/null
mkdir -p "$NODE_DATA/data/data/xapian_node"
mkdir -p "$NODE_DATA/data-write/data/xapian_node"

RESTORED_SIZE=$(du -sh "$NODE_DATA" 2>/dev/null | awk '{print $1}')
echo -e "${GREEN}  ✅ Đã khôi phục tổng cộng: $RESTORED_SIZE${NC}"

# CRITICAL: Create RESET_GEI marker files
# Go startup checks for this marker and resets GEI=0, which prevents
# Rust's COLD-START GUARD from skipping commits after restore.
# Go's sequential block guard handles deduplication for blocks already committed.
echo -e "${BLUE}  📌 Creating RESET_GEI markers for GEI reset...${NC}"
touch "$NODE_DATA/data/data/RESET_GEI"
touch "$NODE_DATA/data-write/data/RESET_GEI"
echo -e "${GREEN}  ✅ RESET_GEI markers created (Go will reset GEI=0 on startup)${NC}"

# ══════════════════════════════════════════════════════════════
# Step 4: Pre-start Validation
# ══════════════════════════════════════════════════════════════
echo -e "${BLUE}[4/7] 🔍 Kiểm tra tính toàn vẹn dữ liệu snapshot...${NC}"
VALIDATION_OK=true

# Check epoch data
if [ -f "$NODE_DATA/back_up/epoch_data_backup.json" ]; then
    python3 -c "import json; json.load(open('$NODE_DATA/back_up/epoch_data_backup.json'))" 2>/dev/null
    if [ $? -eq 0 ]; then
        echo -e "${GREEN}  ✅ epoch_data_backup.json — valid JSON${NC}"
    else
        echo -e "${RED}  ❌ epoch_data_backup.json — INVALID JSON!${NC}"
        VALIDATION_OK=false
    fi
fi

# Check key LevelDB dirs exist
REQUIRED_DIRS=("blocks" "account_state" "trie_database")
for dir in "${REQUIRED_DIRS[@]}"; do
    if [ -d "$NODE_DATA/data/data/$dir" ] && [ -d "$NODE_DATA/data-write/data/$dir" ]; then
        echo -e "${GREEN}  ✅ $dir/ — present in both Master & Sub${NC}"
    else
        echo -e "${RED}  ❌ $dir/ — MISSING in Master or Sub!${NC}"
        VALIDATION_OK=false
    fi
done

# Check PebbleDB has content
PEBBLE_SIZE=$(du -sh "$NODE_DATA/back_up" 2>/dev/null | awk '{print $1}')
if [ -n "$PEBBLE_SIZE" ] && [ "$PEBBLE_SIZE" != "0" ]; then
    echo -e "${GREEN}  ✅ PebbleDB back_up/ — $PEBBLE_SIZE${NC}"
else
    echo -e "${YELLOW}  ⚠️  PebbleDB back_up/ trống hoặc không tồn tại${NC}"
fi

# Verify Rust storage is clean
if [ -d "$RUST_STORAGE" ] && [ "$(ls -A "$RUST_STORAGE" 2>/dev/null)" ]; then
    echo -e "${RED}  ❌ Rust storage vẫn còn dữ liệu cũ!${NC}"
    VALIDATION_OK=false
else
    echo -e "${GREEN}  ✅ Rust DAG storage — sạch${NC}"
fi

if [ "$VALIDATION_OK" = false ]; then
    echo -e "${RED}  ⚠️  Validation có lỗi! Tiếp tục có thể gây fork.${NC}"
    read -p "  Vẫn tiếp tục? (y/N): " CONTINUE_ANYWAY
    if [[ "$CONTINUE_ANYWAY" != "y" && "$CONTINUE_ANYWAY" != "Y" ]]; then
        echo "Đã hủy."
        exit 1
    fi
fi

# ══════════════════════════════════════════════════════════════
# Step 5: Sequential Startup — Go Master → Go Sub → Rust
# ══════════════════════════════════════════════════════════════
echo -e "${BLUE}[5/7] 🚀 Khởi động tuần tự Node $NODE_ID...${NC}"

# 5a. Start Go Master
echo -e "${CYAN}  [5a] Go Master...${NC}"
cd "$GO_SIMPLE_ROOT"

DATA="${GO_DATA_DIR[$NODE_ID]}"
XAPIAN_MASTER="sample/$DATA/data/data/xapian_node"
tmux new-session -d -s "${GO_MASTER_SESSION[$NODE_ID]}" -c "$GO_SIMPLE_ROOT" \
    "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN_MASTER' && ./simple_chain -config=${GO_MASTER_CONFIG[$NODE_ID]} >> \"$LOG_DIR/node_$NODE_ID/go-master-stdout.log\" 2>&1"
echo -e "${GREEN}    🚀 Go Master started (${GO_MASTER_SESSION[$NODE_ID]})${NC}"

# 5b. Wait for Go Master socket
echo -e "${CYAN}  [5b] Waiting for Go Master socket...${NC}"
wait_for_socket "${GO_MASTER_SOCKET[$NODE_ID]}" "Go Master $NODE_ID" 120

# 5c. Start Go Sub
echo -e "${CYAN}  [5c] Go Sub...${NC}"
XAPIAN_SUB="sample/$DATA/data-write/data/xapian_node"
tmux new-session -d -s "${GO_SUB_SESSION[$NODE_ID]}" -c "$GO_SIMPLE_ROOT" \
    "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN_SUB' && ./simple_chain -config=${GO_SUB_CONFIG[$NODE_ID]} >> \"$LOG_DIR/node_$NODE_ID/go-sub-stdout.log\" 2>&1"
echo -e "${GREEN}    🚀 Go Sub started (${GO_SUB_SESSION[$NODE_ID]})${NC}"

# 5d. Wait for Go to load snapshot state — verify block height
echo -e "${CYAN}  [5d] Đợi Go nhận dữ liệu snapshot (10s)...${NC}"
sleep 10

# Read Go's recognized block height
GO_BLOCK=""
for attempt in 1 2 3; do
    GO_BLOCK=$(grep -a "last_committed_block=" "$LOG_DIR/node_$NODE_ID/go-master-stdout.log" 2>/dev/null | tail -1 | sed -n 's/.*last_committed_block=\([0-9]*\).*/\1/p') || true
    [ -n "$GO_BLOCK" ] && break
    sleep 3
done

if [ -n "$GO_BLOCK" ]; then
    echo -e "${GREEN}    ✅ Go Master nhận snapshot — block=$GO_BLOCK${NC}"
else
    echo -e "${YELLOW}    ⚠️  Chưa đọc được block height từ Go Master log${NC}"
fi

# 5e. Start Rust Metanode
echo -e "${CYAN}  [5e] Rust Metanode...${NC}"
cd "$METANODE_ROOT"
tmux new-session -d -s "${RUST_SESSION[$NODE_ID]}" -c "$METANODE_ROOT" \
    "export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; $BINARY start --config ${RUST_CONFIG[$NODE_ID]} >> \"$LOG_DIR/node_$NODE_ID/rust.log\" 2>&1"
echo -e "${GREEN}    🚀 Rust Metanode started (${RUST_SESSION[$NODE_ID]})${NC}"

# ══════════════════════════════════════════════════════════════
# Step 6: Sequential Sync Monitoring (90s)
# ══════════════════════════════════════════════════════════════
echo ""
echo -e "${BLUE}[6/7] 📊 Giám sát sync tuần tự (90s)...${NC}"
echo -e "${BLUE}       Kiểm tra block đang tăng tuần tự, không bị stuck hay jump${NC}"

PREV_BLOCK=""
STUCK_COUNT=0
MAX_STUCK=3  # 3 lần liên tiếp = 30s không tăng → cảnh báo

for t in 10 20 30 40 50 60 70 80 90; do
    sleep 10
    
    # Read current block from log
    CURRENT_BLOCK=$(grep -a 'last_committed_block=' "$LOG_DIR/node_$NODE_ID/go-master-stdout.log" 2>/dev/null | tail -1 | sed -n 's/.*last_committed_block=\([0-9]*\).*/\1/p') || true
    
    # Read GEI if available
    GEI=$(grep -a 'gei=' "$LOG_DIR/node_$NODE_ID/go-master-stdout.log" 2>/dev/null | tail -1 | grep -oP 'gei=\d+' 2>/dev/null || echo "")
    
    # Check Rust status
    RUST_STATUS=$(grep -a "commit_index\|EPOCH\|SYNC" "$LOG_DIR/node_$NODE_ID/rust.log" 2>/dev/null | tail -1 | head -c 100 || echo "")
    
    if [ -z "$CURRENT_BLOCK" ]; then
        echo -e "  ${YELLOW}⏱️ +${t}s: block=? (Go chưa ghi log)${NC}"
        continue
    fi
    
    # Detect stuck (no progress)
    if [ "$CURRENT_BLOCK" = "$PREV_BLOCK" ]; then
        STUCK_COUNT=$((STUCK_COUNT + 1))
        if [ $STUCK_COUNT -ge $MAX_STUCK ]; then
            echo -e "  ${RED}⏱️ +${t}s: block=$CURRENT_BLOCK $GEI — ⚠️ STUCK ${STUCK_COUNT}x liên tiếp!${NC}"
        else
            echo -e "  ${YELLOW}⏱️ +${t}s: block=$CURRENT_BLOCK $GEI — (chưa tăng)${NC}"
        fi
    else
        STUCK_COUNT=0
        # Check for big jump (potential fork indicator)
        if [ -n "$PREV_BLOCK" ]; then
            JUMP=$((CURRENT_BLOCK - PREV_BLOCK))
            if [ $JUMP -gt 100 ]; then
                echo -e "  ${YELLOW}⏱️ +${t}s: block=$CURRENT_BLOCK $GEI — ⚡ jump +$JUMP blocks${NC}"
            else
                echo -e "  ${GREEN}⏱️ +${t}s: block=$CURRENT_BLOCK $GEI — ✅ +$JUMP blocks${NC}"
            fi
        else
            echo -e "  ${GREEN}⏱️ +${t}s: block=$CURRENT_BLOCK $GEI — ✅ syncing${NC}"
        fi
    fi
    
    PREV_BLOCK="$CURRENT_BLOCK"
done

if [ $STUCK_COUNT -ge $MAX_STUCK ]; then
    echo -e "${RED}  ⚠️  Node có thể bị stuck! Kiểm tra logs:${NC}"
    echo "      tail -50 $LOG_DIR/node_$NODE_ID/rust.log"
    echo "      tail -50 $LOG_DIR/node_$NODE_ID/go-master-stdout.log"
fi

# ══════════════════════════════════════════════════════════════
# Step 7: Hash Divergence Check
# ══════════════════════════════════════════════════════════════
echo ""
echo -e "${BLUE}[7/7] 🔒 Kiểm tra hash divergence...${NC}"

# Get restored node's current block via RPC
RESTORED_PORT=${MASTER_RPC_PORTS[$NODE_ID]}
RESTORED_RESP=$(curl -sf -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    "http://127.0.0.1:$RESTORED_PORT" 2>/dev/null || echo "")

if [ -z "$RESTORED_RESP" ]; then
    echo -e "${YELLOW}  ⚠️  Không kết nối được RPC Node $NODE_ID (port $RESTORED_PORT) — bỏ qua hash check${NC}"
else
    RESTORED_HEX=$(echo "$RESTORED_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result','0x0'))" 2>/dev/null || echo "0x0")
    RESTORED_DEC=$((16#${RESTORED_HEX#0x}))
    echo -e "${BLUE}  Node $NODE_ID block hiện tại: $RESTORED_DEC${NC}"
    
    # Find a reference node
    REF_NODE=$(find_reference_node)
    
    if [ -z "$REF_NODE" ]; then
        echo -e "${YELLOW}  ⚠️  Không tìm được node tham chiếu — bỏ qua hash check${NC}"
    else
        REF_PORT=${MASTER_RPC_PORTS[$REF_NODE]}
        echo -e "${BLUE}  Sử dụng Node $REF_NODE (port $REF_PORT) làm node tham chiếu${NC}"
        
        # Compare hashes at the restored node's current block
        if [ $RESTORED_DEC -gt 0 ]; then
            CHECK_BLOCK_HEX=$(printf "0x%x" $RESTORED_DEC)
            
            # Get hash from restored node
            HASH_RESTORED=$(curl -sf -X POST -H "Content-Type: application/json" \
                --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"$CHECK_BLOCK_HEX\",false],\"id\":1}" \
                "http://127.0.0.1:$RESTORED_PORT" 2>/dev/null \
                | python3 -c "import sys,json; r=json.load(sys.stdin).get('result',{}); print(r.get('hash','') if r else '')" 2>/dev/null || echo "")
            
            # Get hash from reference node
            HASH_REFERENCE=$(curl -sf -X POST -H "Content-Type: application/json" \
                --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"$CHECK_BLOCK_HEX\",false],\"id\":1}" \
                "http://127.0.0.1:$REF_PORT" 2>/dev/null \
                | python3 -c "import sys,json; r=json.load(sys.stdin).get('result',{}); print(r.get('hash','') if r else '')" 2>/dev/null || echo "")
            
            if [ -n "$HASH_RESTORED" ] && [ -n "$HASH_REFERENCE" ]; then
                if [ "$HASH_RESTORED" = "$HASH_REFERENCE" ]; then
                    echo -e "${GREEN}  ✅ Block $RESTORED_DEC hash KHỚP giữa Node $NODE_ID và Node $REF_NODE${NC}"
                    echo -e "${GREEN}     Hash: ${HASH_RESTORED:0:18}...${NC}"
                else
                    echo -e "${RED}  ❌ FORK DETECTED! Block $RESTORED_DEC hash KHÁC NHAU!${NC}"
                    echo -e "${RED}     Node $NODE_ID:  $HASH_RESTORED${NC}"
                    echo -e "${RED}     Node $REF_NODE: $HASH_REFERENCE${NC}"
                    echo ""
                    echo -e "${RED}  🚨 HÀNH ĐỘNG CẦN THIẾT:${NC}"
                    echo -e "${RED}     1. Dừng Node $NODE_ID: ./stop_node.sh $NODE_ID${NC}"
                    echo -e "${RED}     2. Kiểm tra snapshot gốc có bị corruption${NC}"
                    echo -e "${RED}     3. Thử restore lại từ snapshot khác${NC}"
                fi
            else
                echo -e "${YELLOW}  ⚠️  Không lấy được hash block — bỏ qua (node đang sync?)${NC}"
            fi
        else
            echo -e "${YELLOW}  ⚠️  Block height = 0 — node chưa sync, bỏ qua hash check${NC}"
        fi
    fi
fi

# ══════════════════════════════════════════════════════════════
# Summary
# ══════════════════════════════════════════════════════════════
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
echo -e "  ${BLUE}tmux sessions:${NC}"
echo "    Go Master: tmux attach -t ${GO_MASTER_SESSION[$NODE_ID]}"
echo "    Go Sub:    tmux attach -t ${GO_SUB_SESSION[$NODE_ID]}"
echo "    Rust:      tmux attach -t ${RUST_SESSION[$NODE_ID]}"
echo ""
echo -e "  ${BLUE}Theo dõi:${NC}"
echo "    tail -f $LOG_DIR/node_$NODE_ID/rust.log"
echo "    tail -f $LOG_DIR/node_$NODE_ID/go-master-stdout.log"
echo ""
