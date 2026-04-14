#!/bin/bash

echo "Tăng giới hạn TCP buffer và queue lên 16MB để chịu tải lớn..."

# 1. Tăng giới hạn cứng của hệ điều hành (rmem_max và wmem_max) lên 16MB
sudo sysctl -w net.core.rmem_max=16777216
sudo sysctl -w net.core.wmem_max=16777216

# 2. Tăng tham số tuning riêng của TCP (min, default, max) lên 16MB
sudo sysctl -w net.ipv4.tcp_rmem="4096 87380 16777216"
sudo sysctl -w net.ipv4.tcp_wmem="4096 65536 16777216"

# 3. Tăng mảng chờ kết nối (backlog) ở cấp độ card mạng (phòng hờ tin nhắn đến dồn dập)
sudo sysctl -w net.core.netdev_max_backlog=10000

echo ""
echo "✅ Đã áp dụng thành công. Cấu hình hiện tại:"
sysctl net.core.rmem_max net.ipv4.tcp_rmem
