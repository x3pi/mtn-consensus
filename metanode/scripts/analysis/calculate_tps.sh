#!/bin/bash
set -euo pipefail

# Tham số đầu vào
NODE_PORT=${1:-9103}
INTERVAL=${2:-5}
METRICS_URL="http://localhost:$NODE_PORT/metrics"

# Màu sắc
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}🚀 Đang tính toán TPS thực tế từ MetaNode (Port: $NODE_PORT)...${NC}"

# Khởi tạo giá trị cũ
prev_tx=0
prev_blocks=0
prev_time=$(date +%s.%N)

# Hàm lấy giá trị metric từ Prometheus text format
get_metric() {
    local metric_name=$1
    # Tìm metric, lấy giá trị cuối cùng, xử lý định dạng khoa học (e.g. 1.4e+02) nếu có
    curl -s "$METRICS_URL" | grep "^${metric_name}" | awk '{print $2}' | tail -n1 | xargs printf "%.0f" 2>/dev/null || echo "0"
}

echo "--------------------------------------------------------------------------------"
printf "| %-10s | %-10s | %-10s | %-10s | %-10s |\n" "Thời gian" "Block mới" "TX mới" "TPS Thực" "BPS"
echo "--------------------------------------------------------------------------------"

while true; do
    # 1. Lấy dữ liệu hiện tại
    # Lấy tổng số giao dịch đã được xác nhận (dùng certifier_accepted_transactions làm đại diện)
    current_tx=$(get_metric "certifier_accepted_transactions")
    # Lấy tổng số block đã được xác nhận (từ sum của histogram hoặc counter blocks)
    current_blocks=$(get_metric "accepted_blocks")
    current_time=$(date +%s.%N)

    # 2. Tính toán sai lệch (Delta)
    if [ $prev_tx -gt 0 ] || [ $prev_blocks -gt 0 ]; then
        delta_tx=$((current_tx - prev_tx))
        delta_blocks=$((current_blocks - prev_blocks))
        
        # Tính thời gian thực tế giữa 2 lần quét (giúp kết quả chính xác hơn sleep 5)
        time_diff=$(awk "BEGIN {print $current_time - $prev_time}")
        
        # 3. Tính TPS và BPS (Blocks Per Second)
        tps=$(awk "BEGIN {printf \"%.2f\", $delta_tx / $time_diff}")
        bps=$(awk "BEGIN {printf \"%.2f\", $delta_blocks / $time_diff}")

        # In kết quả
        timestamp=$(date '+%H:%M:%S')
        printf "| %-10s | %-10d | %-10d | ${GREEN}%-10s${NC} | %-10s |\n" \
               "$timestamp" "$delta_blocks" "$delta_tx" "$tps" "$bps"
    else
        echo "⏳ Đang thu thập dữ liệu đợt đầu..."
    fi

    # 4. Lưu giá trị cho vòng lặp sau
    prev_tx=$current_tx
    prev_blocks=$current_blocks
    prev_time=$current_time

    sleep "$INTERVAL"
done