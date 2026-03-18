#!/bin/bash
# Usage: ./stop_node.sh <node_id>
# Gracefully stop a single node (Go Master + Go Sub + Rust Metanode)
# Uses tmux send-keys to send SIGTERM directly to processes in their sessions

set -e

NODE_ID="${1:?Usage: $0 <node_id> (0-4)}"

# Validate
if [[ ! "$NODE_ID" =~ ^[0-4]$ ]]; then
    echo "❌ Invalid node_id: $NODE_ID (must be 0-4)"
    exit 1
fi

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}🛑 Stopping Node $NODE_ID...${NC}"

# Session names for THIS node only
GO_MASTER_SESSION="go-master-${NODE_ID}"
GO_SUB_SESSION="go-sub-${NODE_ID}"
RUST_SESSION="metanode-${NODE_ID}"

# Socket paths for THIS node only
GO_MASTER_SOCKET="/tmp/rust-go-node${NODE_ID}-master.sock"
EXECUTOR_SOCKET="/tmp/executor${NODE_ID}.sock"
TX_SOCKET="/tmp/metanode-tx-${NODE_ID}.sock"

# ─── Step 1: Send Ctrl+C to tmux sessions (SIGINT → graceful shutdown) ───
# This ensures ONLY the processes in these specific sessions are stopped
for sess in "$GO_MASTER_SESSION" "$GO_SUB_SESSION" "$RUST_SESSION"; do
    if tmux has-session -t "$sess" 2>/dev/null; then
        echo -e "${YELLOW}  📤 Sending Ctrl+C to $sess...${NC}"
        tmux send-keys -t "$sess" C-c 2>/dev/null || true
    fi
done

# Wait for graceful shutdown (PebbleDB FlushAll, etc.)
echo "  ⏳ Waiting 10s for graceful shutdown (FlushAll)..."
sleep 10

# ─── Step 2: Kill tmux sessions (cleanup) ────────────────────
for sess in "$GO_MASTER_SESSION" "$GO_SUB_SESSION" "$RUST_SESSION"; do
    if tmux has-session -t "$sess" 2>/dev/null; then
        echo -e "${YELLOW}  🔪 Killing session $sess...${NC}"
        tmux kill-session -t "$sess" 2>/dev/null || true
    fi
done

# ─── Step 3: Clean sockets ───────────────────────────────────
rm -f "$GO_MASTER_SOCKET" "$EXECUTOR_SOCKET" "$TX_SOCKET" 2>/dev/null || true

echo -e "${GREEN}✅ Node $NODE_ID stopped.${NC}"
