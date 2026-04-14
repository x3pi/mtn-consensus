#!/bin/bash
# ==========================================================
# Script tự động cài đặt môi trường cho NODE 2
# Chạy trên máy ảo/máy chủ có IP: 127.0.0.1
# ==========================================================
set -e

echo -e "\e[1;36m[1/3] Cài đặt đồng bộ thời gian (Chrony)...\e[0m"
if ! command -v chronyd &> /dev/null; then
    sudo apt update && sudo apt -y install chrony
else
    echo -e "  ✅ Chrony đã được cài đặt."
fi
sudo systemctl enable --now chrony

echo -e "\n\e[1;36m[2/3] Cấu hình Firewall (UFW)...\e[0m"
# Rust Consensus P2P
sudo ufw allow 9002/tcp
# Peer Discovery Go Master
sudo ufw allow 19002/tcp
# Go User RPC
sudo ufw allow 10749/tcp
# Go Internal P2P (Primary, Worker)
sudo ufw allow 4200/tcp
sudo ufw allow 4212/tcp
sudo ufw allow 9002/tcp

echo -e "\n\e[1;32m✅ Setup hệ thống hoàn tất cho Máy Node 2.\e[0m"
echo -e "\e[1;33mTiếp theo:\e[0m Bạn hãy copy file binary và config sang máy này rồi chạy."
