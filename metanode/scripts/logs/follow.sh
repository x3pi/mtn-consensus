#!/usr/bin/env bash
# Follow ALL logs for a specific node in parallel
# Usage: ./follow.sh <node_id>
# Example: ./follow.sh 0    # Follow all node 0 logs

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOGS_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)/logs"

if [ $# -eq 0 ]; then
    echo "Usage: $0 <node_id>"
    echo "  Follow all log files for a specific node in real-time"
    echo "  node_id: 0, 1, 2, 3, or 4"
    echo ""
    echo "Log structure:"
    echo "  logs/node_N/"
    echo "    ├── rust.log"
    echo "    ├── go-master-stdout.log"
    echo "    ├── go-sub-stdout.log"
    echo "    ├── go-master/epoch_N/App.log"
    echo "    └── go-sub/epoch_N/App.log"
    exit 1
fi

NODE_ID="$1"
NODE_DIR="$LOGS_DIR/node_$NODE_ID"

if [ ! -d "$NODE_DIR" ]; then
    echo "❌ Node directory not found: $NODE_DIR"
    exit 1
fi

# Build list of log files
FILES=()

# Rust log
[ -f "$NODE_DIR/rust.log" ] && FILES+=("$NODE_DIR/rust.log")

# Go master — find latest epoch App.log, fallback to stdout
GO_MASTER_LOG=$(ls -td "$NODE_DIR"/go-master/epoch_*/App.log 2>/dev/null | head -1 || true)
if [ -n "$GO_MASTER_LOG" ]; then
    FILES+=("$GO_MASTER_LOG")
elif [ -f "$NODE_DIR/go-master-stdout.log" ]; then
    FILES+=("$NODE_DIR/go-master-stdout.log")
fi

# Go sub — find latest epoch App.log, fallback to stdout
GO_SUB_LOG=$(ls -td "$NODE_DIR"/go-sub/epoch_*/App.log 2>/dev/null | head -1 || true)
if [ -n "$GO_SUB_LOG" ]; then
    FILES+=("$GO_SUB_LOG")
elif [ -f "$NODE_DIR/go-sub-stdout.log" ]; then
    FILES+=("$NODE_DIR/go-sub-stdout.log")
fi

if [ ${#FILES[@]} -eq 0 ]; then
    echo "❌ No log files found for node $NODE_ID in $NODE_DIR"
    ls -la "$NODE_DIR" 2>/dev/null
    exit 1
fi

echo "📋 Following ${#FILES[@]} log files for Node $NODE_ID..."
for f in "${FILES[@]}"; do
    echo "   📄 $f"
done
echo "   Press Ctrl+C to stop"
echo ""

tail -f "${FILES[@]}"
