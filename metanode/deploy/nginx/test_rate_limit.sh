#!/bin/bash
# test_rate_limit.sh
# Tests Nginx rate-limiting (port 8545) on MetaNode JSON-RPC.

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

NGINX_URL="http://127.0.0.1:8545"
PAYLOAD='{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

echo -e "${YELLOW}🔍 Kiểm tra Nginx có đang chạy ở port 8545 không...${NC}"
HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" -d "$PAYLOAD" "$NGINX_URL" || echo "FAIL")

if [ "$HTTP_STATUS" == "FAIL" ] || [ "$HTTP_STATUS" == "000" ]; then
    echo -e "${RED}❌ Không thể kết nối tới Nginx ở $NGINX_URL. Hãy chắc chắn bạn đã chạy 'sudo bash install-nginx.sh' thành công!${NC}"
    exit 1
fi
echo -e "${GREEN}✅ Nginx đang phản hồi. Bắt đầu test Rate-Limit (Burst 50 requests)...${NC}"

# Tạo 50 requests gần như đồng thời để kích hoạt Rate Limit Zone (20r/s)
SUCCESS=0
RATE_LIMITED=0

for i in {1..50}; do
    STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" -d "$PAYLOAD" "$NGINX_URL" &)
    # wait a tiny bit to not overwhelm bash, but fast enough to trigger Nginx limit
    if [ "$STATUS" == "200" ]; then
        ((SUCCESS++))
    elif [ "$STATUS" == "503" ]; then
        ((RATE_LIMITED++))
    fi
done

wait
echo -e "\n${YELLOW}📊 Kết quả sau 50 requests:${NC}"
echo -e "   - Thành công (200 OK): ${GREEN}$SUCCESS${NC}"
echo -e "   - Bị chặn bởi Rate Limit (503): ${RED}$RATE_LIMITED${NC}"

if [ "$RATE_LIMITED" -gt 0 ]; then
    echo -e "${GREEN}🚀 TUYỆT VỜI! Rate Limit đã hoạt động chính xác để chống Spam DDoS.${NC}"
else
    echo -e "${YELLOW}⚠️ Nginx chưa chặn được request, có thể Rate burst hiện tại (50) ở metanode.conf chưa bị vượt qua. Bạn có thể test mạnh hơn.${NC}"
fi
