#!/bin/bash
# ═══════════════════════════════════════════════════════════════════
#  MTN Orchestrator — Quản lý khởi động/dừng toàn bộ cluster
#  Metanode (Unified Binary: Go Master + Rust Consensus via FFI)
#
#  Kiến trúc hợp nhất (Apr 2026):
#    Go Master nhúng Rust Consensus trực tiếp qua CGo FFI.
#    Không còn process Rust riêng biệt.
#    Mỗi node chỉ có 1 process: go-master-N
#
#  Thứ tự khởi động: Go Master (tự khởi chạy Rust FFI bên trong)
#  Thứ tự dừng:      Go Master (SIGTERM → StopWait → Flush)
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
get_tx_sock()          { echo "/tmp/metanode-tx-${1}.sock"; }

# Go Master XAPIAN paths
get_master_xapian()    { echo "sample/node${1}/data/data/xapian_node"; }

# Go Master pprof (chỉ node 0 bật)
get_master_pprof() {
    if [ "$1" -eq 0 ]; then
        echo "localhost:6060"
    else
        echo ""
    fi
}

# ─── Timeout settings ────────────────────────────────────────────
SOCKET_TIMEOUT=90      # Chờ socket tối đa 90 giây
PROCESS_TIMEOUT=15     # Chờ process start tối đa 15 giây
SHUTDOWN_TIMEOUT=30    # Chờ process dừng tối đa 30s (Go StopWait=12s + FlushAll + CloseAll)
PHASE_DELAY=3          # Delay giữa các phase (giây)
NODE_DELAY=4           # Delay giữa các node (4s cho FFI startup overhead)
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
    cmd+="export RUST_BACKTRACE=full && "
    cmd+="export GOTRACEBACK=crash && "
    cmd+="export GOTOOLCHAIN=go1.23.5 && "
    cmd+="export GOMEMLIMIT=4GiB && "
    cmd+="export XAPIAN_BASE_PATH='${xapian_path}' && "
    cmd+="export MVM_LOG_DIR='${log_dir}' && "
    cmd+="exec ./simple_chain -config=${config} ${pprof_flag} "
    cmd+=">> \"${log_file}\" 2>&1"

    cd "$GO_DIR" && nohup bash -c "$cmd" &
    log_step "Go Master node${node_id} → process started via nohup"
}


start_rust() {
    local node_id=$1
    log_info "Rust Consensus Engine is now embedded in Go Master via FFI. Skipping standalone metanode execution for node${node_id}."
    return 0
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
        cleanup_socket "$(get_tx_sock $i)"
    done
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: START
# ═══════════════════════════════════════════════════════════════════

cmd_start() {
    local fresh=false
    local build_go=false
    local build_rust=false
    local build_evm=false
    local build_nomt=false
    local exclude_node="-1"
    local epoch_length=""
    
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --fresh) fresh=true; shift ;;
            --build) build_go=true; build_rust=true; shift ;;
            --build-all) build_go=true; build_rust=true; build_evm=true; build_nomt=true; shift ;;
            --exclude-node) exclude_node="$2"; shift 2 ;;
            --epoch-length) epoch_length="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    echo ""
    echo -e "${BOLD}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  🚀 KHỞI ĐỘNG CLUSTER METANODE (${NUM_NODES} nodes)               ║${NC}"
    echo -e "${BOLD}║  Kiến trúc hợp nhất: Go Master + Rust FFI              ║${NC}"
    echo -e "${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"

    # Auto-update IPs configuration back to localhost
    log_info "🔄 Cập nhật IP config về 127.0.0.1..."
    "$SCRIPT_DIR/node/update_ips.sh" 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 || true

    # Build processes
    if $build_nomt; then
        log_info "🛠  Đang build NOMT FFI (Rust)..."
        (cd "$BASE_DIR/mtn-simple-2025/pkg/nomt_ffi/rust_lib" && cargo +nightly build --release) || exit 1
    fi
    if $build_evm; then
        log_info "🛠  Đang build EVM (C++ MVM)..."
        (cd "$BASE_DIR/mtn-simple-2025/pkg/mvm" && chmod +x build.sh && ./build.sh) || exit 1
    fi
    if $build_go; then
        log_info "🛠  Đang build Go (simple_chain)..."
        # Dùng trình biên dịch tận dụng số luồng tối đa, bỏ cờ '-a' để dùng Build Cache (~2s thay vì 3 phút)
        (cd "$GO_DIR" && rm -f simple_chain && CGO_ENABLED=1 go env && CGO_ENABLED=1 go build -p $(nproc) -o simple_chain .) || exit 1
    fi
    if $build_rust; then
        log_info "🛠  Đang build Rust (metanode)..."
        (cd "$RUST_DIR" && cargo +nightly build --release --bin metanode) || exit 1
    fi

    # Kiểm tra binary tồn tại (chỉ cần Go, Rust nhúng via FFI)
    if [ ! -f "$GO_BIN" ]; then
        log_error "Go binary không tồn tại: $GO_BIN"
        log_error "Chạy: cd $GO_DIR && CGO_ENABLED=1 go build -o simple_chain ."
        exit 1
    fi

    # Kiểm tra có session cũ hoặc orphan process không
    local existing=0
    for i in $(seq 0 $((NUM_NODES - 1))); do
        session_exists "go-master-${i}" && existing=$((existing + 1))
        session_exists "metanode-${i}" && existing=$((existing + 1))
    done
    local orphans=$(pgrep -f "simple_chain.*config-" 2>/dev/null | wc -l)
    if [ $existing -gt 0 ] || [ $orphans -gt 0 ]; then
        log_warn "Phát hiện $existing session + $orphans orphan process đang chạy!"
        if ! $fresh; then
            log_error "Dùng 'stop' trước hoặc thêm '--fresh' để dọn sạch và khởi động lại"
            exit 1
        fi
        log_warn "Chế độ --fresh: Dừng tất cả session cũ + kill orphan processes..."
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
        log_step "Xóa Go data và snapshots..."
        for i in $(seq 0 $((NUM_NODES - 1))); do
            rm -rf "$GO_DIR/sample/node${i}/data"
            rm -rf "$GO_DIR/sample/node${i}/back_up"
            rm -rf "$GO_DIR/snapshot_data_node${i}"
        done
        log_step "Xóa logs..."
        for i in $(seq 0 $((NUM_NODES - 1))); do
            if [ "$i" = "$exclude_node" ]; then continue; fi
            rm -rf "$LOG_BASE/node_${i}" 2>/dev/null || true
        done
        log_info "✅ Dọn sạch hoàn tất"
    fi

    # ─── CRASH SAFETY (Mar 2026): Xóa Rust executor_state trước khi start ──
    # executor_state/last_sent_index.bin lưu GEI đã gửi cho Go. Sau nhiều epochs,
    # DAG GC xóa commits cũ. Nếu restart, Rust dựa vào local_go_gei để resume.
    # Xóa file này an toàn vì Rust luôn query Go để lấy GEI hiện tại.
    log_step "Xóa Rust executor_state (tránh recovery gap)..."
    for i in $(seq 0 $((NUM_NODES - 1))); do
        if [ "$i" = "$exclude_node" ]; then continue; fi
        rm -rf "$RUST_DIR/config/storage/node_${i}/executor_state"
    done

    # ─── PHASE 1: Go Master (nhúng Rust FFI) ────────────────────
    log_phase "PHASE 1/1: Khởi động Go Master + Rust FFI (${NUM_NODES} node)"
    log_info "Mỗi Go Master tự khởi chạy Rust Consensus Engine bên trong via FFI"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        if [ "$i" = "$exclude_node" ]; then continue; fi
        start_go_master "$i"
        if [ $i -lt $((NUM_NODES - 1)) ]; then
            sleep "$NODE_DELAY"
        fi
        # Health check: verify process is alive after startup
        local hc_pid=$(pgrep -f "simple_chain.*config-master-node${i}" 2>/dev/null | head -1)
        if [ -z "$hc_pid" ]; then
            log_warn "⚠️  Node ${i} process died during startup! Check logs: $LOG_BASE/node_${i}/go-master-stdout.log"
            # Retry once
            sleep 2
            if ! session_exists "go-master-${i}"; then
                log_warn "  ↻ Retrying node ${i}..."
                start_go_master "$i"
                sleep "$NODE_DELAY"
            fi
        fi
    done

    # Skiping UDS wait because Go Master and Rust now run as a monolithic engine via FFI.
    log_info "Skipping UDS socket wait (FFI replaces UDS)..."
    sleep "$PHASE_DELAY"

    # ─── Rust Consensus (nhúng trong Go Master via FFI) ─────────
    log_info "Rust Consensus Engine đã được nhúng trong Go Master via FFI"
    log_info "Không cần khởi động process Rust riêng biệt"

    # Executor state was moved up above to prevent RocksDB process abort

    if [ -n "$epoch_length" ]; then
        log_step "Ghi đè epoch_length=${epoch_length} vào file cấu hình (cho stress test)..."
        for i in $(seq 0 $((NUM_NODES - 1))); do
            if [ "$i" = "$exclude_node" ]; then continue; fi
            sed -i "s/^epoch_length = .*/epoch_length = ${epoch_length}/" "${RUST_DIR}/config/node_${i}.toml"
        done
    fi

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
    echo -e "${BOLD}║  Kiến trúc hợp nhất: chỉ cần dừng Go Master            ║${NC}"
    echo -e "${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"

    # ─── Dừng Go Master (Rust FFI tự dừng theo process cha) ────
    log_phase "Dừng Go Master + Rust FFI (${NUM_NODES} node)"
    log_info "Gửi SIGTERM → Go Master sẽ: StopWait(12s) → FlushAll → CloseAll"
    log_info "Rust Consensus Engine (nhúng FFI) sẽ dừng cùng process Go Master"
    log_info "(Chờ tối đa ${SHUTDOWN_TIMEOUT}s cho PebbleDB flush hoàn toàn xuống disk)"

    for i in $(seq 0 $((NUM_NODES - 1))); do
        stop_session "go-master-${i}" "Go-Master node${i}"
    done

    # Dọn session metanode cũ (nếu còn sót từ kiến trúc cũ)
    for i in $(seq 0 $((NUM_NODES - 1))); do
        if session_exists "metanode-${i}"; then
            stop_session "metanode-${i}" "Rust-legacy node${i}"
        fi
    done

    sleep $GO_FLUSH_WAIT

    # Dọn socket
    cleanup_all_sockets

    echo ""
    echo -e "${GREEN}${BOLD}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}${BOLD}║  ✅ CLUSTER ĐÃ DỪNG AN TOÀN!                           ║${NC}"
    echo -e "${GREEN}${BOLD}╚══════════════════════════════════════════════════════════╝${NC}"

    # Tự động kill orphan process (thay vì chỉ cảnh báo)
    local orphans=$(pgrep -f "simple_chain.*config-" 2>/dev/null | wc -l)
    if [ $orphans -gt 0 ]; then
        log_warn "⚠️  Phát hiện ${orphans} Go orphan process — đang kill..."
        pkill -TERM -f "simple_chain.*config-" 2>/dev/null || true
        sleep 3
        # Force kill nếu vẫn còn
        local remaining=$(pgrep -f "simple_chain.*config-" 2>/dev/null | wc -l)
        if [ $remaining -gt 0 ]; then
            log_warn "⚠️  Vẫn còn ${remaining} orphan → SIGKILL"
            pkill -KILL -f "simple_chain.*config-" 2>/dev/null || true
            sleep 1
        fi
        log_info "✅ Đã dọn sạch orphan processes"
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

get_rpc_port() {
    local node_id=$1
    case $node_id in
        0) echo "8757" ;;
        1) echo "10747" ;;
        2) echo "10749" ;;
        3) echo "10750" ;;
        4) echo "10748" ;;
        *) echo "8757" ;;
    esac
}

cmd_status() {
    echo ""
    echo -e "${BOLD}╔═════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  📊 TRẠNG THÁI CLUSTER METANODE (FFI hợp nhất)     ║${NC}"
    echo -e "${BOLD}╠═════════════════════════════════════════════════════╣${NC}"
    printf "${BOLD}║  %-5s │ %-38s ║${NC}\n" \
        "Node" "Go Master + Rust FFI"
    echo -e "${BOLD}╠═════════════════════════════════════════════════════╣${NC}"

    local total_nodes=$NUM_NODES
    local alive_nodes=0

    for i in $(seq 0 $((NUM_NODES - 1))); do
        local master_status="${RED}❌ DOWN${NC}"
        local real_pid=$(pgrep -f "simple_chain.*config-master-node${i}" 2>/dev/null | head -1)
        
        if [ -n "$real_pid" ]; then
            alive_nodes=$((alive_nodes + 1))
            local rpc_port=$(get_rpc_port "$i")
            local height_hex=$(curl -s --max-time 1 -X POST http://127.0.0.1:${rpc_port} \
                -H "Content-Type: application/json" \
                -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' 2>/dev/null \
                | grep -oP '"result":"(0x[0-9a-fA-F]+)"' | cut -d'"' -f4)
            
            local height_dec=""
            if [ -n "$height_hex" ]; then
                height_dec=$(printf "%d" "$height_hex" 2>/dev/null)
            fi
            
            if [ -n "$height_dec" ]; then
                master_status="${GREEN}✅ PID ${real_pid} (block #${height_dec})${NC}"
            else
                master_status="${GREEN}✅ PID ${real_pid} (block #?)${NC}"
            fi
        fi

        printf "║  %-5s │ %-47b ║\n" \
            "  $i" "$master_status"
    done

    echo -e "${BOLD}╚═════════════════════════════════════════════════════╝${NC}"

    echo ""
    if [ $alive_nodes -eq $total_nodes ]; then
        echo -e "  ${GREEN}${BOLD}✅ Tất cả ${alive_nodes}/${total_nodes} node đang chạy${NC}"
    elif [ $alive_nodes -gt 0 ]; then
        echo -e "  ${YELLOW}${BOLD}⚠️  ${alive_nodes}/${total_nodes} node đang chạy${NC}"
    else
        echo -e "  ${RED}${BOLD}❌ Không có node nào đang chạy${NC}"
    fi
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  LỆNH: LOGS
# ═══════════════════════════════════════════════════════════════════

cmd_logs() {
    local node_id="${1:-}"
    if [ -z "$node_id" ]; then
        log_error "Cần chỉ định node_id: ./mtn-orchestrator.sh logs <0-4>"
        exit 1
    fi

    local log_dir="$LOG_BASE/node_${node_id}"
    local log_file="$log_dir/go-master-stdout.log"

    echo -e "${BOLD}📋 Log Go Master + Rust FFI node${node_id}:${NC}"
    echo -e "   (Kiến trúc hợp nhất: Rust output nằm cùng stdout với Go)"
    tail -f "$log_file" 2>/dev/null || log_error "Log không tồn tại: $log_file"
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
    echo -e "${BOLD}🛑 Dừng node ${node_id} (Go Master + Rust FFI)${NC}"

    stop_session "go-master-${node_id}" "Go-Master+Rust node${node_id}"

    # Dọn session Rust cũ (nếu còn sót từ kiến trúc cũ)
    if session_exists "metanode-${node_id}"; then
        stop_session "metanode-${node_id}" "Rust-legacy node${node_id}"
    fi

    # Dọn socket
    cleanup_socket "$(get_executor_sock $node_id)"
    cleanup_socket "$(get_master_sock $node_id)"
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
    echo -e "${BOLD}🚀 Khởi động node ${node_id} (Go Master + Rust FFI)${NC}"

    # Dọn socket cũ
    cleanup_socket "$(get_executor_sock $node_id)"
    cleanup_socket "$(get_master_sock $node_id)"
    cleanup_socket "$(get_tx_sock $node_id)"

    # Xóa Rust executor_state (tránh recovery gap)
    rm -rf "$RUST_DIR/config/storage/node_${node_id}/executor_state"

    # Khởi động Go Master (Rust FFI tự động khởi chạy bên trong)
    start_go_master "$node_id"
    sleep "$NODE_DELAY"

    log_info "✅ Node ${node_id} đã khởi động hoàn tất (Go Master + Rust FFI)"
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
    echo -e "${CYAN}Kiến trúc hợp nhất: Go Master nhúng Rust Consensus via FFI${NC}"
    echo ""
    echo -e "  ${CYAN}Toàn bộ cluster:${NC}"
    echo -e "    ${BOLD}start${NC}   [--fresh] [--build] [--build-all]  Khởi động cluster"
    echo -e "    ${BOLD}stop${NC}                      Dừng an toàn"
    echo -e "    ${BOLD}restart${NC} [--fresh]         Stop rồi start"
    echo -e "    ${BOLD}status${NC}                    Xem trạng thái tất cả node"
    echo ""
    echo -e "  ${CYAN}Từng node:${NC}"
    echo -e "    ${BOLD}start-node${NC}   <0-4>        Khởi động 1 node"
    echo -e "    ${BOLD}stop-node${NC}    <0-4>        Dừng 1 node"
    echo -e "    ${BOLD}restart-node${NC} <0-4>        Restart 1 node"
    echo ""
    echo -e "  ${CYAN}Tiện ích:${NC}"
    echo -e "    ${BOLD}logs${NC} <0-4>                    Xem log (Go + Rust hợp nhất)"
    echo -e "    ${BOLD}help${NC}                           Hiển thị hướng dẫn"
    echo ""
    echo -e "  ${CYAN}Ví dụ:${NC}"
    echo -e "    ./mtn-orchestrator.sh start --fresh    # Khởi động mới toàn bộ"
    echo -e "    ./mtn-orchestrator.sh restart-node 1   # Restart riêng node 1"
    echo -e "    ./mtn-orchestrator.sh logs 0            # Xem log node 0"
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

# echo -e "${BLUE}📋 Step 9: Running SetGet test...${NC}"
# CLIENT_DIR="$HOME/nhat/client/cmd/client/call_tool_example_new"
# if [ -d "$CLIENT_DIR" ]; then
#     cd "$CLIENT_DIR"
#     echo -e "${GREEN}  🚀 Running: go run . -data=SetGet.json -config=config-local-genis.json${NC}"
#     # Run the command and pipe 3 enters to it
#     (sleep 2; echo ""; sleep 2; echo ""; sleep 2; echo "") | go run . -data=SetGet.json -config=config-local-genis.json
#     echo -e "${GREEN}  ✅ SetGet test completed${NC}"
# else
#     echo -e "${YELLOW}  ⚠️ Client directory not found: $CLIENT_DIR${NC}"
# fi
# echo ""
