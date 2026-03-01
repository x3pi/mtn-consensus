# Scripts để chạy Full System

## Tổng quan

Scripts để chạy toàn bộ hệ thống:
- **4 Rust Consensus Nodes** (Node 0, 1, 2, 3)
- **1 Go Sub Node** (config-sub-write.json)
- **1 Go Master Node** (config-master.json)

Mỗi lần chạy sẽ:
- ✅ Xóa dữ liệu cũ (Go sample + Rust storage)
- ✅ Tạo committee mới (epoch 0)
- ✅ Khởi động tất cả nodes

## Scripts

### 1. `run_full_system.sh` - Khởi động toàn bộ hệ thống

**Chức năng:**
1. Xóa dữ liệu cũ:
   - `mtn-simple-2025/cmd/simple_chain/sample/` (Go data)
   - `mtn-consensus/metanode/config/storage/` (Rust data)
2. Dừng các nodes đang chạy
3. Tạo committee mới cho 4 nodes (epoch 0)
4. Enable executor cho Node 0
5. Khởi động 4 Rust consensus nodes
6. Khởi động Go Sub Node (tmux session: `go-sub`)
7. Khởi động Go Master Node (tmux session: `go-master`)

**Cách sử dụng:**
```bash
cd /home/abc/chain-n/mtn-consensus/metanode
./scripts/run_full_system.sh
```

**Output:**
```
📋 Bước 1: Xóa dữ liệu cũ...
ℹ️  Xóa dữ liệu Go: ...
ℹ️  Xóa dữ liệu Rust: ...
✅ Đã xóa dữ liệu cũ

📋 Bước 2: Dừng các nodes đang chạy...
✅ Đã dừng các nodes cũ

📋 Bước 3: Tạo committee mới cho đồng thuận...
✅ Đã tạo committee mới

📋 Bước 4: Cấu hình executor cho Node 0...
✅ Executor đã được enable cho Node 0

📋 Bước 5: Khởi động 4 Rust consensus nodes...
✅ Đã khởi động 4 Rust nodes

📋 Bước 6: Khởi động Go Sub Node...
✅ Go Sub Node đã khởi động (tmux session: go-sub)

📋 Bước 7: Khởi động Go Master Node...
✅ Go Master Node đã khởi động (tmux session: go-master)

📋 Bước 8: Kiểm tra hệ thống...
🎉 Hệ thống đã được khởi động!
```

### 2. `stop_full_system.sh` - Dừng toàn bộ hệ thống

**Chức năng:**
1. Dừng Go Sub Node (tmux session: `go-sub`)
2. Dừng Go Master Node (tmux session: `go-master`)
3. Dừng 4 Rust consensus nodes
4. Xóa sockets

**Cách sử dụng:**
```bash
cd /home/abc/chain-n/mtn-consensus/metanode
./scripts/stop_full_system.sh
```

## Xem logs

### Rust Nodes
```bash
# Node 0
tmux attach -t metanode-0

# Node 1
tmux attach -t metanode-1

# Node 2
tmux attach -t metanode-2

# Node 3
tmux attach -t metanode-3
```

### Go Nodes
```bash
# Go Sub Node
tmux attach -t go-sub

# Go Master Node
tmux attach -t go-master
```

## Kiểm tra trạng thái

### Check processes
```bash
# Rust nodes
ps aux | grep metanode | grep -v grep

# Go nodes
ps aux | grep simple_chain | grep -v grep
```

### Check sockets
```bash
# Transaction sockets
ls -la /tmp/metanode-tx-*.sock

# Executor socket (chỉ Node 0)
ls -la /tmp/executor0.sock
```

### Check tmux sessions
```bash
tmux ls
```

## Luồng hoàn chỉnh

```
Go Sub (config-sub-write.json)
    ↓ Transactions → Rust Node 0 (127.0.0.1:10100)
Rust Node 0 (Consensus + Executor)
    ↓ Committed Blocks → Go Master (/tmp/executor0.sock)
Go Master (config-master.json)
    ↓ Execution
State Updated
```

## Troubleshooting

### Rust nodes không khởi động
- Check binary: `ls -la target/release/metanode`
- Build nếu cần: `cargo build --release --bin metanode`
- Check ports: `netstat -tuln | grep -E "9000|9001|9002|9003"`

### Go nodes không khởi động
- Check binary: `ls -la ../mtn-simple-2025/bin/simple_chain`
- Build nếu cần: `cd ../mtn-simple-2025 && go build -o bin/simple_chain ./cmd/simple_chain`
- Check config files: `ls -la ../mtn-simple-2025/cmd/simple_chain/config-*.json`

### Sockets không được tạo
- Check Rust Node 0 đang chạy: `tmux attach -t metanode-0`
- Check logs: `tail -f logs/node_0.log`

### Committee không được tạo
- Check binary: `./target/release/metanode --help`
- Generate thủ công: `./target/release/metanode generate --nodes 4 --output config`

