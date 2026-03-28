#!/bin/bash
# Script để test thủ công (1 lần) quy trình khởi động lại và xử lý giao dịch
# Rất hữu ích khi cần debug lỗi đồng thuận hoặc lệch hash sau khi restart.

echo "=========================================================="
echo "🕹️  TEST KHỞI ĐỘNG LẠI CLUSTER (THỦ CÔNG 1 LẦN)        🕹️"
echo "=========================================================="

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
ORCHESTRATOR="$SCRIPT_DIR/mtn-orchestrator.sh"
TX_SENDER_DIR="$BASE_DIR/mtn-simple-2025/cmd/tool/tx_sender"

if [ ! -f "$ORCHESTRATOR" ]; then
    echo "❌ Không tìm thấy mtn-orchestrator.sh"
    exit 1
fi

echo "🛑 [1/4] Đang dừng cluster cũ (nếu có)..."
"$ORCHESTRATOR" stop > /dev/null 2>&1
sleep 3

echo "🚀 [2/4] Đang khởi động cluster với data cũ..."
"$ORCHESTRATOR" start > /dev/null 2>&1

echo "⏳ [3/4] Đang chờ 30 giây để cluster khôi phục DAG và nối Epoch..."
sleep 30

# Kiểm tra trạng thái chớp nhoáng
echo "🔎 Kiểm tra trạng thái cluster:"
"$ORCHESTRATOR" status

echo "💸 [4/4] Đang gửi giao dịch test (deploy, call, read)..."
cd "$TX_SENDER_DIR"

# Chạy tx_sender
TX_OUTPUT=$(GOTOOLCHAIN=go1.23.5 go run . 2>&1) || {
    echo ""
    echo "❌ GIAO DỊCH LỖI HOẶC HỆ THỐNG BỊ TREO THỜI GIAN DÀI!"
    echo "=========================================================="
    echo "$TX_OUTPUT"
    echo "=========================================================="
    echo "💡 Gợi ý Debug:"
    echo " 1. Kiểm tra log Master: $ORCHESTRATOR logs 0 master"
    echo " 2. Kiểm tra log Rust:    $ORCHESTRATOR logs 0 rust"
    echo "    (Đặc biệt tìm dòng 'Gap detected' hoặc 'Skipping commit')"
    exit 1
}

# Xác minh thành công
if echo "$TX_OUTPUT" | grep -q "✅ All transactions processed"; then
    echo "✅ Toàn bộ quá trình phục hồi và gửi giao dịch THÀNH CÔNG!"
    echo ""
    echo "$TX_OUTPUT" | grep -A 7 "Summary"
    echo ""
    echo "🎉 Hệ thống đã đồng thuận và xử lý chính xác sau khi khởi động lại."
else
    echo "❌ LỖI KHÔNG TÌM THẤY DẤU HIỆU THÀNH CÔNG!"
    echo "$TX_OUTPUT"
    exit 1
fi
