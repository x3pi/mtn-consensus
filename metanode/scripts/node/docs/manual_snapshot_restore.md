# Hướng Dẫn Phục Hồi Node Bằng Snapshot Thủ Công (Dành Cấp Độ Production)

Tài liệu này hướng dẫn chi tiết các bước tải và đắp Snapshot từ một Node khỏe mạnh (Ví dụ: Node 0) sang một Node trống (Ví dụ: Node 2) hoàn toàn thao tác tay trên Terminal.

Kiến trúc này đảm bảo tính an toàn cho dữ liệu, không làm treo server do tải nặng lúc copy, và ép Rust phải đồng bộ lại từ Dữ Liệu Go - đây là cách hoạt động ở môi trường Production thực tế.

---

## Bước 1: Gọi API Xem Snapshot Có Sẵn

Kiểm tra xem Node 0 đã tạo ra bản Snapshot nào chưa. Bật Terminal và chạy thử cURL:

```bash
curl -s http://127.0.0.1:8700/api/snapshots | jq .
```
> **Lưu ý:** Bạn sẽ thấy danh sách trả về chứa các URL, ví dụ: 
> `"download_url":"http://127.0.0.1:8700/download/snap_epoch_X_block_Y"`

---

## Bước 2: Tắt Tiến Trình Node Đích (Node 2)

Bạn phải tắt Node 2 trước khi xóa hoặc ghi đè dữ liệu. Việc này giải phóng khóa `LOCK` của PebbleDB.

```bash
tmux kill-session -t go-master-2 2>/dev/null
tmux kill-session -t go-sub-2 2>/dev/null
tmux kill-session -t metanode-2 2>/dev/null
```

---

## Bước 3: Dọn Sạch Dữ Liệu Cũ Của Node 2 (Clean State)

Đứng ở thư mục gốc của project (có chứa `mtn-consensus` và `mtn-simple-2025`):

```bash
# 1. Dọn Dữ Liệu Go
GO_NODE2_DIR="./mtn-simple-2025/cmd/simple_chain/sample/node2"
rm -rf "$GO_NODE2_DIR/data/data/"*
rm -rf "$GO_NODE2_DIR/data-write/"*
rm -rf "$GO_NODE2_DIR/back_up/"*
rm -rf "$GO_NODE2_DIR/back_up_write/"*

# 2. Dọn Tracking Của Rust (Ép nó phải lấy State thực lại từ Go)
RUST_NODE2_DIR="./mtn-consensus/metanode/config/storage/node_2"
rm -rf "$RUST_NODE2_DIR/epochs"
rm -f "$RUST_NODE2_DIR/last_block_number.bin"
rm -f "$RUST_NODE2_DIR/last_index.json"
```

---

## Bước 4: Tải Snapshot HTTP Bằng `wget`

Dùng kịch bản sau để tự động lấy URL mới nhất và tải toàn bộ file về một thư mục tạm của hệ thống (`/tmp/snap_node2_dl`):

```bash
# Tạo thư mục tải
mkdir -p /tmp/snap_node2_dl
cd /tmp/snap_node2_dl

# Tự động Get URL mới nhất
LATEST_SNAP=$(curl -s http://127.0.0.1:8700/api/snapshots | grep -o '"download_url":"[^"]*' | grep -o '[^"]*$' | tail -1)
echo "Đang tải Snapshot từ đường dẫn: $LATEST_SNAP"

# Tải xuống đa luồng, đệ quy thư mục
wget -q -c -r -np -nH --cut-dirs=2 -P . --reject="index.html*" "$LATEST_SNAP"
echo "Tải hoàn tất!"
```

---

## Bước 5: Đắp Dữ Liệu Snapshot Vào Node 2

Sau khi tải xong ở bước 4, Copy toàn bộ các file (`.log`, `.sst`, `MANIFEST`) vào khoang dữ liệu của Node 2:

```bash
GO_NODE2_DIR="/home/abc/chain-n/mtn-simple-2025/cmd/simple_chain/sample/node2"

# Bơm vào Go Master
cp -r /tmp/snap_node2_dl/* "$GO_NODE2_DIR/data/data/"
# Bơm vào Go Sub
cp -r /tmp/snap_node2_dl/* "$GO_NODE2_DIR/data-write/"
# Bơm dự phòng
cp -r /tmp/snap_node2_dl/* "$GO_NODE2_DIR/back_up/"
cp -r /tmp/snap_node2_dl/* "$GO_NODE2_DIR/back_up_write/"

# Dọn rác
rm -rf /tmp/snap_node2_dl
```

---

## Bước 6: Khởi Động Lại Node 2

Khi Dữ Liệu Go đã sẵn sàng, chúng ta kích hoạt lại Node 2. Do ở Bước 3, dữ liệu tracking của Rust (`last_block_number.bin`) đã bị xóa, Rust sẽ đọc lại State từ Go-Master và tiếp tục tham gia mạng lưới.

```bash
# Chuyển vào thư mục chứa script
cd /home/abc/chain-n/mtn-consensus/metanode/scripts/node

# Chạy khôi phục Node 2 với tùy chọn Resume (giữ Data hiện tại)
./resume_node.sh 2
```

---

### Xác Nhận Thành Công

Sử dụng `tmux` để vào xem Log của từng màn hình, bạn sẽ thấy tiến trình khôi phục:
```bash
tmux attach -t metanode-2
# Nhấn Ctrl+B rồi D để thoát
```
