#!/bin/bash

# Tên của session tmux để dễ quản lý
SESSION_NAME="rbc_session_5_nodes"

# --- Bắt đầu Script ---

# 1. Dọn dẹp session cũ
tmux kill-session -t $SESSION_NAME 2>/dev/null || true
echo "Đã dọn dẹp session tmux cũ (nếu có)."

# 2. Tạo session tmux mới
tmux new-session -d -s $SESSION_NAME
echo "Đã tạo session tmux mới tên là '$SESSION_NAME'."

# 3. Chia màn hình thành 5 ô và tự động sắp xếp
tmux split-window -v # Pane 0 (trên), 1 (dưới)
tmux split-window -h -t $SESSION_NAME:0.0 # Pane 0 chia đôi -> 0 (trái), 2 (phải)
tmux split-window -h -t $SESSION_NAME:0.1 # Pane 1 chia đôi -> 1 (trái), 3 (phải)
tmux split-window -v -t $SESSION_NAME:0.3 # Chia pane 3 -> 3 (trên), 4 (dưới)
tmux select-layout tiled # Lệnh quan trọng: Tự động sắp xếp lại tất cả các ô

# 4. Gửi lệnh 'go run' tới từng ô
echo "Đang khởi chạy 5 node..."

tmux send-keys -t $SESSION_NAME:0.0 "go run . --config='config1.json'" C-m
tmux send-keys -t $SESSION_NAME:0.1 "go run . --config='config2.json'" C-m
tmux send-keys -t $SESSION_NAME:0.2 "go run . --config='config3.json'" C-m
tmux send-keys -t $SESSION_NAME:0.3 "go run . --config='config4.json'" C-m
tmux send-keys -t $SESSION_NAME:0.4 "go run . --config='config5.json'" C-m

# 5. Gắn vào session tmux
echo "Gắn vào session tmux..."
sleep 1
tmux attach-session -t $SESSION_NAME