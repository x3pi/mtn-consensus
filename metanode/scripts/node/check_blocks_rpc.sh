#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  KIỂM TRA BLOCK HEIGHT QUA RPC
# ═══════════════════════════════════════════════════════════════

echo -e "\033[0;34m🔍 Đang gọi RPC API lấy mốc Block hiện tại của toàn bộ 5 Node...\033[0m"
echo "----------------------------------------------------"

# Mảng chứa các port RPC của từng Node Sub (theo file config-sub-node*.json)
PORTS=(8646 10646 10650 10651 10649)

# Lặp qua tất cả 5 node
for i in {0..4}; do
    PORT=${PORTS[$i]}
    
    # Gọi RPC getBlockNumber
    # Payload chuẩn của Ethereum/MetaNode RPC
    RESPONSE=$(curl -s -X POST -H "Content-Type: application/json" \
        --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        http://127.0.0.1:$PORT)
    
    if [ -z "$RESPONSE" ]; then
         echo -e "\033[0;31mNode $i (Port $PORT): ❌ Không thể kết nối (Offline)\033[0m"
         continue
    fi
    
    # Dùng jq để parse kết quả hex JSON (thường trả về dạng "0x2a")
    HEX_BLOCK=$(echo $RESPONSE | jq -r '.result' 2>/dev/null)
    
    if [[ "$HEX_BLOCK" == "null" || -z "$HEX_BLOCK" ]]; then
         echo -e "\033[1;33mNode $i (Port $PORT): ⚠️ RPC chưa phản hồi số liệu block hợp lệ\033[0m"
    else
         # Đổi mã hex sang số thập phân (Decimal)
         DEC_BLOCK=$((16#${HEX_BLOCK#0x}))
         echo -e "\033[0;32mNode $i (Port $PORT): ✅ Block $DEC_BLOCK\033[0m"
    fi
done

echo "----------------------------------------------------"
echo -e "\033[0;34m✅ Hoàn tất kiểm tra qua mạng Local RPC!\033[0m"
