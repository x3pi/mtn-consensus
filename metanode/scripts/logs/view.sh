#!/usr/bin/env bash
# ============================================================================
# Log Viewer for All Nodes — Rust & Go
# Usage: ./view.sh [node_id] [go|rust|all] [lines]
# Examples:
#   ./view.sh              # Show available logs
#   ./view.sh 0            # Tail all logs for node 0
#   ./view.sh 4 rust       # Tail Rust log for node 4
#   ./view.sh 0 go 200     # Last 200 lines of Go logs for node 0
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOGS_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)/logs"
LINES="${3:-50}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Find log files ────────────────────────────────────────────

rust_log() {
    echo "$LOGS_DIR/node_$1/rust.log"
}

go_master_log() {
    local node=$1
    local epoch_log=$(ls -td "$LOGS_DIR/node_$node"/go-master/epoch_*/App.log 2>/dev/null | head -1)
    if [ -n "$epoch_log" ]; then
        echo "$epoch_log"
    else
        echo "$LOGS_DIR/node_$node/go-master-stdout.log"
    fi
}

go_sub_log() {
    local node=$1
    local epoch_log=$(ls -td "$LOGS_DIR/node_$node"/go-sub/epoch_*/App.log 2>/dev/null | head -1)
    if [ -n "$epoch_log" ]; then
        echo "$epoch_log"
    else
        echo "$LOGS_DIR/node_$node/go-sub-stdout.log"
    fi
}

# ── List available logs ────────────────────────────────────────

list_logs() {
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║             Available Log Files (per node)                  ║${NC}"
    echo -e "${CYAN}╠══════════════════════════════════════════════════════════════╣${NC}"

    for node in 0 1 2 3 4; do
        echo -e "${CYAN}║${NC}"
        echo -e "${CYAN}║${NC}  ${YELLOW}Node $node:${NC}"

        local rlog=$(rust_log $node)
        if [ -f "$rlog" ]; then
            local size=$(du -h "$rlog" 2>/dev/null | cut -f1)
            echo -e "${CYAN}║${NC}    ${RED}Rust${NC}:       $rlog  ${GREEN}($size)${NC}"
        fi

        local gmaster=$(go_master_log $node)
        if [ -f "$gmaster" ]; then
            local size=$(du -h "$gmaster" 2>/dev/null | cut -f1)
            echo -e "${CYAN}║${NC}    ${BLUE}Go Master${NC}: $gmaster  ${GREEN}($size)${NC}"
        fi

        local gsub=$(go_sub_log $node)
        if [ -f "$gsub" ]; then
            local size=$(du -h "$gsub" 2>/dev/null | cut -f1)
            echo -e "${CYAN}║${NC}    ${BLUE}Go Sub${NC}:    $gsub  ${GREEN}($size)${NC}"
        fi
    done

    echo -e "${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "Usage: ${GREEN}$0 <node_id> [go|rust|all] [lines]${NC}"
    echo -e "  node_id: 0-4"
    echo -e "  type:    go (master+sub), rust, all (default)"
    echo -e "  lines:   number of lines to tail (default: 50)"
    echo ""
    echo -e "Individual scripts:"
    echo -e "  ${GREEN}./rust.sh <N> [-f]${NC}       — Rust log for node N"
    echo -e "  ${GREEN}./go-master.sh <N> [-f]${NC}  — Go Master log for node N"
    echo -e "  ${GREEN}./go-sub.sh <N> [-f]${NC}     — Go Sub log for node N"
    echo -e "  ${GREEN}./follow.sh <N>${NC}          — Follow all logs for node N"
}

# ── Tail a single log ──────────────────────────────────────────

tail_log() {
    local label="$1"
    local logfile="$2"
    local lines="$3"

    if [ ! -f "$logfile" ]; then
        echo -e "${RED}[MISSING]${NC} $label: $logfile"
        return
    fi

    echo -e "\n${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "  ${YELLOW}$label${NC}  →  $logfile"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    tail -n "$lines" "$logfile"
}

# ── Main ───────────────────────────────────────────────────────

if [ $# -eq 0 ]; then
    list_logs
    exit 0
fi

NODE_ID="${1}"
LOG_TYPE="${2:-all}"

case "$LOG_TYPE" in
    go)
        tail_log "Go Master (Node $NODE_ID)" "$(go_master_log $NODE_ID)" "$LINES"
        tail_log "Go Sub (Node $NODE_ID)" "$(go_sub_log $NODE_ID)" "$LINES"
        ;;
    rust)
        tail_log "Rust Metanode (Node $NODE_ID)" "$(rust_log $NODE_ID)" "$LINES"
        ;;
    all)
        tail_log "Rust Metanode (Node $NODE_ID)" "$(rust_log $NODE_ID)" "$LINES"
        tail_log "Go Master (Node $NODE_ID)" "$(go_master_log $NODE_ID)" "$LINES"
        tail_log "Go Sub (Node $NODE_ID)" "$(go_sub_log $NODE_ID)" "$LINES"
        ;;
    *)
        echo "Unknown log type: $LOG_TYPE (use go, rust, or all)"
        exit 1
        ;;
esac
