#!/usr/bin/env bash
# Tail Rust metanode log for any node
# Usage: ./rust.sh <node_id> [lines|-f]
#   ./rust.sh 0          # Last 50 lines
#   ./rust.sh 0 200      # Last 200 lines
#   ./rust.sh 0 -f       # Follow mode

NODE_ID="${1:?Usage: $0 <node_id> [lines|-f]}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOGFILE="$METANODE_ROOT/logs/node_${NODE_ID}/rust.log"
ARG="${2:-50}"

if [ ! -f "$LOGFILE" ]; then
    echo "❌ Log not found: $LOGFILE"
    exit 1
fi

if [ "$ARG" = "-f" ]; then
    echo "🦀 Following Rust Metanode (Node $NODE_ID): $LOGFILE"
    tail -f "$LOGFILE"
else
    echo "🦀 Rust Metanode (Node $NODE_ID) — last $ARG lines: $LOGFILE"
    tail -n "$ARG" "$LOGFILE"
fi
