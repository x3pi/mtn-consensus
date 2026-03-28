#!/bin/bash
# No set -e to allow orchestrator to return non-zero code if it fails a minor check within its commands

echo "=========================================================="
echo "🚀 BẮT ĐẦU TEST KHỞI ĐỘNG LẠI CLUSTER 10 LẦN"
echo "=========================================================="

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
ORCHESTRATOR="$SCRIPT_DIR/mtn-orchestrator.sh"
TX_SENDER_DIR="$BASE_DIR/mtn-simple-2025/cmd/tool/tx_sender"

if [ ! -f "$ORCHESTRATOR" ]; then
    echo "❌ Không tìm thấy mtn-orchestrator.sh"
    exit 1
fi

TOTAL_RUNS=10

for i in $(seq 1 $TOTAL_RUNS); do
    echo ""
    echo "=========================================================="
    echo "🔄 LẦN TEST THỨ $i / $TOTAL_RUNS"
    echo "=========================================================="
    
    # 1. Dừng cluster
    echo "🛑 Đang dừng cluster..."
    "$ORCHESTRATOR" stop > /dev/null 2>&1
    
    # 2. Chờ một chút đảm bảo mọi thứ đã tắt hẳn
    sleep 3
    
    # 3. Khởi động lại cluster (KHÔNG dùng --fresh để test data persistence)
    echo "🚀 Đang khởi động cluster..."
    "$ORCHESTRATOR" start > /dev/null 2>&1
    
    # 4. Chờ cluster ổn định
    echo "⏳ Đang chờ 30s để cluster ổn định và khôi phục trạng thái..."
    sleep 30
    
    # 5. Kiểm tra trạng thái sơ bộ
    UP_NODES=$("$ORCHESTRATOR" status | grep "✅ Tất cả 15/15" | wc -l)
    if [ "$UP_NODES" -eq 0 ]; then
        echo "⚠️ Cảnh báo: Cluster có vẻ chưa lên đủ 15/15 processes. Vẫn tiếp tục thử gửi giao dịch..."
        "$ORCHESTRATOR" status
    fi
    
    # 6. Gửi giao dịch test
    echo "💸 Đang gửi giao dịch (deploy, store, read)..."
    cd "$TX_SENDER_DIR"
    
    # Chạy tx_sender và lưu output
    TX_OUTPUT=$(GOTOOLCHAIN=go1.23.5 go run . 2>&1) || {
        echo "❌ LỖI VÀO LẦN TEST THỨ $i!"
        echo "Chi tiết lỗi giao dịch:"
        echo "$TX_OUTPUT"
        echo "Dừng test tự động."
        exit 1
    }
    
    # Kiểm tra xem có text báo thành công không
    if echo "$TX_OUTPUT" | grep -q "✅ All transactions processed"; then
        echo "✅ Giao dịch thành công!"
        # In tóm tắt
        echo "$TX_OUTPUT" | grep -A 7 "Summary"
    else
        echo "❌ KHÔNG TÌM THẤY DẤU HIỆU THÀNH CÔNG!"
        echo "$TX_OUTPUT"
        exit 1
    fi
    
    echo "🎉 Test lần $i hoàn tất thành công."
    # Chờ 5s trước vòng lặp tiếp theo để block sync
    sleep 5
done

echo ""
echo "=========================================================="
echo "🏆 HOÀN THÀNH XUẤT SẮC 10/10 LẦN KHỞI ĐỘNG CÓ GIAO DỊCH"
echo "=========================================================="
