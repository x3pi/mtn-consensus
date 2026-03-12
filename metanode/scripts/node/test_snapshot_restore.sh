#!/bin/bash

# Configuration
NODE_ID=2
SIMPLE_CHAIN_DIR="/home/abc/chain-n/mtn-simple-2025/cmd/simple_chain"

echo "🛑 Stopping Node $NODE_ID processes..."
tmux kill-session -t metanode-$NODE_ID 2>/dev/null || true
pkill -f "config-master-node$NODE_ID.json" 2>/dev/null || true
pkill -f "config-sub-node$NODE_ID.json" 2>/dev/null || true

# Wait for processes to release file locks
sleep 3

echo "🗑️ Clearing Node $NODE_ID database state..."
rm -rf "$SIMPLE_CHAIN_DIR/sample/node$NODE_ID/data"
rm -rf "$SIMPLE_CHAIN_DIR/snapshot_data_node$NODE_ID"

echo "🔍 Finding latest snapshot from Node 0..."
# Lấy snapshot mới nhất dựa trên tên thư mục
LATEST_SNAPSHOT=$(ls -dt $SIMPLE_CHAIN_DIR/snapshot_data_node0/snap_epoch_*_block_* 2>/dev/null | head -n 1)

if [ -z "$LATEST_SNAPSHOT" ]; then
    echo "❌ No snapshot found in Node 0. Has it crossed the epoch block boundary yet?"
    exit 1
fi

echo "📦 Found latest snapshot: $LATEST_SNAPSHOT"
echo "📥 'Downloading' (Copying) snapshot to Node $NODE_ID..."

# Khôi phục dữ liệu từ snapshot vào Node 2
mkdir -p "$SIMPLE_CHAIN_DIR/sample/node$NODE_ID/data/data"
cp -r "$LATEST_SNAPSHOT/"* "$SIMPLE_CHAIN_DIR/sample/node$NODE_ID/data/data/"

echo "✅ Snapshot state securely transferred! Node $NODE_ID now possesses epoch synced data."

echo "🚀 Restarting Node $NODE_ID..."
tmux new-session -d -s metanode-$NODE_ID

# Start go-master
tmux send-keys -t metanode-$NODE_ID "cd $SIMPLE_CHAIN_DIR && ./simple_chain -config ./config-master-node$NODE_ID.json 2>&1 | tee /home/abc/chain-n/mtn-consensus/metanode/logs/node_$NODE_ID/go-master-stdout.log" C-m
sleep 1

# Start go-sub 
tmux split-window -h -t metanode-$NODE_ID
tmux send-keys -t metanode-$NODE_ID "cd $SIMPLE_CHAIN_DIR && ./simple_chain -config ./config-sub-node$NODE_ID.json 2>&1 | tee /home/abc/chain-n/mtn-consensus/metanode/logs/node_$NODE_ID/go-sub-stdout.log" C-m
sleep 1

# Start rust consensus
tmux split-window -v -t metanode-$NODE_ID
tmux send-keys -t metanode-$NODE_ID "cd /home/abc/chain-n/mtn-consensus/metanode && RUST_LOG=info ./target/release/metanode start --config config/node_$NODE_ID.toml 2>&1 | tee /home/abc/chain-n/mtn-consensus/metanode/logs/node_$NODE_ID/rust.log" C-m

echo "✅ Node $NODE_ID successfully restarted from snapshot!"
echo "Use 'tmux attach -t metanode-$NODE_ID' to monitor consensus restoration."
