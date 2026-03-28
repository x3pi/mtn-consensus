#!/bin/bash
# ═══════════════════════════════════════════════════════════════════
#  MTN Orchestrator — Quản lý khởi động/dừng toàn bộ cluster
#  Metanode (Rust Consensus + Go Master + Go Sub)
#
#  Thứ tự khởi động: Go Master → Go Sub → Rust Consensus
#  Thứ tự dừng:      Rust Consensus → Go Sub → Go Master
# ═══════════════════════════════════════════════════════════════════

set -euo pipefail

# ─── Đường dẫn gốc ───────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Script nằm ở: mtn-consensus/metanode/scripts/
# BASE_DIR = thư mục cha chứa cả mtn-consensus và mtn-simple-2025
BASE_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
GO_DIR="$BASE_DIR/mtn-simple-2025/cmd/simple_chain"
RUST_DIR="$BASE_DIR/mtn-consensus/metanode"
RUST_BIN="$RUST_DIR/target/release/metanode"
GO_BIN="$GO_DIR/simple_chain"
LOG_BASE="$RUST_DIR/logs"

# ─── Số lượng node ────────────────────────────────────────────────
NUM_NODES=5  # node 0..4

# ─── Màu sắc ─────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ─── Cấu hình mỗi node ──────────────────────────────────────────
# UDS Sockets
get_executor_sock()    { echo "/tmp/executor${1}.sock"; }
get_master_sock()      { echo "/tmp/rust-go-node${1}-master.sock"; }
get_sub_sock()         { echo "/tmp/rust-go-node${1}.sock"; }
get_tx_sock()          { echo "/tmp/metanode-tx-${1}.sock"; }

# Go Master XAPIAN paths
get_master_xapian()    { echo "sample/node${1}/data/data/xapian_node"; }
get_sub_xapian()       { echo "sample/node${1}/data-write/data/xapian_node"; }

# Go Master pprof (chỉ node 0 bật)
get_master_pprof() {
    if [ "$1" -eq 0 ]; then
        echo "localhost:6060"
    else
        echo ""
    fi
}

# ─── Timeout settings ────────────────────────────────────────────
SOCKET_TIMEOUT=30      # Chờ socket tối đa 30 giây
PROCESS_TIMEOUT=15     # Chờ process start tối đa 15 giây
SHUTDOWN_TIMEOUT=30    # Chờ process dừng tối đa 30s (Go StopWait=12s + FlushAll + CloseAll)
PHASE_DELAY=3          # Delay giữa các phase (giây)
NODE_DELAY=2           # Delay giữa các node trong cùng phase
RUST_DRAIN_WAIT=10     # Chờ sau khi Rust dừng để Go xử lý hết block trong pipeline
GO_FLUSH_WAIT=5        # Chờ sau khi Go nhận SIGTERM để flush xong disk

# ═══════════════════════════════════════════════════════════════════
#  Hàm tiện ích
# ═══════════════════════════════════════════════════════════════════

log_info()    { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*"; }
log_phase()   { echo -e "\n${CYAN}${BOLD}═══ $* ═══${NC}"; }
log_step()    { echo -e "  ${BLUE}►${NC} $*"; }

# Kiểm tra tmux session có tồn tại không
session_exists() {
    tmux has-session -t "$1" 2>/dev/null
}

# Lấy PID của process chính trong tmux session
get_session_pid() {
    local session="$1"
    if session_exists "$session"; then
        # Lấy PID của process đang chạy trong pane
        tmux list-panes -t "$session" -F '#{pane_pid}' 2>/dev/null | head -1
    fi
}

# Kiểm tra process còn sống không
is_process_alive() {
    local pid="$1"
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

# Chờ socket xuất hiện
wait_for_socket() {
    local sock="$1"
    local timeout="$2"
    local label="$3"
    local elapsed=0

    while [ ! -S "$sock" ] && [ $elapsed -lt $timeout ]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done

    if [ -S "$sock" ]; then
        log_info "  ✅ Socket sẵn sàng: $label ($sock) [${elapsed}s]"
        return 0
    else
        log_error "  ❌ Timeout chờ socket: $label ($sock) sau ${timeout}s"
        return 1
    fi
}

# Chờ TCP port lắng nghe
wait_for_port() {
    local port="$1"
    local timeout="$2"
    local label="$3"
    local elapsed=0

    while ! ss -tlnp 2>/dev/null | grep -q ":${port} " && [ $elapsed -lt $timeout ]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done

    if ss -tlnp 2>/dev/null | grep -q ":${port} "; then
        log_info "  ✅ Port sẵn sàng: $label (TCP :$port) [${elapsed}s]"
        return 0
    else
        log_error "  ❌ Timeout chờ port: $label (TCP :$port) sau ${timeout}s"
        return 1
    fi
}

# Chờ tmux session process sống
wait_for_session() {
    local session="$1"
    local timeout="$2"
    local elapsed=0

    while ! session_exists "$session" && [ $elapsed -lt $timeout ]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done

    if session_exists "$session"; then
        return 0
    else
        return 1
    fi
}

# Xóa socket cũ nếu tồn tại
cleanup_socket() {
    local sock="$1"
    if [ -e "$sock" ]; then
        rm -f "$sock"
        log_step "Xóa socket cũ: $sock"
    fi
}

# ═══════════════════════════════════════════════════════════════════
#  KHỞI ĐỘNG 1 NODE
# ═══════════════════════════════════════════════════════════════════

start_go_master() {
    local node_id=$1
    local session="go-master-${node_id}"
    local config="config-master-node${node_id}.json"
    local log_dir="$LOG_BASE/node_${node_id}"
    local log_file="$log_dir/go-master-stdout.log"
    local xapian_path=$(get_master_xapian $node_id)
    local pprof=$(get_master_pprof $node_id)

    if session_exists "$session"; then
        log_warn "Session $session đã tồn tại — bỏ qua"
        return 0
    fi

    mkdir -p "$log_dir"

    local pprof_flag=""
    if [ -n "$pprof" ]; then
        pprof_flag="--pprof-addr=$pprof"
    else
        pprof_flag="--pprof-addr="
    fi

    local cmd="ulimit -n 100000; "
    cmd+="export GOTOOLCHAIN=go1.23.5 && "
    cmd+="export GOMEMLIMIT=4GiB && "
    cmd+="export XAPIAN_BASE_PATH='${xapian_path}' && "
    cmd+="exec ./simple_chain -config=${config} ${pprof_flag} "
    cmd+=">> \"${log_file}\" 2>&1"

    tmux new-session -d -s "$session" -c "$GO_DIR" "$cmd"
    log_step "Go Master node${node_id} → session ${BOLD}${session}${NC}"
}

start_go_sub() {
    local node_id=$1
    local session="go-sub-${node_id}"
    local config="config-sub-node${node_id}.json"
    local log_dir="$LOG_BASE/node_${node_id}"
    local log_file="$log_dir/go-sub-stdout.log"
    local xapian_path=$(get_sub_xapian $node_id)

    if session_exists "$session"; then
        log_warn "Session $session đã tồn tại — bỏ qua"
        return 0
    fi

    # Kiểm tra config sub có tồn tại không
    if [ ! -f "$GO_DIR/$config" ]; then
        log_warn "Config $config không tồn tại — bỏ qua Sub node${node_id}"
        return 0
    fi

    mkdir -p "$log_dir"

    local cmd="ulimit -n 100000; "
    cmd+="export GOTOOLCHAIN=go1.23.5 && "
    cmd+="export GOMEMLIMIT=4GiB && "
    cmd+="export XAPIAN_BASE_PATH='${xapian_path}' && "
    cmd+="exec ./simple_chain -config=${config} "
    cmd+=">> \"${log_file}\" 2>&1"

    tmux new-session -d -s "$session" -c "$GO_DIR" "$cmd"
    log_step "Go Sub    node${node_id} → session ${BOLD}${session}${NC}"
}

start_rust() {
    local node_id=$1
    local session="metanode-${node_id}"
    local config="config/node_${node_id}.toml"
    local log_dir="$LOG_BASE/node_${node_id}"
    local log_file="$log_dir/rust.log"

    if session_exists "$session"; then
        log_warn "Session $session đã tồn tại — bỏ qua"
        return 0
    fi

    # Kiểm tra config rust có tồn tại không
    if [ ! -f "$RUST_DIR/$config" ]; then
        log_warn "Config $config không tồn tại — bỏ qua Rust node${node_id}"
        return 0
    fi

    mkdir -p "$log_dir"

    local cmd="ulimit -n 100000; "
    cmd+="export RUST_LOG=info,consensus_core=debug; "
    cmd+="export DB_WRITE_BUFFER_SIZE_MB=256; "
    cmd+="export DB_WAL_SIZE_MB=256; "
    cmd+="${RUST_BIN} start --config ${config} "
    cmd+=">> \"${log_file}\" 2>&1"

    tmux new-session -d -s "$session" -c "$RUST_DIR" "$cmd"
    log_step "Rust      node${node_id} → session ${BOLD}${session}${NC}"
}

# ═══════════════════════════════════════════════════════════════════
#  DỪNG 1 LAYER CHO 1 NODE
# ═══════════════════════════════════════════════════════════════════

# Dừng 1 tmux session an toàn
# Gửi SIGTERM → chờ process tự dừng (flush DB) → SIGKILL nếu timeout
stop_session() {
    local session="$1"
    local label="$2"

    if ! session_exists "$session"; then
        return 0
    fi

    # Tìm PID thực của binary (simple_chain hoặc metanode), không phải bash wrapper
    local real_pids=""
    if [[ "$session" == go-master-* ]]; then
        local node_id=${session#go-master-}
        real_pids=$(pgrep -f "simple_chain.*config-master-node${node_id}" 2>/dev/null || true)
    elif [[ "$session" == go-sub-* ]]; then
        local node_id=${session#go-sub-}
        real_pids=$(pgrep -f "simple_chain.*config-sub-node${node_id}" 2>/dev/null || true)
    elif [[ "$session" == metanode-* ]]; then
        local node_id=${session#metanode-}
        real_pids=$(pgrep -f "metanode start.*node_${node_id}.toml" 2>/dev/null || true)
    fi

    # Fallback: nếu không tìm được binary PID, dùng tmux pane PID + children
    if [ -z "$real_pids" ]; then
        local tmux_pid=$(get_session_pid "$session")
        if [ -n "$tmux_pid" ]; then
            real_pids="$tmux_pid $(pgrep -P "$tmux_pid" 2>/dev/null || true)"
        fi
    fi

    if [ -n "$real_pids" ]; then
        # Gửi SIGTERM — Go sẽ gọi app.Stop() → StopWait() → FlushAll() → CloseAll()
        for p in $real_pids; do
            kill -TERM "$p" 2>/dev/null || true
        done
        log_step "SIGTERM → $label (PIDs: $real_pids)"

        # Chờ process tự dừng (cần đủ thời gian để flush PebbleDB)
        local elapsed=0
        local still_running=true
        while [ $elapsed -lt $SHUTDOWN_TIMEOUT ] && $still_running; do
            still_running=false
            for p in $real_pids; do
                if kill -0 "$p" 2>/dev/null; then
                    still_running=true
                    break
                fi
            done
            if $still_running; then
                sleep 1
                elapsed=$((elapsed + 1))
                # Log tiến trình chờ mỗi 5 giây
                if [ $((elapsed % 5)) -eq 0 ]; then
                    log_step "  ⏳ Đang chờ $label flush dữ liệu... (${elapsed}s/${SHUTDOWN_TIMEOUT}s)"
                fi
            fi
        done

        if $still_running; then
            log_warn "  ⚠️  $label chưa dừng sau ${SHUTDOWN_TIMEOUT}s → SIGKILL (có thể mất dữ liệu!)"
            for p in $real_pids; do
                kill -KILL "$p" 2>/dev/null || true
            done
            sleep 1
        fi
    fi

    # Kill tmux session (cleanup shell wrapper)
    tmux kill-session -t "$session" 2>/dev/null || true
    log_info "  ✅ $label đã dừng"
}

# ═══════════════════════════════════════════════════════════════════
#  DỌN SẠCH SOCKET
# ═══════════════════════════════════════════════════════════════════

cleanup_all_sockets() {
    log_step "Dọn sạch socket cũ trong /tmp/..."
    for i in $(seq 0 $((NUM_NODES - 1))); do
        cleanup_socket "$(get_executor_sock $i)"
        cleanup_socket "$(get_master_sock $i)"
        cleanup_socket "$(get_sub_sock $i)"
        cleanup_socket "$(get_tx_sock $i)"
    done
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: START
# ═══════════════════════════════════════════════════════════════════

cmd_start() {
    local fresh=false
    local keep_data=false
    for arg in "$@"; do
        case "$arg" in
            --fresh)     fresh=true ;;
            --keep-data) keep_data=true ;;
        esac
    done

    echo ""
    echo -e "${BOLD}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  🚀 KHỞI ĐỘNG CLUSTER METANODE (${NUM_NODES} nodes)               ║${NC}"
    echo -e "${BOLD}║  Thứ tự: Go Master → Go Sub → Rust Consensus           ║${NC}"
    echo -e "${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"

    # Kiểm tra binary tồn tại
    if [ ! -f "$GO_BIN" ]; then
        log_error "Go binary không tồn tại: $GO_BIN"
        log_error "Chạy: cd $GO_DIR && go build -o simple_chain ."
        exit 1
    fi
    if [ ! -f "$RUST_BIN" ]; then
        log_error "Rust binary không tồn tại: $RUST_BIN"
        log_error "Chạy: cd $RUST_DIR && cargo +nightly build --release --bin metanode"
        exit 1
    fi

    # Kiểm tra có session cũ không
    local existing=0
    for i in $(seq 0 $((NUM_NODES - 1))); do
        session_exists "go-master-${i}" && existing=$((existing + 1))
        session_exists "go-sub-${i}" && existing=$((existing + 1))
        session_exists "metanode-${i}" && existing=$((existing + 1))
    done
    if [ $existing -gt 0 ]; then
        log_warn "Phát hiện $existing session cũ đang chạy!"
        if ! $fresh; then
            log_error "Dùng 'stop' trước hoặc thêm '--fresh' để dọn sạch và khởi động lại"
            exit 1
        fi
        log_warn "Chế độ --fresh: Dừng tất cả session cũ trước..."
        cmd_stop
        echo ""
    fi

    # Dọn socket cũ
    cleanup_all_sockets

    if $fresh; then
        log_phase "DỌN SẠCH DỮ LIỆU (--fresh)"
        log_step "Xóa Rust storage..."
        for i in $(seq 0 $((NUM_NODES - 1))); do
            rm -rf "$RUST_DIR/config/storage/node_${i}"
        done
        log_step "Xóa Go data..."
        for i in $(seq 0 $((NUM_NODES - 1))); do
            rm -rf "$GO_DIR/sample/node${i}/data"
            rm -rf "$GO_DIR/sample/node${i}/back_up"
            rm -rf "$GO_DIR/sample/node${i}/data-write"
            rm -rf "$GO_DIR/sample/node${i}/back_up_write"
        done
        log_info "✅ Dọn sạch hoàn tất"
    fi

    # ─── PHASE 1: Go Master ─────────────────────────────────────
    log_phase "PHASE 1/3: Khởi động Go Master (${NUM_NODES} node)"
    log_info "Go Master phải sẵn sàng TRƯỚC để nhận block từ Rust"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        start_go_master "$i"
        if [ $i -lt $((NUM_NODES - 1)) ]; then
            sleep "$NODE_DELAY"
        fi
    done

    # Chờ Go Master tạo UDS socket
    log_info "Chờ Go Master tạo UDS socket..."
    local master_ready=true
    for i in $(seq 0 $((NUM_NODES - 1))); do
        if ! wait_for_socket "$(get_master_sock $i)" "$SOCKET_TIMEOUT" "Go-Master-${i}"; then
            master_ready=false
        fi
    done

    if ! $master_ready; then
        log_error "Một số Go Master chưa sẵn sàng! Kiểm tra log."
        log_error "Tiếp tục khởi động nhưng có thể gây fork..."
    fi

    sleep "$PHASE_DELAY"

    # ─── PHASE 2: Go Sub ────────────────────────────────────────
    log_phase "PHASE 2/3: Khởi động Go Sub (${NUM_NODES} node)"
    log_info "Go Sub kết nối đến Go Master để nhận bản sao state"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        start_go_sub "$i"
        if [ $i -lt $((NUM_NODES - 1)) ]; then
            sleep "$NODE_DELAY"
        fi
    done

    sleep "$PHASE_DELAY"

    # ─── PHASE 3: Rust Consensus ────────────────────────────────
    log_phase "PHASE 3/3: Khởi động Rust Consensus (${NUM_NODES} node)"
    log_info "Rust bắt đầu đồng thuận và gửi block cho Go Master"

    # ─── CRASH SAFETY (Mar 2026): Xóa Rust executor_state trước khi start ──
    # executor_state/last_sent_index.bin lưu GEI đã gửi cho Go. Sau nhiều epochs,
    # DAG GC xóa commits cũ. Nếu restart, Rust dựa vào local_go_gei để resume.
    # Xóa file này an toàn vì Rust luôn query Go để lấy GEI hiện tại.
    # (Đã fix lỗi mixup Block vs GEI trong consensus_node.rs)
    log_step "Xóa Rust executor_state (tránh recovery gap)..."
    for i in $(seq 0 $((NUM_NODES - 1))); do
        rm -rf "$RUST_DIR/config/storage/node_${i}/executor_state"
    done

    for i in $(seq 0 $((NUM_NODES - 1))); do
        start_rust "$i"
        if [ $i -lt $((NUM_NODES - 1)) ]; then
            sleep "$NODE_DELAY"
        fi
    done

    # ─── KẾT QUẢ ────────────────────────────────────────────────
    echo ""
    echo -e "${GREEN}${BOLD}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}${BOLD}║  ✅ CLUSTER ĐÃ KHỞI ĐỘNG THÀNH CÔNG!                   ║${NC}"
    echo -e "${GREEN}${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  Kiểm tra trạng thái: ${BOLD}./mtn-orchestrator.sh status${NC}"
    echo -e "  Xem log node 0:     ${BOLD}./mtn-orchestrator.sh logs 0${NC}"
    echo -e "  Dừng cluster:       ${BOLD}./mtn-orchestrator.sh stop${NC}"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: STOP
# ═══════════════════════════════════════════════════════════════════

cmd_stop() {
    echo ""
    echo -e "${BOLD}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  🛑 DỪNG CLUSTER METANODE AN TOÀN                      ║${NC}"
    echo -e "${BOLD}║  Thứ tự: Rust Consensus → Go Sub → Go Master           ║${NC}"
    echo -e "${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"

    # ─── PHASE 1: Dừng Rust ─────────────────────────────────────
    log_phase "PHASE 1/3: Dừng Rust Consensus"
    log_info "Ngừng sản xuất block → Go sẽ không nhận block MỚI"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        stop_session "metanode-${i}" "Rust node${i}"
    done

    # ─── DRAIN WAIT: Chờ Go xử lý hết block còn trong pipeline ──
    log_info ""
    log_info "⏳ Chờ ${RUST_DRAIN_WAIT}s để Go Master xử lý hết block trong pipeline..."
    log_info "   (Go cần commit → broadcast → persist tất cả block còn trong hàng đợi)"
    local drain_count=0
    while [ $drain_count -lt $RUST_DRAIN_WAIT ]; do
        sleep 1
        drain_count=$((drain_count + 1))
        echo -ne "\r  ${BLUE}►${NC} Drain pipeline: ${drain_count}/${RUST_DRAIN_WAIT}s"
    done
    echo ""
    log_info "✅ Drain wait hoàn tất"

    # ─── PHASE 2: Dừng Go Sub ──────────────────────────────────
    log_phase "PHASE 2/3: Dừng Go Sub"
    log_info "Gửi SIGTERM → Go Sub sẽ flush bản sao state xuống disk"
    log_info "(Chờ tối đa ${SHUTDOWN_TIMEOUT}s cho mỗi node để StopWait + FlushAll + CloseAll)"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        stop_session "go-sub-${i}" "Go-Sub node${i}"
    done

    sleep $GO_FLUSH_WAIT

    # ─── PHASE 3: Dừng Go Master ───────────────────────────────
    log_phase "PHASE 3/3: Dừng Go Master"
    log_info "Gửi SIGTERM → Go Master sẽ: StopWait(12s) → FlushAll → CloseAll"
    log_info "(Chờ tối đa ${SHUTDOWN_TIMEOUT}s cho PebbleDB flush hoàn toàn xuống disk)"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        stop_session "go-master-${i}" "Go-Master node${i}"
    done

    sleep $GO_FLUSH_WAIT

    # Dọn socket
    cleanup_all_sockets

    echo ""
    echo -e "${GREEN}${BOLD}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}${BOLD}║  ✅ CLUSTER ĐÃ DỪNG AN TOÀN!                           ║${NC}"
    echo -e "${GREEN}${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"

    # Kiểm tra orphan process
    local orphans=$(pgrep -f "simple_chain.*config-" 2>/dev/null | wc -l)
    local rust_orphans=$(pgrep -f "metanode start" 2>/dev/null | wc -l)
    if [ $((orphans + rust_orphans)) -gt 0 ]; then
        log_warn "⚠️  Phát hiện ${orphans} Go + ${rust_orphans} Rust process orphan!"
        log_warn "   Dùng: pkill -f 'simple_chain.*config-' && pkill -f 'metanode start'"
    fi
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: RESTART
# ═══════════════════════════════════════════════════════════════════

cmd_restart() {
    cmd_stop
    echo ""
    log_info "Chờ ${PHASE_DELAY}s trước khi khởi động lại..."
    sleep "$PHASE_DELAY"
    cmd_start "$@"
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: STATUS
# ═══════════════════════════════════════════════════════════════════

cmd_status() {
    echo ""
    echo -e "${BOLD}╔════════════════════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  📊 TRẠNG THÁI CLUSTER METANODE                                              ║${NC}"
    echo -e "${BOLD}╠════════════════════════════════════════════════════════════════════════════════╣${NC}"
    printf "${BOLD}║  %-5s │ %-18s │ %-18s │ %-18s │ %-8s ║${NC}\n" \
        "Node" "Rust Consensus" "Go Master" "Go Sub" "Sockets"
    echo -e "${BOLD}╠════════════════════════════════════════════════════════════════════════════════╣${NC}"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        # Rust status
        local rust_status="${RED}❌ DOWN${NC}"
        if session_exists "metanode-${i}"; then
            local rust_pid=$(get_session_pid "metanode-${i}")
            # Tìm PID thực của metanode binary
            local real_rust_pid=$(pgrep -f "metanode start.*node_${i}.toml" 2>/dev/null | head -1)
            if [ -n "$real_rust_pid" ]; then
                rust_status="${GREEN}✅ ${real_rust_pid}${NC}"
            else
                rust_status="${YELLOW}⚠️ tmux  ${NC}"
            fi
        fi

        # Go Master status
        local master_status="${RED}❌ DOWN${NC}"
        if session_exists "go-master-${i}"; then
            local real_master_pid=$(pgrep -f "simple_chain.*config-master-node${i}" 2>/dev/null | head -1)
            if [ -n "$real_master_pid" ]; then
                master_status="${GREEN}✅ ${real_master_pid}${NC}"
            else
                master_status="${YELLOW}⚠️ tmux  ${NC}"
            fi
        fi

        # Go Sub status
        local sub_status="${RED}❌ DOWN${NC}"
        if session_exists "go-sub-${i}"; then
            local real_sub_pid=$(pgrep -f "simple_chain.*config-sub-node${i}" 2>/dev/null | head -1)
            if [ -n "$real_sub_pid" ]; then
                sub_status="${GREEN}✅ ${real_sub_pid}${NC}"
            else
                sub_status="${YELLOW}⚠️ tmux  ${NC}"
            fi
        fi

        # Socket status (kiểm tra executor socket + master socket)
        local sock_count=0
        [ -S "$(get_executor_sock $i)" ] && sock_count=$((sock_count + 1))
        [ -S "$(get_master_sock $i)" ] && sock_count=$((sock_count + 1))
        [ -S "$(get_tx_sock $i)" ] && sock_count=$((sock_count + 1))

        local sock_status="${RED}${sock_count}/3${NC}"
        if [ $sock_count -eq 3 ]; then
            sock_status="${GREEN}${sock_count}/3 ✅${NC}"
        elif [ $sock_count -gt 0 ]; then
            sock_status="${YELLOW}${sock_count}/3 ⚠️${NC}"
        fi

        printf "║  %-5s │ %-27b │ %-27b │ %-27b │ %-17b ║\n" \
            "  $i" "$rust_status" "$master_status" "$sub_status" "$sock_status"
    done

    echo -e "${BOLD}╚════════════════════════════════════════════════════════════════════════════════╝${NC}"

    # Tổng hợp
    local total_sessions=0
    local alive_sessions=0
    for i in $(seq 0 $((NUM_NODES - 1))); do
        for prefix in "go-master" "go-sub" "metanode"; do
            total_sessions=$((total_sessions + 1))
            if session_exists "${prefix}-${i}"; then
                alive_sessions=$((alive_sessions + 1))
            fi
        done
    done

    echo ""
    if [ $alive_sessions -eq $total_sessions ]; then
        echo -e "  ${GREEN}${BOLD}✅ Tất cả ${alive_sessions}/${total_sessions} session đang chạy${NC}"
    elif [ $alive_sessions -gt 0 ]; then
        echo -e "  ${YELLOW}${BOLD}⚠️  ${alive_sessions}/${total_sessions} session đang chạy${NC}"
    else
        echo -e "  ${RED}${BOLD}❌ Không có session nào đang chạy${NC}"
    fi
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: LOGS
# ═══════════════════════════════════════════════════════════════════

cmd_logs() {
    local node_id="${1:-}"
    if [ -z "$node_id" ]; then
        log_error "Cần chỉ định node_id: ./mtn-orchestrator.sh logs <0-4> [master|sub|rust]"
        exit 1
    fi

    local layer="${2:-all}"
    local log_dir="$LOG_BASE/node_${node_id}"

    case "$layer" in
        master)
            echo -e "${BOLD}📋 Log Go Master node${node_id}:${NC}"
            tail -f "$log_dir/go-master-stdout.log" 2>/dev/null || log_error "Log không tồn tại"
            ;;
        sub)
            echo -e "${BOLD}📋 Log Go Sub node${node_id}:${NC}"
            tail -f "$log_dir/go-sub-stdout.log" 2>/dev/null || log_error "Log không tồn tại"
            ;;
        rust)
            echo -e "${BOLD}📋 Log Rust node${node_id}:${NC}"
            tail -f "$log_dir/rust.log" 2>/dev/null || log_error "Log không tồn tại"
            ;;
        all)
            echo -e "${BOLD}📋 Log tất cả layers node${node_id}:${NC}"
            tail -f "$log_dir/go-master-stdout.log" "$log_dir/go-sub-stdout.log" "$log_dir/rust.log" 2>/dev/null || log_error "Log không tồn tại"
            ;;
        *)
            log_error "Layer không hợp lệ: $layer (chọn: master|sub|rust|all)"
            exit 1
            ;;
    esac
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: STOP-NODE (dừng 1 node cụ thể)
# ═══════════════════════════════════════════════════════════════════

cmd_stop_node() {
    local node_id="${1:-}"
    if [ -z "$node_id" ]; then
        log_error "Cần chỉ định node_id: ./mtn-orchestrator.sh stop-node <0-4>"
        exit 1
    fi

    echo ""
    echo -e "${BOLD}🛑 Dừng node ${node_id} (Rust → Sub → Master)${NC}"

    stop_session "metanode-${node_id}" "Rust node${node_id}"
    sleep 1
    stop_session "go-sub-${node_id}" "Go-Sub node${node_id}"
    sleep 1
    stop_session "go-master-${node_id}" "Go-Master node${node_id}"

    # Dọn socket
    cleanup_socket "$(get_executor_sock $node_id)"
    cleanup_socket "$(get_master_sock $node_id)"
    cleanup_socket "$(get_sub_sock $node_id)"
    cleanup_socket "$(get_tx_sock $node_id)"

    log_info "✅ Node ${node_id} đã dừng hoàn toàn"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: START-NODE (khởi động 1 node cụ thể)
# ═══════════════════════════════════════════════════════════════════

cmd_start_node() {
    local node_id="${1:-}"
    if [ -z "$node_id" ]; then
        log_error "Cần chỉ định node_id: ./mtn-orchestrator.sh start-node <0-4>"
        exit 1
    fi

    echo ""
    echo -e "${BOLD}🚀 Khởi động node ${node_id} (Master → Sub → Rust)${NC}"

    # Dọn socket cũ
    cleanup_socket "$(get_executor_sock $node_id)"
    cleanup_socket "$(get_master_sock $node_id)"
    cleanup_socket "$(get_sub_sock $node_id)"
    cleanup_socket "$(get_tx_sock $node_id)"

    # Phase 1: Go Master
    start_go_master "$node_id"
    log_info "Chờ Go Master node${node_id} tạo socket..."
    wait_for_socket "$(get_master_sock $node_id)" "$SOCKET_TIMEOUT" "Go-Master-${node_id}" || true
    sleep "$NODE_DELAY"

    # Phase 2: Go Sub
    start_go_sub "$node_id"
    sleep "$NODE_DELAY"

    # Phase 3: Rust
    start_rust "$node_id"

    log_info "✅ Node ${node_id} đã khởi động hoàn tất"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: RESTART-NODE (restart 1 node cụ thể)
# ═══════════════════════════════════════════════════════════════════

cmd_restart_node() {
    local node_id="${1:-}"
    if [ -z "$node_id" ]; then
        log_error "Cần chỉ định node_id: ./mtn-orchestrator.sh restart-node <0-4>"
        exit 1
    fi

    cmd_stop_node "$node_id"
    sleep 2
    cmd_start_node "$node_id"
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: HELP
# ═══════════════════════════════════════════════════════════════════

cmd_help() {
    echo ""
    echo -e "${BOLD}MTN Orchestrator — Quản lý cluster Metanode${NC}"
    echo ""
    echo -e "  ${CYAN}Toàn bộ cluster:${NC}"
    echo -e "    ${BOLD}start${NC}   [--fresh]         Khởi động cluster (Master→Sub→Rust)"
    echo -e "    ${BOLD}stop${NC}                      Dừng an toàn (Rust→Sub→Master)"
    echo -e "    ${BOLD}restart${NC} [--fresh]         Stop rồi start"
    echo -e "    ${BOLD}status${NC}                    Xem trạng thái tất cả node"
    echo ""
    echo -e "  ${CYAN}Từng node:${NC}"
    echo -e "    ${BOLD}start-node${NC}   <0-4>        Khởi động 1 node"
    echo -e "    ${BOLD}stop-node${NC}    <0-4>        Dừng 1 node"
    echo -e "    ${BOLD}restart-node${NC} <0-4>        Restart 1 node"
    echo ""
    echo -e "  ${CYAN}Tiện ích:${NC}"
    echo -e "    ${BOLD}logs${NC} <0-4> [master|sub|rust|all]   Xem log"
    echo -e "    ${BOLD}help${NC}                               Hiển thị hướng dẫn"
    echo ""
    echo -e "  ${CYAN}Ví dụ:${NC}"
    echo -e "    ./mtn-orchestrator.sh start --fresh    # Khởi động mới toàn bộ"
    echo -e "    ./mtn-orchestrator.sh restart-node 1   # Restart riêng node 1"
    echo -e "    ./mtn-orchestrator.sh logs 0 rust      # Xem log Rust node 0"
    echo -e "    ./mtn-orchestrator.sh stop              # Dừng an toàn"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  Entry point
# ═══════════════════════════════════════════════════════════════════

COMMAND="${1:-help}"
shift || true

case "$COMMAND" in
    start)        cmd_start "$@" ;;
    stop)         cmd_stop ;;
    restart)      cmd_restart "$@" ;;
    status)       cmd_status ;;
    logs)         cmd_logs "$@" ;;
    start-node)   cmd_start_node "$@" ;;
    stop-node)    cmd_stop_node "$@" ;;
    restart-node) cmd_restart_node "$@" ;;
    help|--help|-h) cmd_help ;;
    *)
        log_error "Lệnh không hợp lệ: $COMMAND"
        cmd_help
        exit 1
        ;;
esac
