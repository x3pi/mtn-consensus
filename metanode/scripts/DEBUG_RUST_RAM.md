# 🦀 Hướng dẫn Debug RAM cho Rust Metanode

Tài liệu này hướng dẫn cách sử dụng hai bộ công cụ đã được thiết lập để theo dõi và tìm nguyên nhân gây tăng RAM (Memory Leak) trong các Metanode viết bằng Rust.

---

## 1. Công cụ: `debug_rust_ram.sh` (Theo dõi nhanh & Dashboard)
Đây là script tùy biến dành riêng cho Metanode, cung cấp cái nhìn tổng quan và biểu đồ xu hướng giống như Go `pprof`.

### Cách sử dụng
Chạy script tại thư mục `scripts`:
```bash
cd /home/abc/nhat/con-chain-v2/mtn-consensus/metanode/scripts
bash debug_rust_ram.sh [interval_giây] [số_lần_chụp] [cổng_web]
```
*   **Ví dụ:** Chụp 10 lần, mỗi lần cách nhau 60 giây, mở Dashboard tại cổng 8099:
    ```bash
    bash debug_rust_ram.sh 60 10 8099
    ```

### Cách xem Dashboard
Sau khi lệnh kết thúc (hoặc nhấn **Enter** để kết thúc sớm), một server Python sẽ tự động bật lên.
1.  Nếu bạn đang ở máy cục bộ: Mở trình duyệt truy cập `http://localhost:8099/report.html`.
2.  Dashboard sẽ hiển thị:
    *   **VmRSS:** RAM vật lý thực tế mà node đang chiếm.
    *   **Pss_Anon (Heap):** Vùng RAM mà Rust Allocator đang giữ (đây là nơi cần soi nếu nghi ngờ leak code Rust).
    *   **Top Mappings:** Các file thư viện hoặc vùng nhớ hệ thống đang ngốn RAM nhất.
    *   **Rust Logs:** Các sự kiện quan trọng (commit, warn, error) từ tmux session của từng node.

---

## 2. Công cụ: `Heaptrack` (Phân tích sâu đến tận dòng code)
Sử dụng khi bạn đã thấy RAM tăng ở bước 1 và muốn biết chính xác **hàm nào** hoặc **dòng code nào** gây ra.

### A. Gắn vào process đang chạy (Khuyên dùng)
Nếu Metanode đang chạy và bạn thấy RAM cao, hãy lấy PID của nó (ví dụ: `399908`) và chạy:
```bash
heaptrack -p 399908
```
*   Để nó chạy trong khoảng 5-10 phút để thu thập đủ dữ liệu.
*   Nhấn **Ctrl+C** để dừng. Nó sẽ sinh ra một file có đuôi `.zst` (ví dụ: `heaptrack.metanode.12345.zst`).

### B. Chạy metanode mới với heaptrack
```bash
heaptrack ./target/release/metanode start --config <path_to_config>
```

### C. Cách xem báo cáo Heaptrack
#### Cách 1: Xem nhanh dạng văn bản trên Server
```bash
heaptrack --analyze "heaptrack.metanode.xxxx.zst" | head -n 50
```
#### Cách 2: Xem giao diện đồ thị (Flamegraph) - Tốt nhất
1.  Tải file `.zst` đó về máy tính cá nhân.
2.  Mở bằng phần mềm **Heaptrack GUI** (Cài trên máy cá nhân: `sudo apt install heaptrack` trên Linux, hoặc tải bản cài cho Windows/Mac).
3.  Các tab quan trọng cần xem:
    *   **Flame Graph:** Chỉ rõ hàm nào trong Rust đang chiếm bao nhiêu RAM.
    *   **Top Consumers:** Danh sách các hàm tốn bộ nhớ nhất.
    *   **Leaked Allocations:** Các vùng nhớ chưa được giải phóng.

---

## 3. Các chỉ số RAM cần lưu ý (Dành cho Rust)

| Chỉ số | Ý nghĩa | Cần làm gì nếu tăng? |
| :--- | :--- | :--- |
| **VmRSS** | Tổng RAM thực tế đang dùng | Đây là con số htop hiển thị. |
| **Pss_Anon** | Bộ nhớ Heap/Stack thực sự | Nếu tăng liên tục => Chắc chắn Leak trong code Rust hoặc C++ (RocksDB/Xapian). |
| **Pss_File** | RAM dùng để cache file | Tăng là bình thường (Hệ điều hành tự quản lý). |
| **Threads** | Số lượng luồng | Nếu tăng liên tục => Leak Task/Thread (giống Goroutine leak). |

---

## 4. Mẹo tìm nguyên nhân nhanh
*   **Nếu `Pss_Anon` tăng:** Dùng Heaptrack soi ngay.
*   **Nếu `Threads` tăng:** Kiểm tra các phần code `tokio::spawn` hoặc tạo thread mà không có điểm dừng.
*   **Nếu RAM tăng đột ngột rồi đứng yên:** Có thể do cache của Database (RocksDB/PebbleDB) hoặc Allocator chưa trả RAM về cho OS (điều này bình thường trong Rust).
*   **So sánh Snapshot:** Dùng Dashboard (bước 1) để xem ΔRSS. Nếu Δ lớn hơn 50MB trong thời gian ngắn lúc đang test TPS => Cần điều tra kỹ.
