#!/bin/bash
# ╔═══════════════════════════════════════════════════════════════════╗
# ║  AUTOMATED MULTI-SERVER CLUSTER DEPLOYMENT                        ║
# ║                                                                   ║
# ║  Flow: Build locally → Push binaries+configs → Start remote       ║
# ║                                                                   ║
# ║  Usage: ./deploy_cluster.sh [--build] [--push] [--start] [--all] ║
# ║         ./deploy_cluster.sh --all        # Full deploy            ║
# ║         ./deploy_cluster.sh --push       # Push only (no build)   ║
# ║         ./deploy_cluster.sh --start      # Start only             ║
# ║         ./deploy_cluster.sh --stop       # Stop cluster           ║
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
    exit 1
fi
source "$ENV_FILE"

# ─── Parse Arguments ────────────────────────────────────────────────
DO_BUILD=false
DO_PUSH=false
DO_IPS=false
DO_START=false
DO_STOP=false

if [ $# -eq 0 ] || [[ "$*" == *"--all"* ]]; then
    DO_BUILD=true; DO_PUSH=true; DO_IPS=true; DO_START=true
fi
[[ "$*" == *"--build"* ]] && DO_BUILD=true
[[ "$*" == *"--push"* ]] && DO_PUSH=true
[[ "$*" == *"--ips"* ]] && DO_IPS=true
[[ "$*" == *"--start"* ]] && DO_START=true
[[ "$*" == *"--stop"* ]] && DO_STOP=true

# ─── Helper Functions ───────────────────────────────────────────────

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

scp_cmd() {
    local src="$1" dst_host="$2" dst_path="$3"
    local scp_args="$SSH_OPTS -r"
    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" scp $scp_args "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    elif [ -n "${SSH_KEY:-}" ]; then
        scp $scp_args -i "$SSH_KEY" "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    else
        scp $scp_args "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    fi
}

rsync_cmd() {
    local src="$1" dst_host="$2" dst_path="$3"
    local ssh_transport="ssh $SSH_OPTS"
    [ -n "${SSH_KEY:-}" ] && [ "${SSH_AUTH:-key}" != "password" ] && ssh_transport="$ssh_transport -i $SSH_KEY"

    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" rsync -azP -e "$ssh_transport" "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    else
        rsync -azP -e "$ssh_transport" "$src" "${SSH_USER}@${dst_host}:${dst_path}"
    fi
}

get_unique_servers() {
    echo "${NODE_SERVER[@]}" | tr ' ' '\n' | sort -u
}

get_nodes_for_server() {
    local server="$1"
    local nodes=""
    for node_id in "${!NODE_SERVER[@]}"; do
        if [ "${NODE_SERVER[$node_id]}" == "$server" ]; then
            nodes="$nodes $node_id"
        fi
    done
    echo "$nodes" | xargs
}

log_step() { echo -e "\n${BLUE}═══════════════════════════════════════════════════════════════${NC}"; echo -e "${BLUE}  $1${NC}"; echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"; }
log_ok()   { echo -e "${GREEN}  ✅ $1${NC}"; }
log_warn() { echo -e "${YELLOW}  ⚠️  $1${NC}"; }
log_info() { echo -e "${CYAN}  ℹ️  $1${NC}"; }
log_err()  { echo -e "${RED}  ❌ $1${NC}"; }

# ═══════════════════════════════════════════════════════════════════
# PHASE 0: Validate
# ═══════════════════════════════════════════════════════════════════
echo ""
echo -e "${GREEN}╔═══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  🚀 MULTI-SERVER CLUSTER DEPLOYMENT                          ║${NC}"
echo -e "${GREEN}║  Build local → Push binaries+configs → Start remote          ║${NC}"
echo -e "${GREEN}╚═══════════════════════════════════════════════════════════════╝${NC}"
echo ""

# Validate SSH
log_step "Phase 0: Validating SSH connectivity"
SERVERS=$(get_unique_servers)

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    if ssh_cmd "$server" "echo ok" &>/dev/null; then
        log_ok "SSH to $server — OK (nodes: $nodes)"
    else
        log_err "Cannot SSH to $server"
        exit 1
    fi
done

# ═══════════════════════════════════════════════════════════════════
# STOP ONLY (--stop flag)
# ═══════════════════════════════════════════════════════════════════
if $DO_STOP; then
    log_step "Stopping cluster on all servers"
    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        log_info "Stopping nodes [$nodes] on $server..."
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
        log_ok "$server stopped"
    done
    # If only --stop, exit here
    if ! $DO_BUILD && ! $DO_PUSH && ! $DO_START; then
        echo -e "\n${GREEN}  🛑 Cluster stopped.${NC}\n"
        exit 0
    fi
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 1: Build locally
# ═══════════════════════════════════════════════════════════════════
if $DO_BUILD; then
    log_step "Phase 1: Building binaries locally"

    # Build Rust metanode
    log_info "Building Rust metanode..."
    (
        cd "${LOCAL_METANODE}"
        cargo +${RUST_TOOLCHAIN} build --release --bin metanode 2>&1 | tail -3
    )
    log_ok "Rust binary: ${LOCAL_METANODE}/target/release/metanode"

    # Build Go simple_chain
    log_info "Building Go simple_chain..."
    (
        cd "${LOCAL_GO_SIMPLE}"
        export GOTOOLCHAIN=${GO_TOOLCHAIN}
        go build -o simple_chain . 2>&1
    )
    log_ok "Go binary: ${LOCAL_GO_SIMPLE}/simple_chain"
else
    log_info "Phase 1: Skipped (use --build to enable)"
fi

# Verify binaries exist
RUST_BINARY="${LOCAL_METANODE}/target/release/metanode"
GO_BINARY="${LOCAL_GO_SIMPLE}/simple_chain"

if ($DO_PUSH || $DO_START) && [ ! -f "$RUST_BINARY" ]; then
    log_err "Rust binary not found: $RUST_BINARY (run with --build first)"
    exit 1
fi
if ($DO_PUSH || $DO_START) && [ ! -f "$GO_BINARY" ]; then
    log_err "Go binary not found: $GO_BINARY (run with --build first)"
    exit 1
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 2: Stop remote cluster before push
# ═══════════════════════════════════════════════════════════════════
if $DO_PUSH || $DO_START; then
    log_step "Phase 2: Stopping existing cluster"
    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        log_info "Stopping nodes [$nodes] on $server..."
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
        log_ok "$server stopped"
    done
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 3: Push binaries + configs to each server
# ═══════════════════════════════════════════════════════════════════
if $DO_PUSH; then
    log_step "Phase 3: Creating remote directories + pushing files"

    GO_MASTER_CONFIGS=("config-master-node0.json" "config-master-node1.json" "config-master-node2.json" "config-master-node3.json")
    GO_SUB_CONFIGS=("config-sub-node0.json" "config-sub-node1.json" "config-sub-node2.json" "config-sub-node3.json")
    RUST_CONFIGS=("config/node_0.toml" "config/node_1.toml" "config/node_2.toml" "config/node_3.toml")

    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        log_info "Deploying to $server (nodes: [$nodes])..."

        # Create directory structure on remote
        ssh_cmd "$server" "
            mkdir -p '${REMOTE_GO_SIMPLE}'
            mkdir -p '${REMOTE_METANODE}/target/release'
            mkdir -p '${REMOTE_METANODE}/config'
            mkdir -p '${REMOTE_METANODE}/logs'
            mkdir -p '${REMOTE_SCRIPTS}'
        "

        # Push binaries
        echo -e "    📦 Pushing Go binary..."
        rsync_cmd "$GO_BINARY" "$server" "${REMOTE_GO_SIMPLE}/simple_chain"
        echo -e "    📦 Pushing Rust binary..."
        rsync_cmd "$RUST_BINARY" "$server" "${REMOTE_METANODE}/target/release/metanode"

        # Push genesis.json
        echo -e "    📄 Pushing genesis.json..."
        rsync_cmd "${LOCAL_GO_SIMPLE}/genesis.json" "$server" "${REMOTE_GO_SIMPLE}/genesis.json"

        # Push Rust committee.json
        echo -e "    📄 Pushing committee.json..."
        rsync_cmd "${LOCAL_METANODE}/config/committee.json" "$server" "${REMOTE_METANODE}/config/committee.json"

        # Push scripts (update_ips.sh, deploy scripts)
        echo -e "    📄 Pushing scripts..."
        rsync_cmd "${LOCAL_METANODE}/scripts/node/" "$server" "${REMOTE_SCRIPTS}/"

        # Push per-node configs
        for id in $nodes; do
            echo -e "    📄 Node $id configs..."

            # Create node data directories
            ssh_cmd "$server" "
                mkdir -p '${REMOTE_GO_SIMPLE}/sample/node${id}/data/data/xapian_node'
                mkdir -p '${REMOTE_GO_SIMPLE}/sample/node${id}/data-write/data/xapian_node'
                mkdir -p '${REMOTE_GO_SIMPLE}/sample/node${id}/back_up'
                mkdir -p '${REMOTE_GO_SIMPLE}/sample/node${id}/back_up_write'
                mkdir -p '${REMOTE_METANODE}/config/storage/node_${id}'
                mkdir -p '${REMOTE_METANODE}/logs/node_${id}'
            "

            # Go configs (master + sub)
            rsync_cmd "${LOCAL_GO_SIMPLE}/${GO_MASTER_CONFIGS[$id]}" "$server" "${REMOTE_GO_SIMPLE}/${GO_MASTER_CONFIGS[$id]}"
            rsync_cmd "${LOCAL_GO_SIMPLE}/${GO_SUB_CONFIGS[$id]}" "$server" "${REMOTE_GO_SIMPLE}/${GO_SUB_CONFIGS[$id]}"

            # Rust config + keys
            rsync_cmd "${LOCAL_METANODE}/${RUST_CONFIGS[$id]}" "$server" "${REMOTE_METANODE}/${RUST_CONFIGS[$id]}"
            rsync_cmd "${LOCAL_METANODE}/config/node_${id}_network_key.json" "$server" "${REMOTE_METANODE}/config/node_${id}_network_key.json"
            rsync_cmd "${LOCAL_METANODE}/config/node_${id}_protocol_key.json" "$server" "${REMOTE_METANODE}/config/node_${id}_protocol_key.json"
        done

        log_ok "Deployed to $server"
    done
else
    log_info "Phase 3: Skipped (use --push to enable)"
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 4: Update IPs in config files on remote servers
# ═══════════════════════════════════════════════════════════════════
if $DO_IPS; then
    log_step "Phase 4: Updating IP addresses in configs"

    IP_ARGS="${NODE_SERVER[0]} ${NODE_SERVER[1]} ${NODE_SERVER[2]} ${NODE_SERVER[3]}"
    if [ -n "${NODE_SERVER[4]:-}" ]; then
        IP_ARGS="$IP_ARGS ${NODE_SERVER[4]}"
    else
        IP_ARGS="$IP_ARGS ${NODE_SERVER[3]}"
    fi

    for server in $SERVERS; do
        log_info "Updating IPs on $server..."
        ssh_cmd "$server" "
            cd '${REMOTE_SCRIPTS}'
            if [ -f update_ips.sh ]; then
                bash update_ips.sh $IP_ARGS
            else
                echo 'update_ips.sh not found, skipping'
            fi
        "
        log_ok "IPs updated on $server"
    done
else
    log_info "Phase 4: Skipped (use --ips to enable)"
fi

# ═══════════════════════════════════════════════════════════════════
# PHASE 5: Start nodes on remote servers
# ═══════════════════════════════════════════════════════════════════
if $DO_START; then
    log_step "Phase 5: Clean data + start nodes"

    # Clean node data on each server
    for server in $SERVERS; do
        nodes=$(get_nodes_for_server "$server")
        log_info "Cleaning data for nodes [$nodes] on $server..."

        ssh_cmd "$server" "
            cd '${REMOTE_GO_SIMPLE}'
            for id in $nodes; do
                DATA=\"node\${id}\"
                rm -rf \"sample/\${DATA}/data\" 2>/dev/null || true
                rm -rf \"sample/\${DATA}/data-write\" 2>/dev/null || true
                rm -rf \"sample/\${DATA}/back_up\" 2>/dev/null || true
                rm -rf \"sample/\${DATA}/back_up_write\" 2>/dev/null || true
                rm -rf \"${REMOTE_METANODE}/config/storage/node_\${id}\" 2>/dev/null || true
                rm -rf \"${REMOTE_METANODE}/logs/node_\${id}\" 2>/dev/null || true
                mkdir -p \"sample/\${DATA}/data/data/xapian_node\"
                mkdir -p \"sample/\${DATA}/data-write/data/xapian_node\"
                mkdir -p \"sample/\${DATA}/back_up\"
                mkdir -p \"sample/\${DATA}/back_up_write\"
                mkdir -p \"${REMOTE_METANODE}/config/storage/node_\${id}\"
                mkdir -p \"${REMOTE_METANODE}/logs/node_\${id}\"
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
                cd '${REMOTE_GO_SIMPLE}'
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
                cd '${REMOTE_GO_SIMPLE}'
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
                cd '${REMOTE_METANODE}'
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
echo -e "${GREEN}║  🎉 DEPLOYMENT COMPLETE!                                     ║${NC}"
echo -e "${GREEN}╚═══════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${CYAN}  Actions: build=$DO_BUILD push=$DO_PUSH ips=$DO_IPS start=$DO_START${NC}"
echo ""

for server in $SERVERS; do
    nodes=$(get_nodes_for_server "$server")
    echo -e "${CYAN}  📍 $server — Nodes: [$nodes]${NC}"
    for id in $nodes; do
        echo -e "     tmux: metanode-$id | go-master-$id | go-sub-$id"
    done
done

echo ""
echo -e "${GREEN}  📝 Commands:${NC}"
echo -e "     Full deploy:   ${CYAN}./deploy_cluster.sh --all${NC}"
echo -e "     Build only:    ${CYAN}./deploy_cluster.sh --build${NC}"
echo -e "     Push only:     ${CYAN}./deploy_cluster.sh --push --ips${NC}"
echo -e "     Start only:    ${CYAN}./deploy_cluster.sh --start${NC}"
echo -e "     Stop cluster:  ${CYAN}./deploy_cluster.sh --stop${NC}"
echo -e "     Check status:  ${CYAN}./deploy_status.sh${NC}"
echo ""
