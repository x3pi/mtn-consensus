#!/bin/bash
# mtn-ops.sh — MetaNode Production Operations CLI
# Usage: mtn-ops <command> [node_id] [options]

set -e

COMMAND=${1:-}
NODE_ID=${2:-0}
OPTION=${3:-}

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

if [ "$EUID" -ne 0 ]; then
  echo -e "${RED}Please run as root (sudo)${NC}"
  exit 1
fi

log_info() { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_step() { echo -e "  ${CYAN}►${NC} $*"; }

show_help() {
    echo -e "${BOLD}========================================${NC}"
    echo -e "${BOLD}  MetaNode Operations CLI (mtn-ops)     ${NC}"
    echo -e "${BOLD}========================================${NC}"
    echo "Usage: mtn-ops <command> [node_id] [options]"
    echo ""
    echo "Commands:"
    echo "  start   [node]       Start node safely (Master -> Sub -> Rust)"
    echo "  stop    [node]       Stop node safely (Rust -> Sub -> Master)"
    echo "  restart [node]       Restart node securely"
    echo "  status  [node]       Check running status of systemctl services"
    echo "  logs    [node] [l]   View logs (l = all, master, sub, rust)"
    echo "  clean   [node]       Wipe local DB data for a fresh resync"
    echo ""
    echo "Defaults:"
    echo "  node_id : 0 (For single-node servers)"
    echo "  layer   : all (For logs)"
}

case "$COMMAND" in
    start)
        echo -e "${BOLD}🚀 Starting Node $NODE_ID${NC}"
        
        log_step "Starting Go Master (metanode-go-master@${NODE_ID})..."
        systemctl start metanode-go-master@${NODE_ID} || log_warn "Failed to start go-master"
        sleep 2
        
        log_step "Starting Go Sub (metanode-go-sub@${NODE_ID})..."
        systemctl start metanode-go-sub@${NODE_ID} || log_warn "Failed to start go-sub"
        sleep 1
        
        log_step "Starting Rust Consensus (metanode-rust@${NODE_ID})..."
        systemctl start metanode-rust@${NODE_ID} || log_warn "Failed to start rust"
        
        log_info "Node $NODE_ID start sequence completed."
        ;;
        
    stop)
        echo -e "${BOLD}🛑 Stopping Node $NODE_ID${NC}"
        
        log_step "Stopping Rust Consensus (Stopping block production)..."
        systemctl stop metanode-rust@${NODE_ID} || true
        
        log_step "Waiting 5s for Go pipeline to flush...⏳"
        sleep 5
        
        log_step "Stopping Go Sub..."
        systemctl stop metanode-go-sub@${NODE_ID} || true
        
        log_step "Stopping Go Master..."
        systemctl stop metanode-go-master@${NODE_ID} || true
        
        log_info "Node $NODE_ID stopped safely."
        ;;
        
    restart)
        $0 stop $NODE_ID
        sleep 2
        $0 start $NODE_ID
        ;;
        
    status)
        echo -e "${BOLD}📊 Status of Node $NODE_ID${NC}"
        echo -e "\n${CYAN}--- Rust Consensus ---${NC}"
        systemctl --no-pager status metanode-rust@${NODE_ID} | grep -E "Active:|Main PID" || true
        echo -e "\n${CYAN}--- Go Master ---${NC}"
        systemctl --no-pager status metanode-go-master@${NODE_ID} | grep -E "Active:|Main PID" || true
        echo -e "\n${CYAN}--- Go Sub ---${NC}"
        systemctl --no-pager status metanode-go-sub@${NODE_ID} | grep -E "Active:|Main PID" || true
        echo ""
        ;;
        
    logs)
        LAYER=${OPTION:-all}
        echo -e "${BOLD}📋 Viewing Logs for Node $NODE_ID ($LAYER)${NC}"
        if [ "$LAYER" == "master" ]; then
            journalctl -u metanode-go-master@${NODE_ID} -f
        elif [ "$LAYER" == "sub" ]; then
            journalctl -u metanode-go-sub@${NODE_ID} -f
        elif [ "$LAYER" == "rust" ]; then
            journalctl -u metanode-rust@${NODE_ID} -f
        else
            # Tích hợp toàn bộ logs của Node
            journalctl -u metanode-go-master@${NODE_ID} -u metanode-go-sub@${NODE_ID} -u metanode-rust@${NODE_ID} -f
        fi
        ;;
        
    clean)
        echo -e "${RED}${BOLD}⚠️  WARNING: You are about to DELETE all storage data for Node $NODE_ID!${NC}"
        echo -e "${YELLOW}This action cannot be undone. Do this only if you want to resync from scratch.${NC}"
        read -p "Are you sure? (Type 'yes' to confirm): " confirm
        if [ "$confirm" == "yes" ]; then
            log_step "Stopping all services for Node $NODE_ID first..."
            $0 stop $NODE_ID
            
            # Extract paths from systemd unit definition safely
            RUST_DIR=$(systemctl cat metanode-rust@${NODE_ID} 2>/dev/null | grep WorkingDirectory | cut -d '=' -f 2 || echo "")
            GO_DIR=$(systemctl cat metanode-go-master@${NODE_ID} 2>/dev/null | grep WorkingDirectory | cut -d '=' -f 2 || echo "")
            
            if [ -n "$RUST_DIR" ] && [ -d "$RUST_DIR" ]; then
                log_step "Wiping Rust Storage at $RUST_DIR/config/storage/node_${NODE_ID}"
                rm -rf "$RUST_DIR/config/storage/node_${NODE_ID}"
            else
                log_warn "RUST_DIR not detected or invalid: '$RUST_DIR'. Skipping Rust wipe."
            fi
            
            if [ -n "$GO_DIR" ] && [ -d "$GO_DIR" ]; then
                log_step "Wiping Go Database at $GO_DIR/sample/node${NODE_ID}"
                rm -rf "$GO_DIR/sample/node${NODE_ID}/data"
                rm -rf "$GO_DIR/sample/node${NODE_ID}/back_up"
                rm -rf "$GO_DIR/sample/node${NODE_ID}/data-write"
                rm -rf "$GO_DIR/sample/node${NODE_ID}/back_up_write"
            else
                log_warn "GO_DIR not detected or invalid: '$GO_DIR'. Skipping Go wipe."
            fi
            
            log_info "Node data wiped. Ready to resync."
        else
            log_info "Operation cancelled."
        fi
        ;;
        
    *)
        show_help
        ;;
esac
