#!/bin/bash

# Tên session tmux cần dừng, phải khớp với tên trong run_tmux.sh
SESSION_NAME="rbc_session"

echo "Đang cố gắng dừng session tmux '$SESSION_NAME'..."

# Lệnh `tmux kill-session` sẽ dừng session và tất cả các tiến trình bên trong nó.
# `2>/dev/null` sẽ ẩn thông báo lỗi nếu session không tồn tại.
# `|| true` đảm bảo script không thoát với mã lỗi nếu session không được tìm thấy.
tmux kill-session -t $SESSION_NAME 2>/dev/null

# Kiểm tra mã thoát của lệnh vừa rồi để cung cấp thông báo chính xác
if [ $? -eq 0 ]; then
    echo "✅ Session tmux '$SESSION_NAME' đã được dừng thành công."
else
    echo "ℹ️  Không tìm thấy session tmux '$SESSION_NAME' nào đang chạy."
fi

echo ""
echo "Kiểm tra và dọn dẹp các tiến trình Go 'reliable-broadcast' còn sót lại (nếu có)..."

# `pkill -f` sẽ tìm và dừng tất cả các tiến trình có tên hoặc tham số chứa "reliable-broadcast".
# Đây là một biện pháp an toàn để đảm bảo không còn tiến trình nào chạy ngầm.
pkill -f "reliable-broadcast"

echo "✅ Hoàn tất dọn dẹp!"