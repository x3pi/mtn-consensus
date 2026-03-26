#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  FRESH RESTART — Stop All → Build All → Clean Data → Run All
#  Usage: ./fresh_restart.sh [--no-build] [--keep-data]
#    --no-build    Skip build step (use existing binaries)
#    --keep-data   Keep existing data (resume instead of fresh start)
# ═══════════════════════════════════════════════════════════════
set -e
set -o pipefail

# ─── Parse args ──────────────────────────────────────────────
SKIP_BUILD=false
KEEP_DATA=false
for arg in "$@"; do
    case $arg in
        --no-build)  SKIP_BUILD=true ;;
        --keep-data) KEEP_DATA=true ;;
        *)           echo "Unknown arg: $arg"; exit 1 ;;
    esac
done

# ─── Colors ──────────────────────────────────────────────────
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

# ─── Paths ───────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"
LOG_DIR="$METANODE_ROOT/logs"
BINARY="$METANODE_ROOT/target/release/metanode"

NODES=(0 1 2 3 4)
GO_MASTER_CONFIG=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json" "config-master-node4.json")
GO_SUB_CONFIG=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json" "config-sub-node4.json")
GO_DATA_DIR=("node0" "node1" "node2" "node3" "node4")
GO_MASTER_SESSION=("go-master-0" "go-master-1" "go-master-2" "go-master-3" "go-master-4")
GO_SUB_SESSION=("go-sub-0" "go-sub-1" "go-sub-2" "go-sub-3" "go-sub-4")
RUST_SESSION=("metanode-0" "metanode-1" "metanode-2" "metanode-3" "metanode-4")
GO_MASTER_SOCKET=("/tmp/rust-go-node0-master.sock" "/tmp/rust-go-node1-master.sock" "/tmp/rust-go-node2-master.sock" "/tmp/rust-go-node3-master.sock" "/tmp/rust-go-node4-master.sock")
RUST_CONFIG=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml" "config/node_4.toml")

ulimit -n 100000 || true

# ─── Helper ──────────────────────────────────────────────────
wait_for_socket() {
    local socket=$1 name=$2 timeout=${3:-120}
    local start=$(date +%s)
    while true; do
        [ -S "$socket" ] && { echo -e "${GREEN}  ✅ $name ready ($(( $(date +%s) - start ))s)${NC}"; return 0; }
        [ $(( $(date +%s) - start )) -ge $timeout ] && { echo -e "${RED}  ❌ Timeout waiting for $name${NC}"; return 1; }
        sleep 1
    done
}

START_TIME=$(date +%s)
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
if $KEEP_DATA; then
    echo -e "${GREEN}  🔄 RESTART ALL (keep data) — $(date '+%H:%M:%S')${NC}"
else
    echo -e "${GREEN}  🚀 FRESH RESTART (clean data) — $(date '+%H:%M:%S')${NC}"
fi
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# ==============================================================================
# STEP 1: STOP ALL
# ==============================================================================
echo -e "${BLUE}[1/5] 🛑 Stop all processes...${NC}"
# Send SIGTERM first for graceful shutdown (PebbleDB FlushAll + Close)
pkill -f "simple_chain" 2>/dev/null || true
pkill -f "metanode start" 2>/dev/null || true
pkill -f "metanode run" 2>/dev/null || true

# Smart wait: poll up to 15s for processes to exit gracefully
# PebbleDB needs time to flush ~32 shard memtables to SST + fsync
MAX_WAIT=15
for i in $(seq 1 $MAX_WAIT); do
    if ! pgrep -f "simple_chain" > /dev/null 2>&1 && ! pgrep -f "metanode start" > /dev/null 2>&1; then
        echo -e "${GREEN}  ✅ All processes exited gracefully in ${i}s${NC}"
        break
    fi
    if [ "$i" -eq "$MAX_WAIT" ]; then
        echo -e "${YELLOW}  ⚠️ Processes still running after ${MAX_WAIT}s, sending SIGKILL...${NC}"
    fi
    sleep 1
done

for id in 0 1 2 3 4; do
    tmux kill-session -t "go-master-$id" 2>/dev/null || true
    tmux kill-session -t "go-sub-$id" 2>/dev/null || true
    tmux kill-session -t "metanode-$id" 2>/dev/null || true
done
pkill -9 -f "simple_chain" 2>/dev/null || true
pkill -9 -f "metanode start" 2>/dev/null || true
rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true
echo -e "${GREEN}  ✅ All processes stopped${NC}"

# ==============================================================================
# STEP 2: BUILD
# ==============================================================================
if $SKIP_BUILD; then
    echo -e "${YELLOW}[2/5] ⏭️  Skipping build (--no-build)${NC}"
else
    echo -e "${BLUE}[2/5] 🔨 Building binaries...${NC}"
    
    # Rust
    echo "  🦀 Building Rust metanode..."
    export PATH="/home/abc/protoc3/bin:$PATH"
    cd "$METANODE_ROOT" && cargo +nightly build --release --bin metanode
    echo -e "${GREEN}  ✅ Rust binary ready${NC}"
    
    # C++ MVM
    echo "  ⚙️  Building C++ MVM..."
    MVM_ROOT="$GO_PROJECT_ROOT/pkg/mvm"
    if [ -d "$MVM_ROOT/c_mvm" ]; then
        mkdir -p "$MVM_ROOT/c_mvm/build" && cd "$MVM_ROOT/c_mvm/build"
        [ ! -f Makefile ] && cmake ../
        make -j$(nproc) install 2>&1 | tail -1
        mkdir -p "$MVM_ROOT/linker/build" && cd "$MVM_ROOT/linker/build"
        [ ! -f Makefile ] && cmake ..
        make -j$(nproc) install 2>&1 | tail -1
        echo -e "${GREEN}  ✅ C++ MVM ready${NC}"
    else
        echo -e "${YELLOW}  ⚠️ C++ MVM not found, skipping${NC}"
    fi
    
    # Go
    echo "  🐹 Building Go simple_chain..."
    cd "$GO_SIMPLE_ROOT" && GOTOOLCHAIN=go1.23.5 go build -o simple_chain . 2>&1 | tail -3
    echo -e "${GREEN}  ✅ Go binary ready${NC}"
fi

# ==============================================================================
# STEP 3: CLEAN DATA
# ==============================================================================
if $KEEP_DATA; then
    echo -e "${YELLOW}[3/5] ⏭️  Keeping existing data (--keep-data)${NC}"
else
    echo -e "${BLUE}[3/5] 🗑️  Cleaning all data...${NC}"
    for i in "${!NODES[@]}"; do
        DATA="${GO_DATA_DIR[$i]}"
        rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data" 2>/dev/null || true
        rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/data-write" 2>/dev/null || true
        rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up" 2>/dev/null || true
        rm -rf "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write" 2>/dev/null || true
    done
    rm -rf "$GO_SIMPLE_ROOT/snapshot_data"* 2>/dev/null || true
    rm -rf "$METANODE_ROOT/config/storage" 2>/dev/null || true
    rm -rf "$LOG_DIR" 2>/dev/null || true
    rm -f /tmp/epoch_data_backup*.json 2>/dev/null || true
    
    # Recreate directories
    for i in "${!NODES[@]}"; do
        id=${NODES[$i]}; DATA="${GO_DATA_DIR[$i]}"
        mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data/data/xapian_node"
        mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/data-write/data/xapian_node"
        mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up"
        mkdir -p "$GO_SIMPLE_ROOT/sample/$DATA/back_up_write"
        mkdir -p "$LOG_DIR/node_$id"
    done
    mkdir -p "$METANODE_ROOT/config/storage"
    echo -e "${GREEN}  ✅ Data cleaned${NC}"
fi

# ==============================================================================
# STEP 4: START RUST METANODES
# ==============================================================================
echo -e "${BLUE}[4/5] 🦀 Starting Rust metanodes...${NC}"
cd "$METANODE_ROOT"

for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    echo -e "  🚀 Rust Node $id..."
    tmux new-session -d -s "${RUST_SESSION[$i]}" -c "$METANODE_ROOT" \
        "ulimit -n 100000; export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; export METANODE_STATE_SOCK='/tmp/metanode-state-'$id'.sock'; $BINARY start --config ${RUST_CONFIG[$i]} >> \"$LOG_DIR/node_$id/rust.log\" 2>&1"
    sleep 1
done
sleep 3

# Wait for Rust State sockets to be ready before starting Go
for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    wait_for_socket "/tmp/metanode-state-$id.sock" "Rust JMT State $id" 120
done

# ==============================================================================
# STEP 5: START GO MASTERS + SUBS
# ==============================================================================
echo -e "${BLUE}[5/5] 🐹 Starting Go processes...${NC}"
cd "$GO_SIMPLE_ROOT"

for i in "${!NODES[@]}"; do
    id=${NODES[$i]}; DATA="${GO_DATA_DIR[$i]}"
    XAPIAN="sample/$DATA/data/data/xapian_node"
    if [ "$id" -eq "0" ]; then
        PPROF_ARG="--pprof-addr=localhost:6060"
    else
        PPROF_ARG="--pprof-addr="
    fi
    
    echo -e "  🚀 Go Master $id..."
    tmux new-session -d -s "${GO_MASTER_SESSION[$i]}" -c "$GO_SIMPLE_ROOT" \
        "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN' && export METANODE_STATE_SOCK='/tmp/metanode-state-'$id'.sock' && export METANODE_JMT_STATE_PATH='$METANODE_ROOT/config/storage/node_'$id'/jmt_state' && ./simple_chain -config=${GO_MASTER_CONFIG[$i]} $PPROF_ARG >> \"$LOG_DIR/node_$id/go-master-stdout.log\" 2>&1"
    sleep 2
done

# Wait for Go Master sockets
for i in "${!NODES[@]}"; do
    wait_for_socket "${GO_MASTER_SOCKET[$i]}" "Go Master ${NODES[$i]}" 120
done

# Start Go Subs
for i in "${!NODES[@]}"; do
    id=${NODES[$i]}; DATA="${GO_DATA_DIR[$i]}"
    XAPIAN="sample/$DATA/data-write/data/xapian_node"
    
    echo -e "  🚀 Go Sub $id..."
    tmux new-session -d -s "${GO_SUB_SESSION[$i]}" -c "$GO_SIMPLE_ROOT" \
        "ulimit -n 100000; export GOTOOLCHAIN=go1.23.5 && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='$XAPIAN' && export METANODE_STATE_SOCK='/tmp/metanode-state-'$id'.sock' && export METANODE_JMT_STATE_PATH='$METANODE_ROOT/config/storage/node_'$id'/jmt_state' && ./simple_chain -config=${GO_SUB_CONFIG[$i]} >> \"$LOG_DIR/node_$id/go-sub-stdout.log\" 2>&1"
    sleep 1
done
sleep 3

# ==============================================================================
# DONE
# ==============================================================================
ELAPSED=$(( $(date +%s) - START_TIME ))
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🎉 ALL DONE in ${ELAPSED}s — $(date '+%H:%M:%S')${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""
echo -e "  ${BLUE}Nodes:${NC}"
for i in "${!NODES[@]}"; do
    id=${NODES[$i]}
    echo "    Node $id: tmux attach -t metanode-$id | go-master-$id | go-sub-$id"
done
echo ""
echo -e "  ${BLUE}Logs:${NC}    $LOG_DIR/node_N/"
echo -e "  ${BLUE}Status:${NC}  tmux ls"
echo ""
