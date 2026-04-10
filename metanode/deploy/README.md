# Hướng Dẫn Vận Hành Môi Trường Production (Mainnet/Testnet)

Thư mục `deploy/` chứa toàn bộ công cụ để đưa MetaNode lên chạy thực tế trên máy chủ (VPS/Dedicated Server) đạt chuẩn Production. 

Khác với `mtn-orchestrator.sh` (chỉ dùng cho môi trường Dev/Test local thông qua Tmux), môi trường Production sử dụng **Systemd** để tự động quản lý luồng sống (lifecyle), tự động khởi động sau khi sập nguồn, xoay vòng log, và giới hạn mạng lưới với công cụ **Nginx** và **Prometheus/Grafana**.

---

## 1. Yêu cầu Hệ Thống
- **OS**: Ubuntu 22.04 / 24.04 LTS.
- **RAM**: Tối thiểu **16 GB RAM** (Go Runtime cần ~4GB + Rust NOMT Cache cần ~4.2GB).
- **Disk**: NVMe/SSD tĩnh phục vụ cho LevelDB/PebbleDB tốc độ cao.
- **Tools**: Giới hạn File Descriptor tối thiểu `100000` (đã được bọc tự động qua cấu hình `.service`).

## 2. Triển Khai Kiến Trúc Systemd

Công cụ `install-services.sh` sẽ cài đặt Systemd, nhận diện đường dẫn linh hoạt và tạo giới hạn cơ chế cấp phát. 

**Bước 1**: Chạy đoạn Script cài đặt dưới quyền `sudo`
```bash
# Cú pháp: sudo ./install-services.sh [số node] [giới hạn ram go]
sudo ./install-services.sh 5 4GiB
```
*Script sẽ kiểm tra dung lượng hệ thống, gán tự động đường dẫn `RUST_WORKDIR`, `GO_WORKDIR` hiện tại vào template, sau đó sao chép vào `/etc/systemd/system/` và kích hoạt.*

**Bước 2**: Khởi động Node (Tuân thủ thứ tự: Master -> Sub -> Rust)
```bash
sudo systemctl start metanode-go-master@0
sleep 2
sudo systemctl start metanode-go-sub@0
sleep 1
sudo systemctl start metanode-rust@0
```

**Bước 3**: Xem log hệ thống
```bash
# Xem log trực tiếp của Node 0 master
journalctl -u metanode-go-master@0 -f
```

---

## 3. Quản Lý Dung Lượng Log (Bắt buộc)
Các file log của Service sẽ được ghi liên tục ra thư mục `logs/node_{i}/`. Việc ghi đè lâu dài sẽ gây **Đầy Ổ Cứng**.
Khi bạn lệnh `install-services.sh` bên trên, tập tin cấu hình logrotate (`config-templates/metanode-logrotate.conf`) đã được chép tự động vào `/etc/logrotate.d/metanode`.

Hệ điều hành sẽ cắt log 100MB xoay vòng 7 ngày và nén Zip lại để chống quá tải dung lượng.

---

## 4. Setup Bảo vệ mạng Reverse Proxy (Chống DDOS Node)

Tài liệu cấu hình Nginx mẫu nằm tại: `nginx/mtn-rpc.conf`

**Việc cần làm:**
1. Sửa URL và Domain bên trong tệp tin `nginx/mtn-rpc.conf`. Đảm bảo port đang map đúng với config JSON RPC của Go.
2. Sao chép cài đặt vào hạ tầng Web:
```bash
sudo apt install nginx -y
sudo cp nginx/mtn-rpc.conf /etc/nginx/sites-available/mtn-rpc.conf
sudo ln -s /etc/nginx/sites-available/mtn-rpc.conf /etc/nginx/sites-enabled/
sudo systemctl restart nginx
```
Config này giúp Sub-node tránh sập khi bị Bot spam call hàng ngàn lệnh `eth_call` vô cực vào WebSocket hoặc HTTP.

---

## 5. Bảng Điều Khiển Giám Sát (Metrics)

Thao tác vận hành bắt buộc phải có Dashboard dể thấy ngay lúc CPU quá tải, rớt TPS hoặc OOM Ram.
Gói công cụ Metrics (Grafana + Prometheus) nằm ở: `monitoring/docker-compose.yml`.

Khởi chạy hệ thống theo dõi:
```bash
# Yêu cầu cài sẵn docker và docker-compose
cd monitoring
docker-compose up -d
```
Trạm kiểm soát sẽ tự động scrape dữ liệu từ mọi node qua port `9184+` của Rust và `9091+` của Go. 
**Truy cập vào giao diện web:**: `http://<IP Server>:3000` với user: `admin` mật khẩu `admin`.
