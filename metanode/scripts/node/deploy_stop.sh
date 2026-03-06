#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  STOP CLUSTER ON ALL SERVERS                                      ║
# ║  Usage: ./deploy_stop.sh                                          ║
# ╚═══════════════════════════════════════════════════════════════════╝

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/deploy.env"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

ssh_cmd() {
    local host="$1"; shift
    local ssh_args="$SSH_OPTS"
    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" ssh $ssh_args "${SSH_USER}@${host}" "$@"
    elif [ -n "${SSH_KEY:-}" ]; then
        ssh $ssh_args -i "$SSH_KEY" "${SSH_USER}@${host}" "$@"
    else
        ssh $ssh_args "${SSH_USER}@${host}" "$@"
    fi
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
echo -e "${RED}🛑 Stopping cluster on all servers...${NC}"
echo ""

for server in $(get_unique_servers); do
    nodes=$(get_nodes_for_server "$server")
    echo -e "${YELLOW}  📤 Stopping nodes [$nodes] on $server...${NC}"

    ssh_cmd "$server" "
        pkill -f 'simple_chain' 2>/dev/null || true
        pkill -f 'metanode start' 2>/dev/null || true
        sleep 3
        for id in $nodes; do
            tmux kill-session -t go-master-\$id 2>/dev/null || true
            tmux kill-session -t go-sub-\$id 2>/dev/null || true
            tmux kill-session -t metanode-\$id 2>/dev/null || true
        done
        pkill -9 -f 'simple_chain' 2>/dev/null || true
        pkill -9 -f 'metanode start' 2>/dev/null || true
        rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true
    " 2>/dev/null || true

    echo -e "${GREEN}  ✅ $server stopped${NC}"
done

echo ""
echo -e "${GREEN}✅ All servers stopped.${NC}"
echo ""
