#!/bin/bash

# Danh sách thư mục cần nén
folders=("types" "pkg")

# Danh sách file riêng lẻ trong thư mục gốc cần nén
files=("README.md" "TODO.md" "go.mod" "go.sum" "config1.json" "config2.json" "config3.json" "config4.json"  "config5.json" "kill.sh" "main.go" "zip.sh" "run.sh")

# Gộp hai danh sách lại thành một
targets=("${folders[@]}" "${files[@]}")

# Lấy ngày hiện tại
today=$(date +"%Y-%m-%d")

# Tên file nén
output_file="mtn-consensus-${today}.tar.gz"

# Tạo file nén
if tar -czvf "$output_file" "${targets[@]}"; then
    echo "✅ Đã tạo file nén: $output_file"
    # Hiển thị dung lượng file đã nén
    file_size=$(du -h "$output_file" | cut -f1)
    echo "�� Dung lượng file: $file_size"
else
    echo "❌ Lỗi khi tạo file nén!" >&2
    exit 1
fi
