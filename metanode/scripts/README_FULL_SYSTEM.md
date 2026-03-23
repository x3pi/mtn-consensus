# Scripts để chạy Full System

> **⚠️ LƯU Ý:** File này mô tả kiến trúc cũ (`run_full_system.sh`). Hệ thống hiện tại sử dụng kiến trúc 5 nodes, mỗi node gồm **Go Master + Go Sub + Rust Metanode**. Xin tham khảo:
> - [scripts/README.md](./README.md) — Quản lý node mới
> - [scripts/node/OPERATIONS_GUIDE.md](./node/OPERATIONS_GUIDE.md) — Hướng dẫn vận hành đầy đủ

## Tổng quan

Hệ thống bao gồm **5 nodes** (0–4), mỗi node gồm 3 process:
- **Rust Metanode** — Consensus engine
- **Go Master** — Execution engine (EVM), xử lý blocks
- **Go Sub** — Tiếp nhận TX, đồng bộ state từ Master

### Kiến trúc hiện tại

```
Node 0: go-master-0 + go-sub-0 + metanode-0
Node 1: go-master-1 + go-sub-1 + metanode-1
Node 2: go-master-2 + go-sub-2 + metanode-2
Node 3: go-master-3 + go-sub-3 + metanode-3
Node 4: go-master-4 + go-sub-4 + metanode-4 (SyncOnly — không propose blocks)
```

## Scripts hiện tại (thay thế `run_full_system.sh`)

### Khởi động

```bash
cd /home/abc/chain-n/mtn-consensus/metanode/scripts/node

# Fresh start tất cả validators (4 nodes, khuyến nghị)
./run_all_validator.sh

# Fresh start tất cả 5 nodes (bao gồm SyncOnly)
./run_all.sh

# Fresh start 1 node
./run_node.sh <node_id>
```

### Dừng

```bash
./stop_all.sh              # Dừng tất cả
./stop_node.sh <node_id>   # Dừng 1 node
```

### Resume (giữ data)

```bash
./resume_all.sh              # Resume tất cả
./resume_node.sh <node_id>   # Resume 1 node
```

## Luồng hoàn chỉnh

```
Client
  ↓ TX → Go Sub Node N (RPC/API)
Go Sub Node N
  ↓ TX → Rust Metanode N (Unix Domain Socket)
Rust Metanode N (Consensus)
  ↓ Committed Blocks → Go Master N (Unix Domain Socket)
Go Master N (Execution — EVM)
  ↓ State + Receipts → Go Sub Node N (Block Sync)
Go Sub Node N
  ↓ Receipt → Client
```

## Xem logs

```bash
# Rust log
tmux attach -t metanode-0

# Go Master log
tmux attach -t go-master-0

# Go Sub log
tmux attach -t go-sub-0

# Hoặc xem file
tail -f logs/node_0/rust.log
tail -f logs/node_0/go-master-stdout.log
tail -f logs/node_0/go-sub-stdout.log
```

## Kiểm tra trạng thái

### Check processes
```bash
tmux ls
# Mong đợi: go-master-{0..4}, go-sub-{0..4}, metanode-{0..4}
```

### Check sockets
```bash
ls -la /tmp/metanode-tx-*.sock        # Transaction sockets
ls -la /tmp/executor*.sock            # Executor sockets
ls -la /tmp/rust-go-node*-master.sock # Go Master sockets
```

## Troubleshooting

### Rust nodes không khởi động
- Check binary: `ls -la target/release/metanode`
- Build nếu cần: `cargo +nightly build --release --bin metanode`
- Check ports: `netstat -tuln | grep -E "9000|9001|9002|9003|9004"`

### Go nodes không khởi động
- Check binary: `ls -la ../../mtn-simple-2025/cmd/simple_chain/simple_chain`
- Build nếu cần: `cd ../../mtn-simple-2025 && ./build.sh linux`
- Check config files: `ls -la ../../mtn-simple-2025/cmd/simple_chain/config-*.json`

### Sockets không được tạo
- Check Rust Node đang chạy: `tmux attach -t metanode-0`
- Check logs: `tail -f logs/node_0/rust.log`
