#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  KIỂM TRA BLOCK HEIGHT CỦA TẤT CẢ CÁC NODE
# ═══════════════════════════════════════════════════════════════

echo -e "\033[0;34m🔍 Đang quét mốc Block hiện tại của toàn bộ 5 Node...\033[0m"
echo "----------------------------------------------------"

# Determine absolute path to the logs directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_BASE_DIR="$(cd "$SCRIPT_DIR/../../logs" && pwd)"

# Lặp qua tất cả 5 node
for i in {0..4}; do
    LOG_FILE="$LOG_BASE_DIR/node_$i/go-master-stdout.log"
    
    if [ ! -f "$LOG_FILE" ]; then
        echo -e "\033[0;31mNode $i: Không tìm thấy file log\033[0m"
        continue
    fi
    
    # Tìm dòng chứa last_committed_block mới nhất
    BLOCK_LINE=$(grep -a "last_committed_block=" "$LOG_FILE" | tail -n 1)
    
    if [ -z "$BLOCK_LINE" ]; then
        echo -e "\033[1;33mNode $i: Chưa có số liệu block\033[0m"
    else
        # Tách lấy con số đằng sau chữ last_committed_block=
        BLOCK_HEIGHT=$(echo "$BLOCK_LINE" | sed -n 's/.*last_committed_block=\([0-9]*\).*/\1/p')
        echo -e "\033[0;32mNode $i: Block $BLOCK_HEIGHT\033[0m"
    fi
done

echo "----------------------------------------------------"
echo -e "\033[0;34m✅ Hoàn tất kiểm tra!\033[0m"
