#!/bin/bash
# Usage: ./stop_all.sh
# Gracefully stop ALL nodes (0-4)

set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo ""
echo -e "${GREEN}🛑 Stopping ALL nodes...${NC}"
echo ""

# SIGTERM all Go and Rust processes first (for LevelDB flush)
echo -e "${YELLOW}📤 Sending SIGTERM to all processes...${NC}"
pkill -f "simple_chain" 2>/dev/null || true
pkill -f "metanode start" 2>/dev/null || true
pkill -f "metanode run" 2>/dev/null || true
pkill -f "tps_blast" 2>/dev/null || true

echo "⏳ Waiting 5s for graceful shutdown..."
sleep 5

# Kill all tmux sessions
echo -e "${YELLOW}🗑️  Cleaning tmux sessions...${NC}"
for id in 0 1 2 3 4; do
    tmux kill-session -t "go-master-$id" 2>/dev/null || true
    tmux kill-session -t "go-sub-$id" 2>/dev/null || true
    tmux kill-session -t "metanode-$id" 2>/dev/null || true
done
# Legacy session names
tmux kill-session -t "go-master" 2>/dev/null || true
tmux kill-session -t "go-sub" 2>/dev/null || true
tmux kill-session -t "metanode-1-sep" 2>/dev/null || true

# Force kill stragglers
pkill -9 -f "simple_chain" 2>/dev/null || true
pkill -9 -f "metanode start" 2>/dev/null || true
pkill -9 -f "metanode run" 2>/dev/null || true

# Clean all sockets
rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true

echo ""
echo -e "${GREEN}✅ All nodes stopped.${NC}"
echo ""
