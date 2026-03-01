# 🚀 Hướng Dẫn Vận Hành Hệ Thống mtn-consensus

Hướng dẫn từng bước: Build → Cấu hình IP → Chạy → Dừng → Resume.

---

## 📋 Yêu Cầu

> [!IMPORTANT]
> **Cấu trúc thư mục bắt buộc:** Hai project `mtn-simple-2025` và `mtn-consensus` **phải nằm cùng một thư mục gốc**. Các script sử dụng đường dẫn tương đối `../../mtn-simple-2025` để tìm Go project.
>
> ```
> chain-n/                    ← Thư mục gốc (tên tùy ý)
> ├── mtn-consensus/          ← Rust Consensus Engine
> │   └── metanode/
> │       └── scripts/node/   ← Các script vận hành
> └── mtn-simple-2025/        ← Go Execution Engine (EVM)
>     └── cmd/simple_chain/   ← Config + binary Go
> ```

| Yêu cầu | Phiên bản |
|----------|-----------|
| Rust | nightly (với `cargo +nightly`) |
| Go | 1.23+ (`GOTOOLCHAIN=go1.23.5`) |
| protoc | protoc3 (PATH: `/home/abc/protoc3/bin`) |
| tmux | bất kỳ |

---

## 1. Build Binary

> [!TIP]
> **Không cần build thủ công!** Script `run_all.sh` và `resume_all.sh` sẽ **tự động build** cả Rust và Go binary trước khi chạy. Chỉ cần build thủ công khi muốn kiểm tra lỗi compile hoặc build cross-platform.

### 1.1 Build Rust Consensus Engine (thủ công)

```bash
cd mtn-consensus/metanode
export PATH="/home/abc/protoc3/bin:$PATH"
cargo +nightly build --release --bin metanode
```

Binary output: `target/release/metanode`

### 1.2 Build Go Execution Engine — EVM

```bash
cd mtn-simple-2025

# Linux
./build.sh linux

# macOS
./build.sh mac
```

---

## 2. Cấu Hình IP

> [!NOTE]
> **Cấu hình nhanh cho testing.** Script `update_ips.sh` chỉ thay đổi **IP** — giữ nguyên toàn bộ keys, đường dẫn, ports và các cấu hình khác. Mục đích để nhanh chóng chạy hệ thống trên các IP khác nhau.

### 2.1 Cập nhật IP cho tất cả configs

```bash
cd mtn-consensus/metanode/scripts/node

# Preview trước (không ghi file)
./update_ips.sh --dry-run NODE0_IP NODE1_IP NODE2_IP NODE3_IP [NODE4_IP]

# Ghi đè thật
./update_ips.sh NODE0_IP NODE1_IP NODE2_IP NODE3_IP [NODE4_IP]
```

**Ví dụ:**

```bash
# 2 máy (Node 0 máy A, Node 1-4 máy B)
./update_ips.sh 192.168.1.231 192.168.1.232 192.168.1.232 192.168.1.232

# 5 máy khác nhau
./update_ips.sh 10.0.0.1 10.0.0.2 10.0.0.3 10.0.0.4 10.0.0.5

# Tất cả trên localhost
./update_ips.sh 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1
```

### 2.2 Files bị ảnh hưởng

| File | Fields được cập nhật |
|------|---------------------|
| `config/node_{0..4}.toml` | `network_address`, `peer_rpc_addresses` |
| `config-master-node{0..4}.json` | `meta_node_rpc_address` |
| `config-sub-node{0..4}.json` | `meta_node_rpc_address`, `master_address` |

### 2.3 Cấu hình Firewall (chỉ cần 1 lần)

```bash
cd mtn-consensus/metanode/scripts/node
sudo ./setup_firewall.sh
```

Ports cần mở giữa các máy:
- **9000–9004**: Consensus P2P
- **19000–19004**: Peer RPC Discovery
- **4200–4201, 6200–6241**: Go connection ports

---

## 3. Chạy Hệ Thống

### 3.0 Build trước khi chạy từng node

> [!IMPORTANT]
> `run_all.sh` tự động build cả Rust + Go. Nhưng `run_node.sh` và `resume_node.sh` **không tự build**. Nếu có thay đổi code, chạy lệnh build trước:

```bash
cd mtn-consensus/metanode/scripts/node
./build.sh           # Build cả Rust metanode + Go simple_chain
```

### 3.1 Fresh Start tất cả (tự động build + xóa data)

```bash
cd mtn-consensus/metanode/scripts/node
./run_all.sh
```

Script tự động:
1. Dừng tất cả process cũ
2. Build cả Rust và Go binary
3. Xóa data Go + Rust (giữ keys/config)
4. Khởi động Go Masters (5 nodes) → chờ socket ready
5. Khởi động Go Subs (5 nodes)
6. Khởi động Rust Metanodes (5 nodes)

### 3.2 Fresh Start 1 node (xóa data node đó)

```bash
./run_node.sh <node_id>

# Ví dụ:
./run_node.sh 0    # Reset và start Node 0
./run_node.sh 4    # Reset và start Node 4
```

### 3.3 Kiểm tra trạng thái

```bash
# Xem tất cả tmux sessions
tmux ls

# Sessions mong đợi cho mỗi node N:
#   go-master-N, go-sub-N, metanode-N
```

---

## 4. Dừng Hệ Thống

### 4.1 Dừng tất cả

```bash
./stop_all.sh
```

### 4.2 Dừng 1 node

```bash
./stop_node.sh <node_id>

# Ví dụ:
./stop_node.sh 2    # Dừng Node 2 (các node khác vẫn chạy)
```

Quy trình dừng:
1. Gửi `Ctrl+C` (SIGINT) → Go flush LevelDB, Rust flush state
2. Chờ 5s cho graceful shutdown
3. Kill tmux sessions
4. Xóa socket files

---

## 5. Resume (Giữ Data)

### 5.1 Resume tất cả

```bash
./resume_all.sh
```

Giống `run_all.sh` nhưng **không xóa data** — dùng khi restart sau khi tắt tạm.

### 5.2 Resume 1 node

```bash
./resume_node.sh <node_id>

# Ví dụ:
./resume_node.sh 1    # Resume Node 1 (giữ data)
```

> **Khác biệt `run` vs `resume`:**  
> - `run_node.sh` = xóa data + start (fresh)  
> - `resume_node.sh` = giữ data + start (tiếp tục)

---

## 6. Xem Log

### 6.1 Script tiện ích

```bash
cd mtn-consensus/metanode/scripts

# Rust log
./logs/rust.sh 0            # 50 dòng cuối Node 0
./logs/rust.sh 0 200        # 200 dòng cuối
./logs/rust.sh 0 -f         # Follow real-time

# Go Master log
./logs/go-master.sh 1       # Node 1
./logs/go-master.sh 1 -f    # Follow

# Go Sub log
./logs/go-sub.sh 2          # Node 2

# Follow tất cả log 1 node
./logs/follow.sh 0          # Rust + Go Master + Go Sub Node 0

# Tổng quan tất cả
./logs/view.sh              # Liệt kê tất cả log files
./logs/view.sh 0            # Tail tất cả log Node 0
```

### 6.2 Xem trực tiếp qua tmux

```bash
tmux attach -t metanode-0     # Rust Node 0
tmux attach -t go-master-0    # Go Master Node 0
tmux attach -t go-sub-0       # Go Sub Node 0
# Ctrl+B, D để detach (không kill)
```

### 6.3 Đường dẫn log files

```
mtn-consensus/metanode/logs/
├── node_0/
│   ├── rust.log
│   ├── go-master-stdout.log
│   └── go-sub-stdout.log
├── node_1/
│   └── ...
└── node_4/
    └── ...
```

---

## 7. Monitoring Dashboard

```bash
cd mtn-consensus/metanode/monitoring
./start_monitor.sh                    # Production mode
./start_monitor.sh --development      # Dev mode (auto-reload)
./start_monitor.sh --port 3000        # Port tùy chỉnh
```

Dashboard: http://localhost:8080/dashboard

---

## 8. Troubleshooting

### Rust node không khởi động
```bash
ls -la target/release/metanode                      # Kiểm tra binary
netstat -tuln | grep -E "9000|9001|9002|9003|9004"  # Kiểm tra ports
```

### Go node không khởi động
```bash
ls -la ../mtn-simple-2025/cmd/simple_chain/simple_chain  # Binary
ls -la /tmp/rust-go-node*-master.sock                     # Socket Master
```

### Socket lỗi
```bash
# Xóa socket cũ thủ công
rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock
```

### Kiểm tra consensus
```bash
# Xem block mới nhất
./scripts/logs/rust.sh 0 5 | grep "commit_index"

# Xem epoch hiện tại
./scripts/logs/rust.sh 0 5 | grep "epoch"

# Xem TPS thực tế
./scripts/analysis/calculate_real_tps.sh 9103 5
```

---

## 📌 Tóm Tắt Lệnh

| Hành động | Lệnh |
|-----------|-------|
| **Build cả 2** | `./build.sh` |
| **Build Rust** | `cargo +nightly build --release --bin metanode` |
| **Build Go** | `cd mtn-simple-2025 && ./build.sh linux` |
| **Sửa IP** | `./update_ips.sh IP0 IP1 IP2 IP3 [IP4]` |
| **Chạy tất cả (fresh)** | `./run_all.sh` |
| **Chạy 1 node (fresh)** | `./run_node.sh N` |
| **Dừng tất cả** | `./stop_all.sh` |
| **Dừng 1 node** | `./stop_node.sh N` |
| **Resume tất cả** | `./resume_all.sh` |
| **Resume 1 node** | `./resume_node.sh N` |
| **Xem log** | `./logs/rust.sh N -f` |
| **Kiểm tra** | `tmux ls` |

> Tất cả scripts nằm tại: `mtn-consensus/metanode/scripts/node/`
