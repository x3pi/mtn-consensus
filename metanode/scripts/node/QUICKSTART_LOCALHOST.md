# 🚀 Quickstart: Chạy Localhost + Test TPS

Hướng dẫn nhanh chạy toàn bộ hệ thống trên **1 máy duy nhất (localhost)** và test hiệu năng TPS.

---

## Bước 1 — Clone và cấu trúc thư mục

Đặt 2 project cùng một thư mục gốc:

```
chain-n/
├── mtn-consensus/        # Rust Consensus Engine
└── mtn-simple-2025/      # Go Execution Engine (EVM)
```

## Bước 2 — Build EVM

```bash
cd mtn-simple-2025
./build.sh linux          # hoặc: ./build.sh mac
```

## Bước 3 — Cấu hình IP localhost

```bash
cd mtn-consensus/metanode/scripts/node

# Set tất cả nodes về 127.0.0.1
./update_ips.sh 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1
```

## Bước 4 — Chạy tất cả nodes

```bash
# Fresh start: tự động build Rust + Go, xóa data cũ, chạy 5 nodes
./run_all.sh
```

Chờ script hoàn tất (~1-2 phút). Kiểm tra:

```bash
tmux ls
# Mong đợi 15 sessions: go-master-{0..4}, go-sub-{0..4}, metanode-{0..4}
```

## Bước 5 — Test TPS

```bash
cd mtn-simple-2025/cmd/tool/tps_blast

# Test nhẹ: 5 clients × 1000 TX = 5,000 TX
./run_multinode_load.sh 5 1000

# Test trung bình: 10 clients × 10,000 TX = 100,000 TX
./run_multinode_load.sh 10 10000

# Test nặng: 20 clients × 50,000 TX = 1,000,000 TX
./run_multinode_load.sh 20 50000
```

Kết quả mẫu:

```
╔═══════════════════════════════════════════════════════════╗
║                    📊 TỔNG KẾT                           ║
╠═══════════════════════════════════════════════════════════╣
  🏆 SYSTEM TPS:  ~10000 tx/s   ✅ VƯỢT MỤC TIÊU 10K!
  📦 Tổng TX gửi:        100000
  📥 TX trong blocks:     100000
  ✅ Success Rate:         100.0%
  🛡️  HỆ THỐNG KHÔNG FORK — AN TOÀN 100%
```

## Bước 6 — Dừng hệ thống

```bash
cd mtn-consensus/metanode/scripts/node
./stop_all.sh
```

---

## 📌 Tóm Tắt

```bash
# 1. Build EVM
cd mtn-simple-2025 && ./build.sh linux

# 2. Set IP localhost
cd mtn-consensus/metanode/scripts/node
./update_ips.sh 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1

# 3. Chạy
./run_all.sh

# 4. Test TPS
cd mtn-simple-2025/cmd/tool/tps_blast
./run_multinode_load.sh 10 10000

# 5. Dừng
cd mtn-consensus/metanode/scripts/node
./stop_all.sh
```

> Xem thêm: [OPERATIONS_GUIDE.md](./OPERATIONS_GUIDE.md) — hướng dẫn đầy đủ run/stop/resume từng node.
