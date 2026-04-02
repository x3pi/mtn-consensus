#!/bin/bash
# ==========================================================
# Script tự động cài đặt môi trường cho NODE 3
# Chạy trên máy ảo/máy chủ có IP: 192.168.1.231
# ==========================================================
set -e

echo -e "\e[1;36m[1/3] Cài đặt đồng bộ thời gian (Chrony)...\e[0m"
sudo apt update && sudo apt install chrony -y
sudo systemctl enable --now chrony

echo -e "\n\e[1;36m[2/3] Cấu hình Firewall (UFW)...\e[0m"
# Rust Consensus P2P
sudo ufw allow 9003/tcp
# Peer Discovery Go Master
sudo ufw allow 19003/tcp
# Go User RPC
sudo ufw allow 10750/tcp
# Go Internal P2P (Primary, Worker)
sudo ufw allow 4300/tcp
sudo ufw allow 4312/tcp
sudo ufw allow 9003/tcp

echo -e "\n\e[1;32m✅ Setup hệ thống hoàn tất cho Máy Node 3.\e[0m"
echo -e "\e[1;33mTiếp theo:\e[0m Bạn hãy copy file binary và config sang máy này rồi chạy."
