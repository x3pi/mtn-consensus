# Hướng Dẫn Bảo Mật — MetaNode Consensus (Tầng Rust)

Tài liệu này mô tả cấu hình bảo mật và các biện pháp gia cố cho tầng consensus
Rust (`mtn-consensus`).

## Mục Lục

1. [Bảo Mật Mạng](#bảo-mật-mạng)
2. [Bảo Mật IPC / UDS](#bảo-mật-ipc--uds)
3. [An Toàn Fork](#an-toàn-fork)
4. [An Toàn Bộ Nhớ](#an-toàn-bộ-nhớ)
5. [Chống Tấn Công Consensus](#chống-tấn-công-consensus)

---

## Bảo Mật Mạng

### Cổng và Giao Thức

| Cổng | Giao Thức | Mục Đích | Xác Thực |
|---|---|---|---|
| gRPC (consensus) | TCP + TLS | Consensus Mysticeti DAG | TLS qua tonic |
| `peer_rpc_port` | TCP | RPC ngang hàng tùy chỉnh (chuyển tiếp TX, khám phá peer) | Không |
| `rust_tx_socket_path` | UDS | Nhận giao dịch từ Go | Quyền filesystem |

### RPC Ngang Hàng

Server peer RPC tùy chỉnh (`network/peer_rpc/server.rs`) sử dụng TCP thuần không có TLS.
Điều này chấp nhận được khi các node chạy trên **mạng riêng** hoặc **VPN**.

**Khuyến nghị**: Sử dụng WireGuard hoặc iptables để giới hạn cổng peer RPC chỉ cho
các validator đã biết.

---

## Bảo Mật IPC / UDS

### Quyền Socket

| Socket | Chế Độ | Thiết Lập Bởi |
|---|---|---|
| UDS Transaction (`metanode-tx-{N}.sock`) | `0o660` | `tx_socket_server.rs:94` |
| UDS Notification (`metanode-notification-{N}.sock`) | `0o660` | `notification_server.rs:47` |
| Executor Send (`executor{N}.sock`) | Mặc định OS | Go tạo |
| Executor Receive (`rust-go.sock_1`) | Mặc định OS | Go tạo |

**Đảm bảo** tiến trình Go và Rust chạy dưới **cùng user hoặc group** để giao tiếp UDS
hoạt động với quyền `0o660`.

---

## An Toàn Fork

Tầng Rust triển khai các cơ chế phòng chống fork nghiêm ngặt:

### Dừng Khẩn Cấp (process::exit thay vì panic)

Các điều kiện sau gây **dừng tiến trình ngay lập tức** qua `process::exit(1)` để
ngăn phân kỳ trạng thái:

| File | Điều Kiện | Thông Báo |
|---|---|---|
| `commit_processor.rs` | Thiếu dữ liệu committee cho epoch | `[HALTING] No committee data` |
| `commit_processor.rs` | Chỉ số leader vượt quá phạm vi | `[HALTING] Leader index out of range` |
| `commit_processor.rs` | Độ dài địa chỉ ETH không hợp lệ | `[HALTING] Invalid ETH address` |
| `metrics.rs` | Lỗi khởi tạo Prometheus metrics | `[HALTING] Metrics init failed` |

### Nguyên Tắc Thiết Kế

1. **Thời gian từ consensus**: Tất cả timestamp block lấy từ consensus Rust DAG
2. **Chọn leader xác định**: Leader được tính từ dữ liệu committee + số round
3. **Phối hợp ranh giới epoch**: Go thông báo cho Rust về chuyển đổi epoch qua UDS RPC
4. **Xếp hàng giao dịch**: TX được xếp hàng (không bị loại bỏ) trong quá trình chuyển đổi epoch

---

## An Toàn Bộ Nhớ

### Code Tùy Chỉnh

- **0 khối `unsafe`** trong `metanode/src/` (code tùy chỉnh)
- Tất cả `.unwrap()` đã thay bằng `.expect()` với mô tả chi tiết
- Tất cả `assert!()` không có mô tả đã được gia cố thêm thông báo

### Code Upstream

- `meta-consensus/core/` chứa ~50 TODO (upstream, không sửa đổi)
- `tower-0.5.2` có cảnh báo lint đã biết (tồn tại trước, không liên quan bảo mật)

---

## Chống Tấn Công Consensus

| Vector Tấn Công | Biện Pháp Giảm Thiểu |
|---|---|
| **Thao túng leader** | Leader tính xác định từ committee + round |
| **Lạm dụng epoch** | Chuyển đổi epoch yêu cầu thông báo Go executor + cập nhật committee |
| **Trùng lặp TX** | TxRecycler theo dõi TX đã gửi + committed_transaction_hashes khử trùng |
| **Giả mạo DAG** | Xử lý bởi giao thức consensus Mysticeti core |
| **Phát lại block** | Số block tăng đơn điệu, được Go executor xác minh |
| **Tràn mempool** | Backpressure tại 8M TX pool; xếp hàng lock-free khi chuyển đổi epoch |
