#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  CLUSTER STATUS CHECK                                             ║
# ║  Check all nodes across all servers                               ║
# ║  Usage: ./deploy_status.sh                                       ║
# ╚═══════════════════════════════════════════════════════════════════╝

set -uo pipefail

# Colors
GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'
CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/deploy.env"

# ─── Port mapping per node (must match config files) ────────────────
# Go Master RPC ports
declare -A GO_MASTER_RPC
GO_MASTER_RPC[0]=8757
GO_MASTER_RPC[1]=10747
GO_MASTER_RPC[2]=10749
GO_MASTER_RPC[3]=10750

# Go Sub RPC ports
declare -A GO_SUB_RPC
GO_SUB_RPC[0]=8646
GO_SUB_RPC[1]=10646
GO_SUB_RPC[2]=10650
GO_SUB_RPC[3]=10651

# Rust metrics ports
declare -A RUST_METRICS
RUST_METRICS[0]=9100
RUST_METRICS[1]=9101
RUST_METRICS[2]=9102
RUST_METRICS[3]=9103

# ─── SSH Helper ─────────────────────────────────────────────────────
ssh_cmd() {
    local host="$1"; shift
    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" ssh $SSH_OPTS "${SSH_USER}@${host}" "$@" 2>/dev/null
    elif [ -n "${SSH_KEY:-}" ]; then
        ssh $SSH_OPTS -i "$SSH_KEY" "${SSH_USER}@${host}" "$@" 2>/dev/null
    else
        ssh $SSH_OPTS "${SSH_USER}@${host}" "$@" 2>/dev/null
    fi
}

get_unique_servers() { echo "${NODE_SERVER[@]}" | tr ' ' '\n' | sort -u; }
get_nodes_for_server() {
    local server="$1"; local nodes=""
    for nid in "${!NODE_SERVER[@]}"; do
        [ "${NODE_SERVER[$nid]}" == "$server" ] && nodes="$nodes $nid"
    done
    echo "$nodes" | xargs
}

# ─── Status Icons ───────────────────────────────────────────────────
ok()   { echo -e "${GREEN}✅${NC}"; }
fail() { echo -e "${RED}❌${NC}"; }

# ═══════════════════════════════════════════════════════════════════
# MAIN STATUS CHECK
# ═══════════════════════════════════════════════════════════════════
echo ""
echo -e "${BOLD}╔═══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  📊 CLUSTER STATUS CHECK — $(date '+%Y-%m-%d %H:%M:%S')          ║${NC}"
echo -e "${BOLD}╚═══════════════════════════════════════════════════════════════╝${NC}"

TOTAL_OK=0
TOTAL_FAIL=0
BLOCK_HEIGHTS=()

SERVERS=$(get_unique_servers)

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  📍 Server: ${BOLD}$server${NC}${CYAN} — Nodes: [$nodes]${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

    # SSH connectivity
    if ! ssh_cmd "$server" "echo ok" &>/dev/null; then
        echo -e "  $(fail) SSH unreachable"
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
        continue
    fi
    echo -e "  $(ok) SSH connected"

    for id in $nodes; do
        echo ""
        echo -e "  ${BOLD}── Node $id ──${NC}"

        # 1. Process status via tmux
        TMUX_MASTER=$(ssh_cmd "$server" "tmux has-session -t go-master-${id} 2>/dev/null && echo UP || echo DOWN")
        TMUX_SUB=$(ssh_cmd "$server" "tmux has-session -t go-sub-${id} 2>/dev/null && echo UP || echo DOWN")
        TMUX_RUST=$(ssh_cmd "$server" "tmux has-session -t metanode-${id} 2>/dev/null && echo UP || echo DOWN")

        [ "$TMUX_MASTER" == "UP" ] && { echo -e "    $(ok) Go Master   — tmux session active"; TOTAL_OK=$((TOTAL_OK+1)); } || { echo -e "    $(fail) Go Master   — tmux session missing"; TOTAL_FAIL=$((TOTAL_FAIL+1)); }
        [ "$TMUX_SUB" == "UP" ]    && { echo -e "    $(ok) Go Sub      — tmux session active"; TOTAL_OK=$((TOTAL_OK+1)); } || { echo -e "    $(fail) Go Sub      — tmux session missing"; TOTAL_FAIL=$((TOTAL_FAIL+1)); }
        [ "$TMUX_RUST" == "UP" ]   && { echo -e "    $(ok) Rust Node   — tmux session active"; TOTAL_OK=$((TOTAL_OK+1)); } || { echo -e "    $(fail) Rust Node   — tmux session missing"; TOTAL_FAIL=$((TOTAL_FAIL+1)); }

        # 2. Go Master RPC health
        MASTER_PORT="${GO_MASTER_RPC[$id]}"
        HEALTH=$(ssh_cmd "$server" "curl -sf http://127.0.0.1:${MASTER_PORT}/health 2>/dev/null" || echo "FAIL")
        if [ "$HEALTH" != "FAIL" ]; then
            echo -e "    $(ok) Go Master RPC (:${MASTER_PORT}/health) — responded"
        else
            echo -e "    $(fail) Go Master RPC (:${MASTER_PORT}/health) — no response"
        fi

        # 3. Go Master block number
        BLOCK=$(ssh_cmd "$server" "curl -sf http://127.0.0.1:${MASTER_PORT}/block_number 2>/dev/null" || echo "")
        if [ -n "$BLOCK" ]; then
            echo -e "    $(ok) Block height: ${GREEN}${BLOCK}${NC}"
            BLOCK_HEIGHTS+=("node${id}=$BLOCK")
        else
            echo -e "    ${YELLOW}   ⚠️  Block number not available yet${NC}"
        fi

        # 4. Pipeline stats
        STATS=$(ssh_cmd "$server" "curl -sf http://127.0.0.1:${MASTER_PORT}/pipeline/stats 2>/dev/null" || echo "")
        if [ -n "$STATS" ]; then
            echo -e "    $(ok) Pipeline stats available"
        fi

        # 5. Go Sub health
        SUB_PORT="${GO_SUB_RPC[$id]}"
        SUB_HEALTH=$(ssh_cmd "$server" "curl -sf http://127.0.0.1:${SUB_PORT}/health 2>/dev/null" || echo "FAIL")
        if [ "$SUB_HEALTH" != "FAIL" ]; then
            echo -e "    $(ok) Go Sub RPC   (:${SUB_PORT}/health) — responded"
        else
            echo -e "    $(fail) Go Sub RPC   (:${SUB_PORT}/health) — no response"
        fi

        # 6. Rust metrics port
        METRICS_PORT="${RUST_METRICS[$id]}"
        METRICS=$(ssh_cmd "$server" "curl -sf http://127.0.0.1:${METRICS_PORT}/metrics 2>/dev/null | head -1" || echo "FAIL")
        if [ "$METRICS" != "FAIL" ] && [ -n "$METRICS" ]; then
            echo -e "    $(ok) Rust metrics  (:${METRICS_PORT}) — responded"
        else
            echo -e "    ${YELLOW}   ⚠️  Rust metrics (:${METRICS_PORT}) — no response${NC}"
        fi

        # 7. Recent log tail
        LOG_TAIL=$(ssh_cmd "$server" "tail -3 '${REMOTE_METANODE}/logs/node_${id}/go-master-stdout.log' 2>/dev/null" || echo "")
        if [ -n "$LOG_TAIL" ]; then
            echo -e "    ${CYAN}📋 Last Go Master log:${NC}"
            echo "$LOG_TAIL" | while IFS= read -r line; do
                echo -e "       ${line}"
            done
        fi
    done
done

# ═══════════════════════════════════════════════════════════════════
# SUMMARY
# ═══════════════════════════════════════════════════════════════════
echo ""
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}  📊 SUMMARY${NC}"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  Services: ${GREEN}${TOTAL_OK} up${NC} / ${RED}${TOTAL_FAIL} down${NC}"

if [ ${#BLOCK_HEIGHTS[@]} -gt 0 ]; then
    echo -e "  Block heights: ${BLOCK_HEIGHTS[*]}"

    # Check consensus (all same block height ±2)
    HEIGHTS=($(printf '%s\n' "${BLOCK_HEIGHTS[@]}" | sed 's/.*=//' | sort -n))
    if [ ${#HEIGHTS[@]} -gt 1 ]; then
        MIN=${HEIGHTS[0]}
        MAX=${HEIGHTS[-1]}
        DIFF=$((MAX - MIN))
        if [ "$DIFF" -le 2 ]; then
            echo -e "  Consensus: $(ok) ${GREEN}Nodes in sync (diff: $DIFF blocks)${NC}"
        else
            echo -e "  Consensus: $(fail) ${RED}Nodes OUT OF SYNC (diff: $DIFF blocks)${NC}"
        fi
    fi
fi

echo ""
if [ "$TOTAL_FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}🎉 All systems operational!${NC}"
else
    echo -e "  ${YELLOW}⚠️  Some services need attention.${NC}"
fi
echo ""
