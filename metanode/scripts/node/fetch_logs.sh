#!/bin/bash
# ============================================================================
# Fetch Logs Script - Tự động kéo toàn bộ log từ cụm máy chủ về thư mục cục bộ
# Cách chạy: bash fetch_logs.sh [path/to/env_file]
# Ví dụ: bash fetch_logs.sh deploy-3machines.env
# ============================================================================

set -uo pipefail

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="${1:-$SCRIPT_DIR/deploy.env}"

# Hỗ trợ truyền tên file env tương tự như deploy_cluster.sh (VD: --env deploy-3machines.env)
if [ "$#" -ge 2 ] && [ "$1" == "--env" ]; then
    ENV_FILE="$2"
    if [[ "$ENV_FILE" != /* ]]; then
        ENV_FILE="$SCRIPT_DIR/$ENV_FILE"
    fi
elif [ "$#" -eq 1 ] && [[ "$1" != --* ]]; then
    ENV_FILE="$1"
    if [[ "$ENV_FILE" != /* ]]; then
        ENV_FILE="$SCRIPT_DIR/$ENV_FILE"
    fi
fi

if [ ! -f "$ENV_FILE" ]; then
    echo -e "${YELLOW}Không tìm thấy file config: $ENV_FILE${NC}"
    echo "Usage: ./fetch_logs.sh --env <env-file>"
    exit 1
fi

echo -e "${CYAN}📋 Using config: $ENV_FILE${NC}"
source "$ENV_FILE"

LOCAL_LOGS_DIR="${LOCAL_METANODE}/logs"
mkdir -p "$LOCAL_LOGS_DIR"

echo -e "${GREEN}📥 Bắt đầu đồng bộ log từ các máy chủ về: $LOCAL_LOGS_DIR${NC}"

# Hàm lấy danh sách server unique
get_unique_servers() {
    local servers=""
    for ip in "${NODE_SERVER[@]}"; do
        if [[ ! " $servers " =~ " $ip " ]] && [ -n "$ip" ]; then
            servers="$servers $ip"
        fi
    done
    echo "$servers"
}

# Hàm pull bằng rsync
rsync_pull_cmd() {
    local host="$1"
    local remote_path="$2"
    local local_path="$3"
    
    # -a: archive, -v: verbose, -z: compress, --update: only download newer files
    local rsync_args="-avz --update"
    
    if [ "${SSH_AUTH:-key}" == "password" ]; then
        sshpass -p "$SSH_PASSWORD" rsync $rsync_args -e "ssh $SSH_OPTS" "${SSH_USER}@${host}:${remote_path}" "${local_path}"
    elif [ -n "${SSH_KEY:-}" ]; then
        rsync $rsync_args -e "ssh $SSH_OPTS -i $SSH_KEY" "${SSH_USER}@${host}:${remote_path}" "${local_path}"
    else
        rsync $rsync_args -e "ssh $SSH_OPTS" "${SSH_USER}@${host}:${remote_path}" "${local_path}"
    fi
}

SERVERS=$(get_unique_servers)

for server in $SERVERS; do
    echo -e "\n${CYAN}🌐 Đang đồng bộ log từ server: $server...${NC}"
    # Đồng bộ toàn bộ nội dung trong thư mục logs của remote về thư mục logs local
    # Thêm dấu / ở cuối remote_path để copy nội dung TRONG thư mục logs chứ không copy thêm folder "logs" bị lồng
    rsync_pull_cmd "$server" "${REMOTE_METANODE}/logs/" "${LOCAL_LOGS_DIR}/"
    
    if [ $? -eq 0 ]; then
        echo -e "   ✅ Tải thành công từ $server"
    else
        echo -e "   ⚠️ Có lỗi khi tải log từ $server"
    fi
done

echo -e "\n${GREEN}🎉 Hoàn thành! Log của tất cả các node hiện có sẵn tại: ${LOCAL_LOGS_DIR}${NC}"
