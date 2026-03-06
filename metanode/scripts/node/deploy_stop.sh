#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  STOP CLUSTER ON ALL SERVERS                                      ║
# ║  Usage: ./deploy_stop.sh                                         ║
# ╚═══════════════════════════════════════════════════════════════════╝

set -uo pipefail

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/deploy.env"

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

echo ""
echo -e "${RED}╔═══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${RED}║  🛑 STOPPING CLUSTER ON ALL SERVERS                          ║${NC}"
echo -e "${RED}╚═══════════════════════════════════════════════════════════════╝${NC}"
echo ""

SERVERS=$(get_unique_servers)

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    echo -e "${CYAN}  Stopping $server (nodes: [$nodes])...${NC}"

    ssh_cmd "$server" "
        # Graceful stop
        pkill -f 'simple_chain' 2>/dev/null || true
        pkill -f 'metanode start' 2>/dev/null || true
        sleep 3

        # Kill tmux sessions
        for id in $nodes; do
            tmux kill-session -t go-master-\$id 2>/dev/null || true
            tmux kill-session -t go-sub-\$id 2>/dev/null || true
            tmux kill-session -t metanode-\$id 2>/dev/null || true
        done

        # Force kill if still running
        pkill -9 -f 'simple_chain' 2>/dev/null || true
        pkill -9 -f 'metanode start' 2>/dev/null || true

        # Clean up sockets
        rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true
    " || true

    echo -e "${GREEN}  ✅ $server stopped${NC}"
done

echo ""
echo -e "${GREEN}  🛑 Cluster stopped on all servers.${NC}"
echo ""
