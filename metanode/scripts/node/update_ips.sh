#!/bin/bash

# ============================================================================
# update_ips.sh — Ghi đè IP tất cả config files cho cả Rust và Go
#
# Cách dùng:
#   ./update_ips.sh NODE0_IP NODE1_IP NODE2_IP NODE3_IP [NODE4_IP]
#
# Ví dụ:
#   ./update_ips.sh 192.168.1.231 192.168.1.232 192.168.1.232 192.168.1.232
#   ./update_ips.sh 10.0.0.1 10.0.0.2 10.0.0.3 10.0.0.4 10.0.0.5
#   ./update_ips.sh 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1  # localhost
#
# Files ảnh hưởng:
#   Rust:  mtn-consensus/metanode/config/node_{0..4}.toml
#          - network_address (IP:port P2P consensus)
#          - peer_rpc_addresses (danh sách IP peer discovery)
#   Go:    mtn-simple-2025/cmd/simple_chain/config-master-node{0..4}.json
#          - meta_node_rpc_address (Rust RPC endpoint)
#          mtn-simple-2025/cmd/simple_chain/config-sub-node{0..4}.json
#          - meta_node_rpc_address (Rust RPC endpoint)
#          - nodes.master_address (Go Master endpoint cho sub node)
#   Tools: mtn-simple-2025/cmd/tool/tps_blast/run_multinode_load.sh
#          - NODES, RPCS arrays, block_hash_checker nodes
#          mtn-simple-2025/cmd/tool/tps_blast/run_node0_only_load.sh
#          - NODES array, -rpc parameter
#          mtn-simple-2025/cmd/tool/tx_sender/config.json
#          - parent_connection_address
#          mtn-simple-2025/cmd/tool/tps_blast/config.json
#          - parent_connection_address
#          mtn-simple-2025/cmd/tool/tps_benchmark_multi_node/config.json
#          - parent_connection_address
# ============================================================================

set -euo pipefail

# ─── Colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# ─── Paths ───────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RUST_CONFIG_DIR="$METANODE_DIR/config"
GO_DIR="$(cd "$METANODE_DIR/../../mtn-simple-2025" && pwd)"
GO_CONFIG_DIR="$GO_DIR/cmd/simple_chain"

# ─── Port Mapping ────────────────────────────────────────────────────────────
# Consensus P2P ports per node
CONSENSUS_PORTS=(9000 9001 9002 9003 9004)
# Peer RPC ports per node
PEER_RPC_PORTS=(19000 19001 19002 19003 19004)
# Rust RPC ports (metrics + 1000) per node
RUST_RPC_PORTS=(10100 10101 10102 10103 10104)
# altRust RPC ports for meta_node_rpc_address (some configs use 10111, 10112 etc)
# We detect from existing config and only change the IP part.

# Go Master connection ports per node
GO_MASTER_CONN_PORTS=(4201 6201 6211 6221 6241)
# Go Sub connection ports per node
GO_SUB_CONN_PORTS=(4200 6200 6210 6220 6240)

# ─── Usage ───────────────────────────────────────────────────────────────────
usage() {
    echo -e "${BOLD}╔═══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}║  🌐 IP Config Override Tool for mtn-consensus            ║${NC}"
    echo -e "${BOLD}╚═══════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "Cách dùng:"
    echo -e "  ${CYAN}$0 NODE0_IP NODE1_IP NODE2_IP NODE3_IP [NODE4_IP]${NC}"
    echo ""
    echo -e "Flags:"
    echo -e "  ${CYAN}--dry-run${NC}    Chỉ preview, không ghi file"
    echo -e "  ${CYAN}--help${NC}       Hiện hướng dẫn"
    echo ""
    echo -e "Ví dụ:"
    echo -e "  $0 192.168.1.231 192.168.1.232 192.168.1.232 192.168.1.232"
    echo -e "  $0 10.0.0.1 10.0.0.2 10.0.0.3 10.0.0.4 10.0.0.5"
    echo -e "  $0 --dry-run 192.168.1.100 192.168.1.101 192.168.1.102 192.168.1.103"
    echo ""
    echo -e "Files ảnh hưởng:"
    echo -e "  ${GREEN}Rust:${NC}  config/node_{0..4}.toml (network_address, peer_rpc_addresses)"
    echo -e "  ${GREEN}Go:${NC}    config-master-node{0..4}.json (meta_node_rpc_address)"
    echo -e "  ${GREEN}Go:${NC}    config-sub-node{0..4}.json (meta_node_rpc_address, master_address)"
    exit 0
}

# ─── Parse Args ──────────────────────────────────────────────────────────────
DRY_RUN=false
IPS=()

for arg in "$@"; do
    case "$arg" in
        --dry-run)
            DRY_RUN=true
            ;;
        --help|-h)
            usage
            ;;
        *)
            IPS+=("$arg")
            ;;
    esac
done

if [ ${#IPS[@]} -lt 4 ]; then
    echo -e "${RED}❌ Cần ít nhất 4 IPs (node 0-3). Nhận được: ${#IPS[@]}${NC}"
    echo ""
    usage
fi

# Default node 4 = node 3's IP if not provided
if [ ${#IPS[@]} -lt 5 ]; then
    IPS+=(${IPS[3]})
fi

NODE_COUNT=${#IPS[@]}

# ─── Validate IPs ───────────────────────────────────────────────────────────
validate_ip() {
    local ip=$1
    if [[ ! $ip =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$ ]]; then
        echo -e "${RED}❌ IP không hợp lệ: $ip${NC}"
        exit 1
    fi
}

for ip in "${IPS[@]}"; do
    validate_ip "$ip"
done

# ─── Print Plan ──────────────────────────────────────────────────────────────
echo -e "${BOLD}╔═══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  🌐 IP Config Override Tool                              ║${NC}"
echo -e "${BOLD}╚═══════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${CYAN}📋 IP mapping:${NC}"
for i in $(seq 0 $((NODE_COUNT - 1))); do
    echo -e "  Node $i: ${GREEN}${IPS[$i]}${NC}"
done
echo ""

if $DRY_RUN; then
    echo -e "${YELLOW}⚠️  DRY RUN — không ghi file, chỉ preview${NC}"
    echo ""
fi

CHANGED_COUNT=0

# ─── Helper: sed with backup ────────────────────────────────────────────────
safe_sed() {
    local file="$1"
    local pattern="$2"
    local description="$3"

    if [ ! -f "$file" ]; then
        echo -e "  ${YELLOW}⚠️  File không tồn tại: $(basename $file)${NC}"
        return
    fi

    # Check if pattern would change anything
    if grep -qP "$(echo "$pattern" | sed 's|s/||;s|/.*||')" "$file" 2>/dev/null || true; then
        if $DRY_RUN; then
            echo -e "  ${CYAN}[DRY] ${description}${NC}"
        else
            sed -i "$pattern" "$file"
            echo -e "  ${GREEN}✅ ${description}${NC}"
        fi
        CHANGED_COUNT=$((CHANGED_COUNT + 1))
    fi
}

# ─── Update Rust TOML Configs ────────────────────────────────────────────────
echo -e "${BOLD}═══ Rust TOML (node_{0..4}.toml) ═══${NC}"

for N in $(seq 0 $((NODE_COUNT - 1))); do
    TOML="$RUST_CONFIG_DIR/node_${N}.toml"
    if [ ! -f "$TOML" ]; then
        echo -e "  ${YELLOW}⚠️  Bỏ qua node_${N}.toml (không tồn tại)${NC}"
        continue
    fi

    echo -e "\n  ${BOLD}📄 node_${N}.toml${NC}"
    NODE_IP="${IPS[$N]}"

    # 1. network_address = "IP:900N"
    safe_sed "$TOML" \
        "s|^network_address = \"[^\"]*\"|network_address = \"${NODE_IP}:${CONSENSUS_PORTS[$N]}\"|" \
        "network_address → ${NODE_IP}:${CONSENSUS_PORTS[$N]}"

    # 2. peer_rpc_addresses = ["IP0:19000", "IP1:19001", ...]
    #    Build the list of all OTHER nodes' peer addresses
    PEER_LIST=""
    for P in $(seq 0 $((NODE_COUNT - 1))); do
        if [ $P -ne $N ]; then
            if [ -n "$PEER_LIST" ]; then
                PEER_LIST="$PEER_LIST, "
            fi
            PEER_LIST="${PEER_LIST}\"${IPS[$P]}:${PEER_RPC_PORTS[$P]}\""
        fi
    done

    safe_sed "$TOML" \
        "s|^peer_rpc_addresses = \[.*\]|peer_rpc_addresses = [${PEER_LIST}]|" \
        "peer_rpc_addresses → [${PEER_LIST}]"
done

# ─── Update Go Master JSON Configs ───────────────────────────────────────────
echo -e "\n${BOLD}═══ Go Master JSON (config-master-node{0..4}.json) ═══${NC}"

for N in $(seq 0 $((NODE_COUNT - 1))); do
    MASTER_JSON="$GO_CONFIG_DIR/config-master-node${N}.json"
    if [ ! -f "$MASTER_JSON" ]; then
        echo -e "  ${YELLOW}⚠️  Bỏ qua config-master-node${N}.json (không tồn tại)${NC}"
        continue
    fi

    echo -e "\n  ${BOLD}📄 config-master-node${N}.json${NC}"
    NODE_IP="${IPS[$N]}"

    # meta_node_rpc_address: chỉ thay IP, giữ port
    # Pattern: "meta_node_rpc_address": "IP:PORT"
    safe_sed "$MASTER_JSON" \
        "s|\"meta_node_rpc_address\": \"[0-9.]*:|\"meta_node_rpc_address\": \"${NODE_IP}:|" \
        "meta_node_rpc_address IP → ${NODE_IP}"
done

# ─── Update Go Sub JSON Configs ──────────────────────────────────────────────
echo -e "\n${BOLD}═══ Go Sub JSON (config-sub-node{0..4}.json) ═══${NC}"

for N in $(seq 0 $((NODE_COUNT - 1))); do
    SUB_JSON="$GO_CONFIG_DIR/config-sub-node${N}.json"
    if [ ! -f "$SUB_JSON" ]; then
        echo -e "  ${YELLOW}⚠️  Bỏ qua config-sub-node${N}.json (không tồn tại)${NC}"
        continue
    fi

    echo -e "\n  ${BOLD}📄 config-sub-node${N}.json${NC}"
    NODE_IP="${IPS[$N]}"

    # meta_node_rpc_address: thay IP, giữ port
    safe_sed "$SUB_JSON" \
        "s|\"meta_node_rpc_address\": \"[0-9.]*:|\"meta_node_rpc_address\": \"${NODE_IP}:|" \
        "meta_node_rpc_address IP → ${NODE_IP}"

    # nodes.master_address: thay IP, giữ port
    # Kết nối Sub → Master trên cùng máy → dùng IP của node đó
    safe_sed "$SUB_JSON" \
        "s|\"master_address\": \"[0-9.]*:|\"master_address\": \"${NODE_IP}:|" \
        "master_address IP → ${NODE_IP}"
done

# ─── Update TPS Blast Scripts ────────────────────────────────────────────────
echo -e "\n${BOLD}═══ TPS Blast Scripts (run_multinode_load.sh, run_node0_only_load.sh) ═══${NC}"

TPS_DIR="$GO_DIR/cmd/tool/tps_blast"

# Go Master connection ports per node (same as config)
GO_CONN_PORTS=(4201 6201 6211 6221 6241)
# Go RPC ports per node
GO_RPC_PORTS=(8757 10747 10749 10750 10748)

# Build NODES array string: "IP0:4201" "IP1:6201" "IP2:6211" "IP3:6221"
# Only use first 4 nodes (validators, exclude node4 SyncOnly)
NODES_STR=""
for P in 0 1 2 3; do
    if [ -n "$NODES_STR" ]; then
        NODES_STR="$NODES_STR "
    fi
    NODES_STR="${NODES_STR}\"${IPS[$P]}:${GO_CONN_PORTS[$P]}\""
done

# Build RPCS array string: "IP0:8757" "IP1:10747" "IP2:10749" "IP3:10750"
RPCS_STR=""
for P in 0 1 2 3; do
    if [ -n "$RPCS_STR" ]; then
        RPCS_STR="$RPCS_STR "
    fi
    # Node 0 uses localhost RPC since script runs on node 0
    if [ $P -eq 0 ]; then
        RPCS_STR="${RPCS_STR}\"127.0.0.1:${GO_RPC_PORTS[$P]}\""
    else
        RPCS_STR="${RPCS_STR}\"${IPS[$P]}:${GO_RPC_PORTS[$P]}\""
    fi
done

# run_multinode_load.sh
MULTI_SH="$TPS_DIR/run_multinode_load.sh"
if [ -f "$MULTI_SH" ]; then
    echo -e "\n  ${BOLD}📄 run_multinode_load.sh${NC}"

    # NODES array
    safe_sed "$MULTI_SH" \
        "s|^NODES=(.*)|NODES=(${NODES_STR})|" \
        "NODES → (${NODES_STR})"

    # RPCS array (each client verifies against its node's RPC)
    safe_sed "$MULTI_SH" \
        "s|^RPCS=(.*)|RPCS=(${RPCS_STR})|" \
        "RPCS → (${RPCS_STR})"

    # block_hash_checker: master=node0 (localhost), compare with node3 (remote)
    safe_sed "$MULTI_SH" \
        "s|-nodes \"master=http://[^\"]*\"|-nodes \"master=http://127.0.0.1:${GO_RPC_PORTS[0]},node3=http://${IPS[3]}:${GO_RPC_PORTS[3]}\"|" \
        "hash_checker → master=127.0.0.1:${GO_RPC_PORTS[0]}, node3=${IPS[3]}:${GO_RPC_PORTS[3]}"
fi

# run_node0_only_load.sh
NODE0_SH="$TPS_DIR/run_node0_only_load.sh"
if [ -f "$NODE0_SH" ]; then
    echo -e "\n  ${BOLD}📄 run_node0_only_load.sh${NC}"

    safe_sed "$NODE0_SH" \
        "s|^NODES=(.*)|NODES=(${NODES_STR})|" \
        "NODES → (${NODES_STR})"

    # -rpc parameter (use node0's RPC port for this script, localhost since we run on node0)
    safe_sed "$NODE0_SH" \
        "s|-rpc \"[0-9.]*:[0-9]*\"|-rpc \"127.0.0.1:${GO_RPC_PORTS[0]}\"|" \
        "-rpc → 127.0.0.1:${GO_RPC_PORTS[0]}"
fi

# ─── Update Tool Config JSONs ────────────────────────────────────────────────
echo -e "\n${BOLD}═══ Tool Configs (tx_sender, tps_blast, tps_benchmark_multi_node) ═══${NC}"

# tx_sender/config.json — connects to node0 Sub port
TX_SENDER_CFG="$GO_DIR/cmd/tool/tx_sender/config.json"
if [ -f "$TX_SENDER_CFG" ]; then
    echo -e "\n  ${BOLD}📄 tx_sender/config.json${NC}"
    safe_sed "$TX_SENDER_CFG" \
        "s|\"parent_connection_address\": \"[^\"]*\"|\"parent_connection_address\": \"${IPS[0]}:${GO_SUB_CONN_PORTS[0]}\"|" \
        "parent_connection_address → ${IPS[0]}:${GO_SUB_CONN_PORTS[0]}"
fi

# tps_blast/config.json — connects to node0 Master port
TPS_BLAST_CFG="$TPS_DIR/config.json"
if [ -f "$TPS_BLAST_CFG" ]; then
    echo -e "\n  ${BOLD}📄 tps_blast/config.json${NC}"
    safe_sed "$TPS_BLAST_CFG" \
        "s|\"parent_connection_address\": \"[^\"]*\"|\"parent_connection_address\": \"${IPS[0]}:${GO_MASTER_CONN_PORTS[0]}\"|" \
        "parent_connection_address → ${IPS[0]}:${GO_MASTER_CONN_PORTS[0]}"
fi

# tps_benchmark_multi_node/config.json — connects to node0 Sub port
TPS_BENCH_CFG="$GO_DIR/cmd/tool/tps_benchmark_multi_node/config.json"
if [ -f "$TPS_BENCH_CFG" ]; then
    echo -e "\n  ${BOLD}📄 tps_benchmark_multi_node/config.json${NC}"
    safe_sed "$TPS_BENCH_CFG" \
        "s|\"parent_connection_address\": \"[^\"]*\"|\"parent_connection_address\": \"${IPS[0]}:${GO_SUB_CONN_PORTS[0]}\"|" \
        "parent_connection_address → ${IPS[0]}:${GO_SUB_CONN_PORTS[0]}"
fi

# ─── Update Genesis JSON ─────────────────────────────────────────────────────
echo -e "\n${BOLD}═══ Genesis (genesis.json) ═══${NC}"

GENESIS="$GO_CONFIG_DIR/genesis.json"
if [ -f "$GENESIS" ]; then
    echo -e "\n  ${BOLD}📄 genesis.json${NC}"

    # Validator port mapping: primary, worker, p2p(tcp)
    GENESIS_PRIMARY_PORTS=(4000 4100 4200 4300)
    GENESIS_WORKER_PORTS=(4012 4112 4212 4312)
    GENESIS_P2P_PORTS=(9000 9011 9002 9003)

    for V in 0 1 2 3; do
        V_IP="${IPS[$V]}"

        # primary_address: "IP:PORT"
        safe_sed "$GENESIS" \
            "s|\"primary_address\": \"[0-9.]*:${GENESIS_PRIMARY_PORTS[$V]}\"|\"primary_address\": \"${V_IP}:${GENESIS_PRIMARY_PORTS[$V]}\"|" \
            "validator $V primary_address → ${V_IP}:${GENESIS_PRIMARY_PORTS[$V]}"

        # worker_address: "IP:PORT"
        safe_sed "$GENESIS" \
            "s|\"worker_address\": \"[0-9.]*:${GENESIS_WORKER_PORTS[$V]}\"|\"worker_address\": \"${V_IP}:${GENESIS_WORKER_PORTS[$V]}\"|" \
            "validator $V worker_address → ${V_IP}:${GENESIS_WORKER_PORTS[$V]}"

        # p2p_address: "/ip4/IP/tcp/PORT"
        safe_sed "$GENESIS" \
            "s|\"p2p_address\": \"/ip4/[0-9.]*/tcp/${GENESIS_P2P_PORTS[$V]}\"|\"p2p_address\": \"/ip4/${V_IP}/tcp/${GENESIS_P2P_PORTS[$V]}\"|" \
            "validator $V p2p_address → /ip4/${V_IP}/tcp/${GENESIS_P2P_PORTS[$V]}"
    done

    # epoch_timestamp_ms: rounded down to the previous hour boundary
    # This ensures multiple runs within the same hour produce the same timestamp
    CURRENT_EPOCH_SEC=$(( $(date +%s) / 3600 * 3600 ))
    CURRENT_TS_MS="${CURRENT_EPOCH_SEC}000"
    DISPLAY_TIME=$(date -d "@${CURRENT_EPOCH_SEC}" '+%Y-%m-%d %H:%M:%S')
    safe_sed "$GENESIS" \
        "s|\"epoch_timestamp_ms\": [0-9]*|\"epoch_timestamp_ms\": ${CURRENT_TS_MS}|" \
        "epoch_timestamp_ms → ${CURRENT_TS_MS} (${DISPLAY_TIME})"
else
    echo -e "  ${YELLOW}⚠️  genesis.json không tồn tại${NC}"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}═══════════════════════════════════════════════════════════${NC}"
if $DRY_RUN; then
    echo -e "${YELLOW}📊 DRY RUN hoàn tất: ${CHANGED_COUNT} thay đổi sẽ được áp dụng${NC}"
    echo -e "${CYAN}   Chạy lại không có --dry-run để ghi file thật.${NC}"
else
    echo -e "${GREEN}✅ Hoàn tất: ${CHANGED_COUNT} thay đổi đã được áp dụng${NC}"
fi
echo ""

# ─── Verify ──────────────────────────────────────────────────────────────────
if ! $DRY_RUN; then
    echo -e "${BOLD}🔍 Verification — IP hiện tại trong configs:${NC}"
    echo ""

    echo -e "  ${CYAN}Rust (network_address):${NC}"
    for N in $(seq 0 $((NODE_COUNT - 1))); do
        TOML="$RUST_CONFIG_DIR/node_${N}.toml"
        if [ -f "$TOML" ]; then
            ADDR=$(grep '^network_address' "$TOML" | sed 's/.*= "//;s/"//')
            echo -e "    Node $N: ${GREEN}${ADDR}${NC}"
        fi
    done

    echo ""
    echo -e "  ${CYAN}Rust (peer_rpc_addresses):${NC}"
    for N in $(seq 0 $((NODE_COUNT - 1))); do
        TOML="$RUST_CONFIG_DIR/node_${N}.toml"
        if [ -f "$TOML" ]; then
            PEERS=$(grep '^peer_rpc_addresses' "$TOML" | sed 's/.*= //')
            echo -e "    Node $N: ${GREEN}${PEERS}${NC}"
        fi
    done

    echo ""
    echo -e "  ${CYAN}Go (meta_node_rpc_address):${NC}"
    for N in $(seq 0 $((NODE_COUNT - 1))); do
        MASTER_JSON="$GO_CONFIG_DIR/config-master-node${N}.json"
        if [ -f "$MASTER_JSON" ]; then
            ADDR=$(grep 'meta_node_rpc_address' "$MASTER_JSON" | sed 's/.*: "//;s/".*//' | head -1 || true)
            if [ -n "$ADDR" ]; then
                echo -e "    Master $N: ${GREEN}${ADDR}${NC}"
            fi
        fi
        SUB_JSON="$GO_CONFIG_DIR/config-sub-node${N}.json"
        if [ -f "$SUB_JSON" ]; then
            ADDR=$(grep 'meta_node_rpc_address' "$SUB_JSON" | sed 's/.*: "//;s/".*//' | head -1 || true)
            if [ -n "$ADDR" ]; then
                echo -e "    Sub $N:    ${GREEN}${ADDR}${NC}"
            fi
        fi
    done
fi
