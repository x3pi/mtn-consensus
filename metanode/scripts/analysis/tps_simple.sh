#!/bin/bash
# Script TPS đơn giản - chỉ hiện số liệu cơ bản
# Cách dùng: ./tps_simple.sh [node_port]

NODE_PORT=${1:-9103}
INTERVAL=5
TX_PER_BLOCK=10

echo "📊 TPS Monitor - Node $NODE_PORT (TX/block: $TX_PER_BLOCK)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

prev_committed=0
prev_time=$(date +%s)

while true; do
    # Try different metric names
    committed=$(curl -s "http://localhost:$NODE_PORT/metrics" | grep "committed_leaders_total" | awk '{sum += $2} END {print sum}' || echo "0")
    if [ "$committed" = "0" ]; then
        committed=$(curl -s "http://localhost:$NODE_PORT/metrics" | grep "committed_leaders{" | awk '{sum += $2} END {print sum}' || echo "0")
    fi
    current_time=$(date +%s)

    if [ "$prev_committed" != "0" ] || [ "$committed" != "0" ]; then
        time_diff=$((current_time - prev_time))
        if [ "$time_diff" != "0" ]; then
            blocks_per_sec=$(echo "scale=2; ($committed - $prev_committed) / $time_diff" | bc -l 2>/dev/null || echo "0.00")
            tps=$(echo "scale=1; $blocks_per_sec * $TX_PER_BLOCK" | bc -l 2>/dev/null || echo "0.0")

            echo "$(date '+%H:%M:%S') - Blocks/sec: $blocks_per_sec, TPS: $tps, Total committed: $committed"
        fi
    fi

    prev_committed=$committed
    prev_time=$current_time
    sleep $INTERVAL
done
