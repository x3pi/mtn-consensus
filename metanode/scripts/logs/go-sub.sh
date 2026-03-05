#!/usr/bin/env bash
# Tail Go Sub log for any node (latest epoch App.log)
# Usage: ./go-sub.sh <node_id> [lines|-f]
#   ./go-sub.sh 0          # Last 50 lines
#   ./go-sub.sh 1 200      # Last 200 lines
#   ./go-sub.sh 2 -f       # Follow mode

NODE_ID="${1:?Usage: $0 <node_id> [lines|-f]}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOG_DIR="$METANODE_ROOT/logs/node_${NODE_ID}/go-sub"
STDOUT_LOG="$METANODE_ROOT/logs/node_${NODE_ID}/go-sub-stdout.log"
ARG="${2:-50}"

# Try epoch-based App.log first, fallback to stdout log
LOGFILE=$(ls -td "$LOG_DIR"/epoch_*/App.log 2>/dev/null | head -1)
if [ -z "$LOGFILE" ]; then
    LOGFILE="$STDOUT_LOG"
fi

if [ ! -f "$LOGFILE" ]; then
    echo "❌ Log not found for Node $NODE_ID"
    echo "   Tried: $LOG_DIR/epoch_*/App.log"
    echo "   Tried: $STDOUT_LOG"
    exit 1
fi

if [ "$ARG" = "-f" ]; then
    echo "📋 Following Go Sub (Node $NODE_ID): $LOGFILE"
    tail -f "$LOGFILE"
else
    echo "📋 Go Sub (Node $NODE_ID) — last $ARG lines: $LOGFILE"
    tail -n "$ARG" "$LOGFILE"
fi
