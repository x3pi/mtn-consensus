#!/bin/bash
set -euo pipefail

NODE_PORT=${1:-9103}
INTERVAL=${2:-5}
METRICS_URL="http://localhost:$NODE_PORT/metrics"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "🚀 ${YELLOW}METANODE ANALYSIS FIXED${NC} (Port: $NODE_PORT)"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

prev_tx=0
prev_blocks=0
prev_time=$(date +%s.%N)

printf "│ %-10s │ %-10s │ %-10s │ %-12s │ %-8s │\n" "Thời gian" "TPS (Thực)" "BPS" "Lat. (Avg)" "Total TX"
echo "├────────────┼────────────┼────────────┼──────────────┼──────────┤"

while true; do
    raw_metrics=$(curl -s "$METRICS_URL")
    current_time=$(date +%s.%N)
    
    if [ -z "$raw_metrics" ]; then
        echo -e "│ $(date '+%H:%M:%S') │ ${RED}Lỗi: Không thể kết nối tới Node${NC}             │"
        sleep $INTERVAL
        continue
    fi

    # 1. Trích xuất và Cộng dồn dữ liệu (Sửa lỗi awk tại đây)
    current_tx=$(echo "$raw_metrics" | grep "^certifier_accepted_transactions" | awk '{sum += $2} END {printf "%.0f", sum}')
    current_blocks=$(echo "$raw_metrics" | grep "^accepted_blocks" | awk '{sum += $2} END {printf "%.0f", sum}')
    
    # Lấy dữ liệu Latency (tránh lỗi đa dòng bằng cách cộng dồn trước)
    l_sum=$(echo "$raw_metrics" | grep "^block_commit_latency_sum" | awk '{sum += $2} END {print sum}')
    l_count=$(echo "$raw_metrics" | grep "^block_commit_latency_count" | awk '{sum += $2} END {print sum}')
    
    # Gán giá trị mặc định nếu rỗng để tránh lỗi awk
    l_sum=${l_sum:-0}
    l_count=${l_count:-0}

    # 2. Tính toán Delta
    time_diff=$(awk "BEGIN {print $current_time - $prev_time}")
    
    # Kiểm tra nếu đã có dữ liệu từ vòng lặp trước
    if (( $(echo "$prev_tx >= 0" | bc -l) )) && [ "$prev_tx" != "0" ]; then
        delta_tx=$(awk "BEGIN {print $current_tx - $prev_tx}")
        delta_blocks=$(awk "BEGIN {print $current_blocks - $prev_blocks}")
        
        tps=$(awk "BEGIN {if ($time_diff > 0) printf \"%.2f\", $delta_tx / $time_diff; else print \"0.00\"}")
        bps=$(awk "BEGIN {if ($time_diff > 0) printf \"%.2f\", $delta_blocks / $time_diff; else print \"0.00\"}")
        
        # Tính Latency trung bình (ms)
        avg_lat_ms=$(awk "BEGIN {if ($l_count > 0) printf \"%.1f\", ($l_sum / $l_count) * 1000; else print \"0.0\"}")

        tps_color=$NC
        if (( $(echo "$tps > 0" | bc -l) )); then tps_color=$GREEN; fi

        timestamp=$(date '+%H:%M:%S')
        printf "│ %-10s │ ${tps_color}%-10.2f${NC} │ %-10.2f │ %-10s ms │ %-8d │\n" \
               "$timestamp" "$tps" "$bps" "$avg_lat_ms" "$current_tx"
    else
        # Lần chạy đầu tiên chỉ lưu giá trị, không tính toán
        timestamp=$(date '+%H:%M:%S')
        printf "│ %-10s │ ${YELLOW}%-10s${NC} │ %-10s │ %-12s │ %-8d │\n" \
               "$timestamp" "Warmup..." "Wait..." "Calculating" "$current_tx"
    fi

    prev_tx=$current_tx
    prev_blocks=$current_blocks
    prev_time=$current_time

    sleep "$INTERVAL"
done