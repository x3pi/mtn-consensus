#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  AUTOMATED MULTI-SERVER CLUSTER DEPLOYMENT                        ║
# ║  Usage: ./deploy_cluster.sh [--sync] [--build] [--start] [--all] ║
# ║         ./deploy_cluster.sh --all        # Full deploy            ║
# ║         ./deploy_cluster.sh --start      # Start only (no sync)   ║
# ║         ./deploy_cluster.sh --sync       # Sync code only         ║
# ╚═══════════════════════════════════════════════════════════════════╝

set -euo pipefail

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
CYAN='\033[0;36m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/deploy.env"

# Load config
if [ ! -f "$ENV_FILE" ]; then
    echo -e "${RED}❌ deploy.env not found at $ENV_FILE${NC}"
    echo "   Copy deploy.env.example and edit it with your server IPs"
    exit 1
fi
source "$ENV_FILE"

# ─── Parse Arguments ────────────────────────────────────────────────
DO_SYNC=false
DO_BUILD=false
DO_IPS=false
DO_START=false

if [ $# -eq 0 ] || [[ "$*" == *"--all"* ]]; then
    DO_SYNC=true; DO_BUILD=true; DO_IPS=true; DO_START=true
fi
[[ "$*" == *"--sync"* ]] && DO_SYNC=true
[[ "$*" == *"--build"* ]] && DO_BUILD=true
[[ "$*" == *"--ips"* ]] && DO_IPS=true
[[ "$*" == *"--start"* ]] && DO_START=true

# ─── Helper Functions ───────────────────────────────────────────────

# Build SSH command — supports both key and password auth
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

rsync_cmd() {
    local src="$1" dst_host="$2" dst_path="$3"
    local rsync_args="-azP --delete"

    # Build SSH transport command
    local ssh_transport="ssh $SSH_OPTS"
    [ -n "${SSH_KEY:-}" ] && [ "${SSH_AUTH:-key}" != "password" ] && ssh_transport="$ssh_transport -i $SSH_KEY"

    # Build exclude args
    local excludes=""
    for pattern in "${RSYNC_EXCLUDES[@]}"; do
        excludes="$excludes --exclude='$pattern'"
    done

    if [ "${SSH_AUTH:-key}" == "password" ]; then
        eval sshpass -p "'$SSH_PASSWORD'" rsync $rsync_args -e "'$ssh_transport'" $excludes "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    else
        eval rsync $rsync_args -e "'$ssh_transport'" $excludes "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    fi
}

# Get unique server list
get_unique_servers() {
    echo "${NODE_SERVER[@]}" | tr ' ' '\n' | sort -u
}

# Get nodes for a specific server
get_nodes_for_server() {
    local server="$1"
    local nodes=""
    for node_id in "${!NODE_SERVER[@]}"; do
        if [ "${NODE_SERVER[$node_id]}" == "$server" ]; then
            nodes="$nodes $node_id"
        fi
    done
    echo "$nodes" | xargs  # trim whitespace
}

log_step() { echo -e "\n${BLUE}═══════════════════════════════════════════════════════════════${NC}"; echo -e "${BLUE}  $1${NC}"; echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"; }
log_ok()   { echo -e "${GREEN}  ✅ $1${NC}"; }
log_warn() { echo -e "${YELLOW}  ⚠️  $1${NC}"; }
log_info() { echo -e "${CYAN}  ℹ️  $1${NC}"; }
log_err()  { echo -e "${RED}  ❌ $1${NC}"; }

# ═══════════════════════════════════════════════════════════════════
# PHASE 0: Validate SSH connectivity
# ═══════════════════════════════════════════════════════════════════
echo ""
echo -e "${GREEN}╔═══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  🚀 MULTI-SERVER CLUSTER DEPLOYMENT                          ║${NC}"
echo -e "${GREEN}╚═══════════════════════════════════════════════════════════════╝${NC}"
echo ""

log_step "Phase 0: Validating SSH connectivity"
SERVERS=$(get_unique_servers)

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    if ssh_cmd "$server" "echo ok" &>/dev/null; then
        log_ok "SSH to $server — OK (nodes: $nodes)"
    else
        log_err "Cannot SSH to $server"
        echo "   Fix: ssh-copy-id ${SSH_USER}@${server}"
        exit 1
    fi
done

# ═══════════════════════════════════════════════════════════════════
# PHASE 1: Stop existing cluster (on all servers)
# ═══════════════════════════════════════════════════════════════════
log_step "Phase 1: Stopping existing cluster on all servers"

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    log_info "Stopping nodes [$nodes] on $server..."
    ssh_cmd "$server" "
        # SIGTERM for graceful shutdown
        pkill -f 'simple_chain' 2>/dev/null || true
        pkill -f 'metanode start' 2>/dev/null || true
        sleep 3
        # Clean tmux sessions
        for id in $nodes; do
            tmux kill-session -t go-master-\$id 2>/dev/null || true
            tmux kill-session -t go-sub-\$id 2>/dev/null || true
            tmux kill-session -t metanode-\$id 2>/dev/null || true
        done
        # Force kill stragglers
        pkill -9 -f 'simple_chain' 2>/dev/null || true
        pkill -9 -f 'metanode start' 2>/dev/null || true
        # Clean sockets
        rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true
    " 2>/dev/null || true
    log_ok "$server stopped"
done

# ═══════════════════════════════════════════════════════════════════
# PHASE 2: Sync code to all servers
# ═══════════════════════════════════════════════════════════════════
if $DO_SYNC; then
    log_step "Phase 2: Syncing code to all servers"

    LOCAL_CHAIN_DIR="$(cd "$SCRIPT_DIR/../../../../" && pwd)"

    for server in $SERVERS; do
        # Skip localhost
        local_ip=$(hostname -I 2>/dev/null | awk '{print $1}')
        if [ "$server" == "$local_ip" ] || [ "$server" == "127.0.0.1" ]; then
            log_info "Skipping sync to $server (local)"
            continue
        fi

        log_info "Syncing to $server..."
        for dir in "${SYNC_DIRS[@]}"; do
            echo -e "    📦 $dir → $server:${REMOTE_CHAIN_DIR}/$dir"
            rsync_cmd "${LOCAL_CHAIN_DIR}/$dir/" "$server" "${REMOTE_CHAIN_DIR}/$dir/"
        done
        log_ok "Synced to $server"
    done
else
    log_info "Phase 2: Skipped (use --sync to enable)"
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 3: Update IPs in config files
# ═══════════════════════════════════════════════════════════════════
if $DO_IPS; then
    log_step "Phase 3: Updating IP addresses in configs"

    # Build IP args for update_ips.sh: NODE0_IP NODE1_IP NODE2_IP NODE3_IP
    IP_ARGS="${NODE_SERVER[0]} ${NODE_SERVER[1]} ${NODE_SERVER[2]} ${NODE_SERVER[3]}"

    # If there are more nodes (e.g. node 4), add them
    if [ -n "${NODE_SERVER[4]:-}" ]; then
        IP_ARGS="$IP_ARGS ${NODE_SERVER[4]}"
    else
        IP_ARGS="$IP_ARGS ${NODE_SERVER[3]}"  # Default node4 to same as node3
    fi

    for server in $SERVERS; do
        log_info "Updating IPs on $server..."
        ssh_cmd "$server" "
            cd ${REMOTE_SCRIPTS}
            if [ -f update_ips.sh ]; then
                bash update_ips.sh $IP_ARGS
            else
                echo 'update_ips.sh not found, skipping'
            fi
        "
        log_ok "IPs updated on $server"
    done
else
    log_info "Phase 3: Skipped (use --ips to enable)"
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 4: Build binaries on all servers
# ═══════════════════════════════════════════════════════════════════
if $DO_BUILD; then
    log_step "Phase 4: Building binaries on all servers"

    for server in $SERVERS; do
        log_info "Building on $server..."
        ssh_cmd "$server" "
            export PATH=\"/home/${SSH_USER}/protoc3/bin:\$PATH\"
            export PATH=\"/home/${SSH_USER}/.cargo/bin:\$PATH\"
            export PATH=\"/usr/local/go/bin:\$PATH\"

            # Build Rust
            echo '  🔧 Building Rust metanode...'
            cd ${REMOTE_METANODE}
            cargo +${RUST_TOOLCHAIN} build --release --bin metanode 2>&1 | tail -1

            # Build Go
            echo '  🔧 Building Go simple_chain...'
            cd ${REMOTE_GO_SIMPLE}
            export GOTOOLCHAIN=${GO_TOOLCHAIN}
            go build -o simple_chain . 2>&1
            echo '  ✅ Build complete'
        "
        log_ok "Built on $server"
    done
else
    log_info "Phase 4: Skipped (use --build to enable)"
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 5: Clean data and start nodes
# ═══════════════════════════════════════════════════════════════════
if $DO_START; then
    log_step "Phase 5: Clean data + start nodes on all servers"

    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        log_info "Cleaning data for nodes [$nodes] on $server..."

        ssh_cmd "$server" "
            cd ${REMOTE_GO_SIMPLE}
            METANODE_ROOT='${REMOTE_METANODE}'
            LOG_DIR=\"\${METANODE_ROOT}/logs\"

            for id in $nodes; do
                DATA=\"node\${id}\"
                # Clean Go data
                rm -rf \"sample/\${DATA}/data\" 2>/dev/null || true
                rm -rf \"sample/\${DATA}/data-write\" 2>/dev/null || true
                rm -rf \"sample/\${DATA}/back_up\" 2>/dev/null || true
                rm -rf \"sample/\${DATA}/back_up_write\" 2>/dev/null || true
                # Clean Rust storage
                rm -rf \"\${METANODE_ROOT}/config/storage/node_\${id}\" 2>/dev/null || true
                # Clean logs
                rm -rf \"\${LOG_DIR}/node_\${id}\" 2>/dev/null || true
                # Recreate
                mkdir -p \"sample/\${DATA}/data/data/xapian_node\"
                mkdir -p \"sample/\${DATA}/data-write/data/xapian_node\"
                mkdir -p \"sample/\${DATA}/back_up\"
                mkdir -p \"sample/\${DATA}/back_up_write\"
                mkdir -p \"\${METANODE_ROOT}/config/storage/node_\${id}\"
                mkdir -p \"\${LOG_DIR}/node_\${id}\"
            done

            rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true
            rm -f /tmp/epoch_data_backup*.json 2>/dev/null || true
        "
        log_ok "Data cleaned on $server"
    done

    # ─── Start Go Masters ───────────────────────────────────────────
    log_step "Phase 5a: Starting Go Masters"

    GO_MASTER_CONFIGS=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json")

    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        for id in $nodes; do
            log_info "Starting Go Master $id on $server..."
            XAPIAN="sample/node${id}/data/data/xapian_node"
            PPROF_ARG=""
            [ "$id" -eq "0" ] && PPROF_ARG="--pprof-addr=localhost:6060"

            ssh_cmd "$server" "
                cd ${REMOTE_GO_SIMPLE}
                LOG_DIR='${REMOTE_METANODE}/logs'
                tmux new-session -d -s 'go-master-${id}' -c '${REMOTE_GO_SIMPLE}' \
                    \"ulimit -n 100000; export GOTOOLCHAIN=${GO_TOOLCHAIN} && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='${XAPIAN}' && ./simple_chain -config=${GO_MASTER_CONFIGS[$id]} ${PPROF_ARG} >> \\\"\${LOG_DIR}/node_${id}/go-master-stdout.log\\\" 2>&1\"
            "
            log_ok "Go Master $id started"
        done
    done

    sleep 3

    # ─── Wait for UDS sockets ───────────────────────────────────────
    log_step "Phase 5b: Waiting for Go Master sockets"

    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        for id in $nodes; do
            SOCKET="/tmp/rust-go-node${id}-master.sock"
            log_info "Waiting for socket $SOCKET on $server..."

            for attempt in $(seq 1 60); do
                if ssh_cmd "$server" "test -S $SOCKET" 2>/dev/null; then
                    log_ok "Go Master $id ready (${attempt}s)"
                    break
                fi
                if [ "$attempt" -eq 60 ]; then
                    log_warn "Timeout waiting for Go Master $id socket"
                fi
                sleep 1
            done
        done
    done

    # ─── Start Go Subs ──────────────────────────────────────────────
    log_step "Phase 5c: Starting Go Subs"

    GO_SUB_CONFIGS=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json")

    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        for id in $nodes; do
            log_info "Starting Go Sub $id on $server..."
            XAPIAN="sample/node${id}/data-write/data/xapian_node"

            ssh_cmd "$server" "
                cd ${REMOTE_GO_SIMPLE}
                LOG_DIR='${REMOTE_METANODE}/logs'
                tmux new-session -d -s 'go-sub-${id}' -c '${REMOTE_GO_SIMPLE}' \
                    \"ulimit -n 100000; export GOTOOLCHAIN=${GO_TOOLCHAIN} && export GOMEMLIMIT=4GiB && export XAPIAN_BASE_PATH='${XAPIAN}' && ./simple_chain -config=${GO_SUB_CONFIGS[$id]} >> \\\"\${LOG_DIR}/node_${id}/go-sub-stdout.log\\\" 2>&1\"
            "
            log_ok "Go Sub $id started"
            sleep 1
        done
    done

    sleep 5

    # ─── Start Rust Metanodes ───────────────────────────────────────
    log_step "Phase 5d: Starting Rust Metanodes"

    RUST_CONFIGS=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml")

    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        for id in $nodes; do
            log_info "Starting Rust Node $id on $server..."

            ssh_cmd "$server" "
                cd ${REMOTE_METANODE}
                BINARY='${REMOTE_METANODE}/target/release/metanode'
                LOG_DIR='${REMOTE_METANODE}/logs'
                tmux new-session -d -s 'metanode-${id}' -c '${REMOTE_METANODE}' \
                    \"ulimit -n 100000; export RUST_LOG=info,consensus_core=debug; export DB_WRITE_BUFFER_SIZE_MB=256; export DB_WAL_SIZE_MB=256; \\\$BINARY start --config ${RUST_CONFIGS[$id]} >> \\\"\${LOG_DIR}/node_${id}/rust.log\\\" 2>&1\"
            "
            log_ok "Rust Node $id started"
            sleep 1
        done
    done

    sleep 5
fi

# ═══════════════════════════════════════════════════════════════════
# SUMMARY
# ═══════════════════════════════════════════════════════════════════
echo ""
echo -e "${GREEN}╔═══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  🎉 CLUSTER DEPLOYMENT COMPLETE!                              ║${NC}"
echo -e "${GREEN}╚═══════════════════════════════════════════════════════════════╝${NC}"
echo ""

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    echo -e "${CYAN}  📍 $server — Nodes: [$nodes]${NC}"
    for id in $nodes; do
        echo -e "     tmux: metanode-$id | go-master-$id | go-sub-$id"
    done
done

echo ""
echo -e "${GREEN}  📝 Next steps:${NC}"
echo -e "     Check status:  ${CYAN}./deploy_status.sh${NC}"
echo -e "     Stop cluster:  ${CYAN}./deploy_stop.sh${NC}"
echo -e "     View logs:     ${CYAN}ssh ${SSH_USER}@SERVER_IP 'tail -f ${REMOTE_METANODE}/logs/node_N/go-master-stdout.log'${NC}"
echo ""
