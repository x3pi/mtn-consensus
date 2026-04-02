#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  REMOTE LOG VIEWER — Multi-machine cluster                        ║
# ║  Dựa trên mtn-orchestrator.sh logs, nhưng qua SSH                ║
# ║                                                                   ║
# ║  Usage:                                                           ║
# ║    ./deploy_logs.sh --env deploy-3machines.env <node> [layer]     ║
# ║    ./deploy_logs.sh --env deploy-3machines.env status             ║
# ║    ./deploy_logs.sh --env deploy-3machines.env all                ║
# ║                                                                   ║
# ║  Layer: master | sub | rust | all (default: all)                  ║
# ║                                                                   ║
# ║  Examples:                                                        ║
# ║    ./deploy_logs.sh --env deploy-3machines.env 0 rust             ║
# ║    ./deploy_logs.sh --env deploy-3machines.env 2 master           ║
# ║    ./deploy_logs.sh --env deploy-3machines.env status             ║
# ║    ./deploy_logs.sh --env deploy-3machines.env all                ║
# ╚═══════════════════════════════════════════════════════════════════╝

set -uo pipefail

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ─── Parse --env argument ────────────────────────────────────────────
CUSTOM_ENV=""
ARGS=("$@")
for i in "${!ARGS[@]}"; do
    arg="${ARGS[$i]}"
    case "$arg" in
        --env=*) CUSTOM_ENV="${arg#--env=}" ;;
        --env)
            next=$((i+1))
            if [ "$next" -lt "${#ARGS[@]}" ]; then
                CUSTOM_ENV="${ARGS[$next]}"
            fi ;;
    esac
done

# Resolve env file
if [ -n "${CUSTOM_ENV}" ]; then
    [[ "$CUSTOM_ENV" != /* ]] && CUSTOM_ENV="$SCRIPT_DIR/$CUSTOM_ENV"
    ENV_FILE="$CUSTOM_ENV"
else
    ENV_FILE="${ENV_FILE:-$SCRIPT_DIR/deploy.env}"
fi

if [ ! -f "$ENV_FILE" ]; then
    echo -e "${RED}❌ Config not found: $ENV_FILE${NC}"
    echo -e "   Usage: ./deploy_logs.sh --env deploy-3machines.env <node|status|all> [layer]"
    exit 1
fi

source "$ENV_FILE"

# ─── SSH helpers ─────────────────────────────────────────────────────
ssh_cmd() {
    local host="$1"; shift
    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" ssh $SSH_OPTS "${SSH_USER}@${host}" "$@"
    elif [ -n "${SSH_KEY:-}" ]; then
        ssh $SSH_OPTS -i "$SSH_KEY" "${SSH_USER}@${host}" "$@"
    else
        ssh $SSH_OPTS "${SSH_USER}@${host}" "$@"
    fi
}

get_server_for_node() {
    echo "${NODE_SERVER[$1]}"
}

log_dir_for_node() {
    local node_id="$1"
    echo "${REMOTE_METANODE}/logs/node_${node_id}"
}

# ─── Parse command args (skip --env and its value) ──────────────────
CLEAN_ARGS=()
skip_next=false
for arg in "${ARGS[@]}"; do
    if $skip_next; then skip_next=false; continue; fi
    case "$arg" in
        --env=*) ;;
        --env) skip_next=true ;;
        *) CLEAN_ARGS+=("$arg") ;;
    esac
done

COMMAND="${CLEAN_ARGS[0]:-help}"
ARG2="${CLEAN_ARGS[1]:-}"
ARG3="${CLEAN_ARGS[2]:-all}"

# ═══════════════════════════════════════════════════════════════════
#  COMMAND: status — Hiển thị trạng thái tất cả các máy
# ═══════════════════════════════════════════════════════════════════
cmd_status() {
    echo ""
    echo -e "${BOLD}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  📊 REMOTE CLUSTER STATUS                                       ║${NC}"
    echo -e "${BOLD}╠══════════════════════════════════════════════════════════════════╣${NC}"

    declare -A SERVER_NODES
    for node_id in "${!NODE_SERVER[@]}"; do
        server="${NODE_SERVER[$node_id]}"
        SERVER_NODES[$server]="${SERVER_NODES[$server]:-} $node_id"
    done

    for server in $(echo "${NODE_SERVER[@]}" | tr ' ' '\n' | sort -u); do
        nodes="${SERVER_NODES[$server]}"
        echo -e "${BOLD}║  📍 $server — Node(s):${nodes}${NC}"
        echo -e "${BOLD}╠──────────────────────────────────────────────────────────────────╣${NC}"

        for node_id in $nodes; do
            LOG_D="${REMOTE_METANODE}/logs/node_${node_id}"
            result=$(ssh_cmd "$server" "
                rust_s='❌ DOWN'
                master_s='❌ DOWN'
                sub_s='❌ DOWN'
                tmux list-sessions 2>/dev/null | grep -q 'metanode-${node_id}' && rust_s='✅ RUNNING'
                tmux list-sessions 2>/dev/null | grep -q 'go-master-${node_id}' && master_s='✅ RUNNING'
                tmux list-sessions 2>/dev/null | grep -q 'go-sub-${node_id}' && sub_s='✅ RUNNING'

                # Lấy dòng log cuối
                rust_last=\$(tail -1 '${LOG_D}/rust.log' 2>/dev/null | cut -c1-60 || echo '(no log)')
                master_last=\$(tail -1 '${LOG_D}/go-master-stdout.log' 2>/dev/null | cut -c1-60 || echo '(no log)')

                echo \"Rust=\$rust_s | Master=\$master_s | Sub=\$sub_s\"
                echo \"  Rust last: \$rust_last\"
                echo \"  Master last: \$master_last\"
            " 2>/dev/null || echo "  ⚠️  SSH failed")
            echo -e "  ${CYAN}Node $node_id:${NC} $result"
        done
        echo -e "${BOLD}╠══════════════════════════════════════════════════════════════════╣${NC}"
    done
    echo -e "${BOLD}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${GREEN}Xem log chi tiết:${NC}"
    echo -e "    ${CYAN}./deploy_logs.sh --env $CUSTOM_ENV 0 rust${NC}     # Log Rust node 0"
    echo -e "    ${CYAN}./deploy_logs.sh --env $CUSTOM_ENV 0 master${NC}   # Log Go Master node 0"
    echo -e "    ${CYAN}./deploy_logs.sh --env $CUSTOM_ENV 2 rust${NC}     # Log Rust node 2 (máy 233)"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  COMMAND: all — Tail tất cả nodes cùng lúc (background + merge)
# ═══════════════════════════════════════════════════════════════════
cmd_all() {
    local layer="${ARG2:-rust}"
    echo ""
    echo -e "${BOLD}📋 Tailing ${layer} logs từ tất cả nodes... (Ctrl+C để dừng)${NC}"
    echo ""

    declare -A SERVER_NODES
    for node_id in "${!NODE_SERVER[@]}"; do
        server="${NODE_SERVER[$node_id]}"
        SERVER_NODES[$server]="${SERVER_NODES[$server]:-} $node_id"
    done

    # Chạy song song: mỗi SSH session tail log và prefix với [node-X]
    PIDS=()
    for server in $(echo "${NODE_SERVER[@]}" | tr ' ' '\n' | sort -u); do
        nodes="${SERVER_NODES[$server]}"
        for node_id in $nodes; do
            LOG_D="${REMOTE_METANODE}/logs/node_${node_id}"
            case "$layer" in
                master) LOG_FILE="$LOG_D/go-master-stdout.log" ;;
                sub)    LOG_FILE="$LOG_D/go-sub-stdout.log" ;;
                rust)   LOG_FILE="$LOG_D/rust.log" ;;
                evm)    LOG_FILE="$LOG_D/mvm_cpp_master-node${node_id}.log" ;; # Try master first
                *)      LOG_FILE="$LOG_D/rust.log" ;;
            esac

            # Fallback for EVM log if master log doesn't exist, try sub log
            if [ "$layer" = "evm" ]; then
                ssh_cmd "$server" "test -f '$LOG_FILE' || test -f '$LOG_D/mvm_cpp_sub-node${node_id}.log'" >/dev/null 2>&1
                if [ $? -ne 0 ]; then
                     # This check helps, it could be either. Let tail -f handle the exact missing file, but we point to master by default.
                     LOG_FILE="$LOG_D/mvm_cpp_master-node${node_id}.log"
                     # Also append sub evm log just in case
                     LOG_FILE_SUB="$LOG_D/mvm_cpp_sub-node${node_id}.log"
                else
                     LOG_FILE_SUB="$LOG_D/mvm_cpp_sub-node${node_id}.log"
                fi
            fi

            # Tail và prefix từng dòng với [node-X ip]
            (
                if [ "$layer" = "evm" ]; then
                     ssh_cmd "$server" "tail -f '$LOG_FILE' '$LOG_FILE_SUB' 2>/dev/null || tail -f '$LOG_FILE' 2>&1" \
                     | while IFS= read -r line; do
                         echo -e "${CYAN}[node-${node_id} ${server}]${NC} $line"
                     done
                else
                     ssh_cmd "$server" "tail -f '$LOG_FILE' 2>/dev/null || tail -f '$LOG_FILE' 2>&1" \
                     | while IFS= read -r line; do
                         echo -e "${CYAN}[node-${node_id} ${server}]${NC} $line"
                     done
                fi
            ) &
            PIDS+=($!)
        done
    done

    echo -e "${YELLOW}  Đang theo dõi ${#PIDS[@]} log streams... Ctrl+C để dừng${NC}"
    trap "kill ${PIDS[*]} 2>/dev/null; exit 0" INT TERM
    wait
}

# ═══════════════════════════════════════════════════════════════════
#  COMMAND: <node_id> [layer] — Tail log 1 node cụ thể
# ═══════════════════════════════════════════════════════════════════
cmd_node_log() {
    local node_id="$COMMAND"
    local layer="$ARG2"
    local server
    server=$(get_server_for_node "$node_id")
    local LOG_D="${REMOTE_METANODE}/logs/node_${node_id}"

    if [ -z "${NODE_SERVER[$node_id]+_}" ]; then
        echo -e "${RED}❌ Node $node_id không tồn tại trong config${NC}"
        exit 1
    fi

    echo ""
    echo -e "${BOLD}📋 Log Node ${node_id} [${layer}] trên ${server} — Ctrl+C để dừng${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""

    case "$layer" in
        master)
            echo -e "${BOLD}  → Go Master: $LOG_D/go-master-stdout.log${NC}"
            ssh_cmd "$server" "tail -f '$LOG_D/go-master-stdout.log' 2>/dev/null || echo 'Log chưa tồn tại'"
            ;;
        sub)
            echo -e "${BOLD}  → Go Sub: $LOG_D/go-sub-stdout.log${NC}"
            ssh_cmd "$server" "tail -f '$LOG_D/go-sub-stdout.log' 2>/dev/null || echo 'Log chưa tồn tại'"
            ;;
        rust)
            echo -e "${BOLD}  → Rust: $LOG_D/rust.log${NC}"
            ssh_cmd "$server" "tail -f '$LOG_D/rust.log' 2>/dev/null || echo 'Log chưa tồn tại'"
            ;;
        evm)
            echo -e "${BOLD}  → EVM (C++): $LOG_D/mvm_cpp_...${NC}"
            ssh_cmd "$server" "tail -f $LOG_D/mvm_cpp_master-node${node_id}.log $LOG_D/mvm_cpp_sub-node${node_id}.log 2>/dev/null || echo 'Log chưa tồn tại'"
            ;;
        all|*)
            echo -e "${BOLD}  → Tất cả layers (master + sub + rust + evm)${NC}"
            ssh_cmd "$server" "
                tail -f \
                    '$LOG_D/go-master-stdout.log' \
                    '$LOG_D/go-sub-stdout.log' \
                    '$LOG_D/rust.log' \
                    '$LOG_D/mvm_cpp_master-node${node_id}.log' \
                    '$LOG_D/mvm_cpp_sub-node${node_id}.log' \
                    2>/dev/null || echo 'Log chưa tồn tại'
            "
            ;;
    esac
}

# ═══════════════════════════════════════════════════════════════════
#  COMMAND: tmux — Attach vào tmux session của node cụ thể
# ═══════════════════════════════════════════════════════════════════
cmd_tmux() {
    local node_id="$ARG2"
    local layer="${ARG3:-rust}"
    local server
    server=$(get_server_for_node "$node_id")

    case "$layer" in
        master) SESSION="go-master-${node_id}" ;;
        sub)    SESSION="go-sub-${node_id}" ;;
        rust)   SESSION="metanode-${node_id}" ;;
        *)      SESSION="metanode-${node_id}" ;;
    esac

    echo -e "${BOLD}🖥  Attach tmux session '${SESSION}' trên ${server}${NC}"
    echo -e "${YELLOW}  (Dùng Ctrl+B D để detach khỏi tmux)${NC}"
    echo ""

    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" ssh $SSH_OPTS -t "${SSH_USER}@${server}" "tmux attach -t '$SESSION' || tmux ls"
    elif [ -n "${SSH_KEY:-}" ]; then
        ssh $SSH_OPTS -i "$SSH_KEY" -t "${SSH_USER}@${server}" "tmux attach -t '$SESSION' || tmux ls"
    else
        ssh $SSH_OPTS -t "${SSH_USER}@${server}" "tmux attach -t '$SESSION' || tmux ls"
    fi
}

# ═══════════════════════════════════════════════════════════════════
#  COMMAND: last — Xem N dòng cuối log (không follow)
# ═══════════════════════════════════════════════════════════════════
cmd_last() {
    local node_id="$ARG2"
    local layer="${ARG3:-rust}"
    local lines="${CLEAN_ARGS[3]:-50}"
    local server
    server=$(get_server_for_node "$node_id")
    local LOG_D="${REMOTE_METANODE}/logs/node_${node_id}"

    echo ""
    echo -e "${BOLD}📋 Last ${lines} lines — Node ${node_id} [${layer}] trên ${server}${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

    case "$layer" in
        master) ssh_cmd "$server" "tail -n $lines '$LOG_D/go-master-stdout.log' 2>/dev/null || echo 'Log không tồn tại'" ;;
        sub)    ssh_cmd "$server" "tail -n $lines '$LOG_D/go-sub-stdout.log' 2>/dev/null || echo 'Log không tồn tại'" ;;
        rust)   ssh_cmd "$server" "tail -n $lines '$LOG_D/rust.log' 2>/dev/null || echo 'Log không tồn tại'" ;;
        evm)    ssh_cmd "$server" "tail -n $lines $LOG_D/mvm_cpp_master-node${node_id}.log $LOG_D/mvm_cpp_sub-node${node_id}.log 2>/dev/null || echo 'Log không tồn tại'" ;;
        *)      ssh_cmd "$server" "tail -n $lines '$LOG_D/rust.log' 2>/dev/null || echo 'Log không tồn tại'" ;;
    esac
}

# ═══════════════════════════════════════════════════════════════════
#  HELP
# ═══════════════════════════════════════════════════════════════════
cmd_help() {
    echo ""
    echo -e "${BOLD}deploy_logs.sh — Xem log cluster từ xa${NC}"
    echo ""
    echo -e "  ${CYAN}Trạng thái & tổng quan:${NC}"
    echo -e "    ${BOLD}status${NC}                      Hiển thị trạng thái tất cả nodes"
    echo ""
    echo -e "  ${CYAN}Xem log 1 node cụ thể (follow/tail -f):${NC}"
    echo -e "    ${BOLD}<node_id> [layer]${NC}            Tail log realtime"
    echo -e "    Layer: ${BOLD}master${NC} | ${BOLD}sub${NC} | ${BOLD}rust${NC} | ${BOLD}evm${NC} | ${BOLD}all${NC} (default: all)"
    echo ""
    echo -e "  ${CYAN}Xem log tất cả nodes cùng lúc:${NC}"
    echo -e "    ${BOLD}all [layer]${NC}                  Merge logs tất cả nodes (prefix [node-X])"
    echo ""
    echo -e "  ${CYAN}Xem N dòng cuối (không follow):${NC}"
    echo -e "    ${BOLD}last <node_id> [layer] [N]${NC}   Xem N dòng cuối (default: 50)"
    echo ""
    echo -e "  ${CYAN}Attach tmux session (interactive):${NC}"
    echo -e "    ${BOLD}tmux <node_id> [layer]${NC}       Vào tmux session trực tiếp"
    echo ""
    echo -e "  ${CYAN}Ví dụ:${NC}"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env status"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env 0 rust      # Tail Rust node 0 (máy 234)"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env 0 master    # Tail Go Master node 0"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env 2 rust      # Tail Rust node 2 (máy 233)"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env 2 evm       # Tail EVM log node 2"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env 3 all       # Tail tất cả layer node 3 (máy 231)"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env all rust    # Tail Rust TẤT CẢ nodes cùng lúc"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env last 0 rust 100  # 100 dòng cuối node 0"
    echo -e "    ./deploy_logs.sh --env deploy-3machines.env tmux 1 master    # Vào tmux master node 1"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════
#  Entry point
# ═══════════════════════════════════════════════════════════════════
case "$COMMAND" in
    status)         cmd_status ;;
    all)            cmd_all ;;
    last)           cmd_last ;;
    tmux)           cmd_tmux ;;
    help|--help|-h) cmd_help ;;
    [0-9]|[0-9][0-9])  cmd_node_log ;;
    *)
        echo -e "${RED}❌ Lệnh không hợp lệ: $COMMAND${NC}"
        cmd_help
        exit 1
        ;;
esac
