# Reliable Broadcast - Hệ thống Đồng thuận Blockchain

## Tổng quan

Dự án này triển khai một hệ thống blockchain với kiến trúc tách biệt:
- **Simple-chain**: Phần thực thi (execution layer)
- **MTN-consensus**: Phần đồng thuận (consensus layer)

> **Lưu ý quan trọng**: Hiện tại hệ thống chưa hỗ trợ đồng bộ hóa cho các node đồng thuận mới tham gia. Để chạy thử nghiệm, bạn cần xóa toàn bộ dữ liệu và khởi động tất cả các node cùng lúc.

## Yêu cầu hệ thống

### Cài đặt phụ thuộc

Để chạy hệ thống bằng các script bash có sẵn, bạn cần cài đặt:

```bash
# Ubuntu/Debian
sudo apt-get install tmux

# CentOS/RHEL
sudo yum install tmux

# macOS
brew install tmux
```

## Hướng dẫn chạy hệ thống

### Bước 1: Khởi động Simple-chain (Execution Layer)

Chạy lệnh sau để khởi động phần thực thi:

```bash
./cmd/simple_chain/run_nodes.sh
```

### Bước 2: Khởi động MTN-consensus (Consensus Layer)

**Chờ đợi**: Đợi cho đến khi simple-chain khởi động thành công và xuất hiện dòng log:


Sau đó chạy lệnh để khởi động các node đồng thuận:

```bash
./run.sh
```

### Bước 3: Kiểm tra hoạt động

Sau khi mtn-consensus khởi động, bạn sẽ thấy:
- Block bắt đầu được tạo
- Logs xuất hiện liên tục trên màn hình

Khi hệ thống hoạt động ổn định, bạn có thể tiến hành test giao dịch.

## Test giao dịch

Để thử nghiệm giao dịch, chạy script test sau:

```bash
./cmd/client/call_tool_example/run_1000_times.sh
```