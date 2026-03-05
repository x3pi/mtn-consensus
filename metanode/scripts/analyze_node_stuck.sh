#!/bin/bash

# Script phân tích chi tiết vấn đề node bị stuck (commit processor không xử lý commits)

set -e

LOG_DIR="${1:-logs/latest}"
NODE_ID="${2:-0}"

echo "=========================================="
echo "🔍 PHÂN TÍCH NODE STUCK - Node $NODE_ID"
echo "=========================================="
echo ""

LOG_FILE="$LOG_DIR/node_${NODE_ID}.log"

if [ ! -f "$LOG_FILE" ]; then
    echo "❌ Không tìm thấy log file: $LOG_FILE"
    exit 1
fi

echo "📊 1. THỐNG KÊ TỔNG QUAN"
echo "----------------------------------------"
TOTAL_LINES=$(wc -l < "$LOG_FILE")
LAST_COMMIT_PROCESSED=$(grep -E "Executing commit|Global Index" "$LOG_FILE" | tail -1 | grep -oE "commit #[0-9]+" | grep -oE "[0-9]+" || echo "NONE")
LAST_CONSENSUS_COMMIT=$(grep -E "Consensus commit C[0-9]+" "$LOG_FILE" | tail -1 | grep -oE "C[0-9]+" | grep -oE "[0-9]+" || echo "NONE")

echo "  - Tổng số dòng log: $TOTAL_LINES"
echo "  - Commit cuối cùng được xử lý: #$LAST_COMMIT_PROCESSED"
echo "  - Commit cuối cùng từ consensus: #$LAST_CONSENSUS_COMMIT"

if [ "$LAST_COMMIT_PROCESSED" != "NONE" ] && [ "$LAST_CONSENSUS_COMMIT" != "NONE" ]; then
    GAP=$((LAST_CONSENSUS_COMMIT - LAST_COMMIT_PROCESSED))
    echo "  - ⚠️  GAP: $GAP commits chưa được xử lý!"
fi

echo ""
echo "📋 2. TIMELINE COMMIT PROCESSOR"
echo "----------------------------------------"
echo "  Commit đầu tiên:"
grep -E "Executing commit|Global Index" "$LOG_FILE" | head -3
echo ""
echo "  Commit cuối cùng:"
grep -E "Executing commit|Global Index" "$LOG_FILE" | tail -3

echo ""
echo "📋 3. TIMELINE CONSENSUS COMMITS"
echo "----------------------------------------"
echo "  Consensus commit gần đây:"
grep -E "Consensus commit C[0-9]+" "$LOG_FILE" | tail -5

echo ""
echo "📋 4. KIỂM TRA COMMIT SYNcer"
echo "----------------------------------------"
echo "  Trạng thái commit syncer gần đây:"
grep -E "Checking to schedule fetches|highest_handled_index" "$LOG_FILE" | tail -3

echo ""
echo "📋 5. KIỂM TRA EPOCH TRANSITION"
echo "----------------------------------------"
EPOCH_TRANSITIONS=$(grep -iE "epoch.*transition|EPOCH.*TRANSITION|transition.*epoch" "$LOG_FILE" | wc -l)
echo "  - Số lần epoch transition: $EPOCH_TRANSITIONS"
if [ "$EPOCH_TRANSITIONS" -gt 0 ]; then
    echo "  - Epoch transitions gần đây:"
    grep -iE "epoch.*transition|EPOCH.*TRANSITION|transition.*epoch" "$LOG_FILE" | tail -5
fi

echo ""
echo "📋 6. KIỂM TRA RECEIVER CHANNEL"
echo "----------------------------------------"
RECEIVER_CLOSED=$(grep -iE "receiver.*closed|Commit receiver|channel.*closed" "$LOG_FILE" | wc -l)
echo "  - Số lần receiver đóng: $RECEIVER_CLOSED"
if [ "$RECEIVER_CLOSED" -gt 0 ]; then
    echo "  - ⚠️  Receiver đã bị đóng:"
    grep -iE "receiver.*closed|Commit receiver|channel.*closed" "$LOG_FILE"
fi

echo ""
echo "📋 7. KIỂM TRA LỖI"
echo "----------------------------------------"
ERRORS=$(grep -iE "ERROR|WARN.*commit|fail.*commit|stuck|panic" "$LOG_FILE" | tail -10)
if [ -n "$ERRORS" ]; then
    echo "  - ⚠️  Có lỗi liên quan đến commit:"
    echo "$ERRORS"
else
    echo "  - ✅ Không có lỗi rõ ràng"
fi

echo ""
echo "📋 8. SO SÁNH VỚI NODE KHÁC"
echo "----------------------------------------"
for other_node in 1 2 3; do
    OTHER_LOG="$LOG_DIR/node_${other_node}.log"
    if [ -f "$OTHER_LOG" ]; then
        OTHER_LAST=$(grep -E "Executing commit|Global Index" "$OTHER_LOG" | tail -1 | grep -oE "commit #[0-9]+" | grep -oE "[0-9]+" || echo "NONE")
        echo "  - Node $other_node: Commit cuối = $OTHER_LAST"
    fi
done

echo ""
echo "=========================================="
echo "✅ PHÂN TÍCH HOÀN TẤT"
echo "=========================================="

