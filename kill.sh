#!/bin/bash

# Tên của session tmux để dễ quản lý
# Cần khớp với SESSION_NAME trong file run.sh tương ứng
SESSION_NAME="rbc_session_5_nodes"

# Dừng session tmux được chỉ định
tmux kill-session -t $SESSION_NAME 2>/dev/null || true
echo "Đã gửi lệnh dừng tới session tmux '$SESSION_NAME' (nếu nó đang tồn tại)."