#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  CHECK CLUSTER STATUS ON ALL SERVERS                              ║
# ║  Usage: ./deploy_status.sh                                        ║
# ╚═══════════════════════════════════════════════════════════════════╝

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/deploy.env"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
CYAN='\033[0;36m'
NC='\033[0m'

ssh_cmd() {
    local host="$1"; shift
    local ssh_args="$SSH_OPTS"
    [ -n "${SSH_KEY:-}" ] && ssh_args="$ssh_args -i $SSH_KEY"
    ssh $ssh_args "${SSH_USER}@${host}" "$@"
}

get_unique_servers() {
    echo "${NODE_SERVER[@]}" | tr ' ' '\n' | sort -u
}

get_nodes_for_server() {
    local server="$1"
    local nodes=""
    for node_id in "${!NODE_SERVER[@]}"; do
        [ "${NODE_SERVER[$node_id]}" == "$server" ] && nodes="$nodes $node_id"
    done
    echo "$nodes" | xargs
}

echo ""
echo -e "${CYAN}╔═══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  📊 CLUSTER STATUS                                            ║${NC}"
echo -e "${CYAN}╚═══════════════════════════════════════════════════════════════╝${NC}"
echo ""

# ─── Port Maps ──────────────────────────────────────────────────────
# Node → RPC ports (for eth_blockNumber check)
declare -A RPC_PORTS
RPC_PORTS[0]=8757
RPC_PORTS[1]=10747
RPC_PORTS[2]=10749
RPC_PORTS[3]=10750

for server in $(get_unique_servers); do
    nodes=$(get_nodes_for_server "$server")
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  📍 Server: $server — Nodes: [$nodes]${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

    # Check SSH
    if ! ssh_cmd "$server" "echo ok" &>/dev/null; then
        echo -e "${RED}  ❌ SSH unreachable${NC}"
        continue
    fi

    # Check tmux sessions
    tmux_output=$(ssh_cmd "$server" "tmux ls 2>/dev/null || echo 'no sessions'" 2>/dev/null)

    for id in $nodes; do
        echo ""
        echo -e "  ${CYAN}Node $id:${NC}"

        # Go Master
        if echo "$tmux_output" | grep -q "go-master-${id}"; then
            echo -e "    ${GREEN}✅ Go Master $id — running${NC}"
        else
            echo -e "    ${RED}❌ Go Master $id — stopped${NC}"
        fi

        # Go Sub
        if echo "$tmux_output" | grep -q "go-sub-${id}"; then
            echo -e "    ${GREEN}✅ Go Sub $id — running${NC}"
        else
            echo -e "    ${RED}❌ Go Sub $id — stopped${NC}"
        fi

        # Rust
        if echo "$tmux_output" | grep -q "metanode-${id}"; then
            echo -e "    ${GREEN}✅ Rust Node $id — running${NC}"
        else
            echo -e "    ${RED}❌ Rust Node $id — stopped${NC}"
        fi

        # UDS socket
        socket_ok=$(ssh_cmd "$server" "test -S /tmp/rust-go-node${id}-master.sock && echo yes || echo no" 2>/dev/null)
        if [ "$socket_ok" == "yes" ]; then
            echo -e "    ${GREEN}✅ UDS Socket — connected${NC}"
        else
            echo -e "    ${YELLOW}⚠️  UDS Socket — missing${NC}"
        fi

        # RPC check (try reaching it)
        rpc_port="${RPC_PORTS[$id]:-}"
        if [ -n "$rpc_port" ]; then
            rpc_result=$(curl -s --connect-timeout 3 -X POST "http://${server}:${rpc_port}" \
                -H "Content-Type: application/json" \
                -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' 2>/dev/null || echo "fail")

            if echo "$rpc_result" | grep -q '"result"'; then
                block_hex=$(echo "$rpc_result" | grep -oP '"result":"0x[0-9a-fA-F]+"' | grep -oP '0x[0-9a-fA-F]+')
                block_dec=$((block_hex))
                echo -e "    ${GREEN}✅ RPC (:$rpc_port) — Block #$block_dec${NC}"
            else
                echo -e "    ${RED}❌ RPC (:$rpc_port) — not responding${NC}"
            fi
        fi
    done
    echo ""
done

# ─── Fork Check ─────────────────────────────────────────────────────
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${CYAN}  🔍 Cross-Node Fork Check${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

# Get block numbers from all nodes
declare -A BLOCK_NUMBERS
for id in "${!NODE_SERVER[@]}"; do
    server="${NODE_SERVER[$id]}"
    rpc_port="${RPC_PORTS[$id]:-}"
    [ -z "$rpc_port" ] && continue

    result=$(curl -s --connect-timeout 3 -X POST "http://${server}:${rpc_port}" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' 2>/dev/null || echo "fail")

    if echo "$result" | grep -q '"result"'; then
        block_hex=$(echo "$result" | grep -oP '"result":"0x[0-9a-fA-F]+"' | grep -oP '0x[0-9a-fA-F]+')
        BLOCK_NUMBERS[$id]=$((block_hex))
    else
        BLOCK_NUMBERS[$id]=-1
    fi
done

# Compare block numbers
all_match=true
ref_block=-1
for id in "${!BLOCK_NUMBERS[@]}"; do
    block="${BLOCK_NUMBERS[$id]}"
    if [ "$block" -lt 0 ]; then
        echo -e "  ${RED}❌ Node $id — not responding${NC}"
        all_match=false
        continue
    fi
    if [ "$ref_block" -lt 0 ]; then
        ref_block="$block"
    fi
    diff=$((block - ref_block))
    abs_diff=${diff#-}
    if [ "$abs_diff" -gt 2 ]; then
        echo -e "  ${YELLOW}⚠️  Node $id at block $block (drift: $diff from Node 0)${NC}"
        all_match=false
    else
        echo -e "  ${GREEN}  Node $id at block $block${NC}"
    fi
done

if $all_match && [ "$ref_block" -ge 0 ]; then
    echo -e "\n  ${GREEN}✅ All nodes in sync — NO FORK${NC}"
else
    echo -e "\n  ${YELLOW}⚠️  Possible sync issues detected${NC}"
fi

echo ""
