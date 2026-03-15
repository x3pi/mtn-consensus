#!/bin/bash
# Usage: ./stop_node_ai.sh <node_id>
# Gracefully stop a single node (AI-friendly, no tmux dependency, uses nohup/bg process matching)

set -e

NODE_ID="${1:?Usage: $0 <node_id> (0-4)}"

if [[ ! "$NODE_ID" =~ ^[0-4]$ ]]; then
    echo "❌ Invalid node_id: $NODE_ID (must be 0-4)"
    exit 1
fi

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}🛑 Stopping Node $NODE_ID (AI Version)...${NC}"

# 1. Ask processes to shut down gracefully
echo -e "${YELLOW}  📤 Sending SIGINT to node $NODE_ID processes...${NC}"

# Find go-master
MASTER_PIDS=$(pgrep -f "simple_chain -config=config-master-node$NODE_ID.json" || true)
if [ -n "$MASTER_PIDS" ]; then
    kill -SIGINT $MASTER_PIDS 2>/dev/null || true
fi

# Find go-sub
SUB_PIDS=$(pgrep -f "simple_chain -config=config-sub-node$NODE_ID.json" || true)
if [ -n "$SUB_PIDS" ]; then
    kill -SIGINT $SUB_PIDS 2>/dev/null || true
fi

# Find rust metanode
RUST_PIDS=$(pgrep -f "metanode start --config config/node_$NODE_ID.toml" || true)
if [ -n "$RUST_PIDS" ]; then
    kill -SIGINT $RUST_PIDS 2>/dev/null || true
fi

# Fallback: kill tmux if it still exists from human user
for sess in "go-master-$NODE_ID" "go-sub-$NODE_ID" "metanode-$NODE_ID"; do
    if tmux has-session -t "$sess" 2>/dev/null; then
        tmux send-keys -t "$sess" C-c 2>/dev/null || true
    fi
done

echo "  ⏳ Waiting 5s for graceful shutdown..."
sleep 5

# 2. Force kill remaining and clean up tmux sessions
for sess in "go-master-$NODE_ID" "go-sub-$NODE_ID" "metanode-$NODE_ID"; do
    if tmux has-session -t "$sess" 2>/dev/null; then
        tmux kill-session -t "$sess" 2>/dev/null || true
    fi
done

pkill -9 -f "config-master-node$NODE_ID.json" 2>/dev/null || true
pkill -9 -f "config-sub-node$NODE_ID.json" 2>/dev/null || true
pkill -9 -f "config/node_$NODE_ID.toml" 2>/dev/null || true

# 3. Clean sockets
rm -f "/tmp/rust-go-node${NODE_ID}-master.sock" "/tmp/executor${NODE_ID}.sock" "/tmp/metanode-tx-${NODE_ID}.sock" 2>/dev/null || true

echo -e "${GREEN}✅ Node $NODE_ID stopped.${NC}"
