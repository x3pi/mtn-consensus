#!/bin/bash

# Tên của session tmux để dễ quản lý
SESSION_NAME="rbc_session"

# Cấu hình các node
PEERS="0:localhost:8000,1:localhost:8001,2:localhost:8002,3:localhost:8003"

# --- Bắt đầu Script ---

# 1. Dọn dẹp: Đảm bảo không có session nào trùng tên đang chạy
# Lệnh `|| true` để script không báo lỗi nếu session chưa tồn tại
tmux kill-session -t $SESSION_NAME 2>/dev/null || true
echo "Đã dọn dẹp session tmux cũ (nếu có)."

# 2. Tạo một session tmux mới ở chế độ nền (detached)
# Session có 1 cửa sổ, cửa sổ có 1 ô (pane 0)
tmux new-session -d -s $SESSION_NAME
echo "Đã tạo session tmux mới tên là '$SESSION_NAME'."

# 3. Chia màn hình thành một lưới 2x2
# Chia ô 0 theo chiều ngang, tạo ra ô 1 bên phải
tmux split-window -h -t $SESSION_NAME:0.0
# Chia ô 0 theo chiều dọc, tạo ra ô 2 bên dưới
tmux split-window -v -t $SESSION_NAME:0.0
# Chia ô 1 (bên phải trên) theo chiều dọc, tạo ra ô 3 bên dưới
tmux split-window -v -t $SESSION_NAME:0.1

# 4. Gửi lệnh 'go run' tới từng ô để khởi chạy các node
echo "Đang khởi chạy 4 node trong 4 ô..."

# Chạy Node 0 trong ô trên-trái (pane 0)
tmux send-keys -t $SESSION_NAME:0.0 "go run . --id=0 --peers='$PEERS'" C-m

# Chạy Node 1 trong ô trên-phải (pane 1)
tmux send-keys -t $SESSION_NAME:0.1 "go run . --id=1 --peers='$PEERS'" C-m

# Chạy Node 2 trong ô dưới-trái (pane 2)
tmux send-keys -t $SESSION_NAME:0.2 "go run . --id=2 --peers='$PEERS'" C-m

# Chạy Node 3 trong ô dưới-phải (pane 3)
tmux send-keys -t $SESSION_NAME:0.3 "go run . --id=3 --peers='$PEERS'" C-m

# 5. Gắn vào session tmux để xem và tương tác
echo "Gắn vào session tmux. Màn hình của bạn sẽ được chia thành 4 phần."
sleep 1
tmux attach-session -t $SESSION_NAME