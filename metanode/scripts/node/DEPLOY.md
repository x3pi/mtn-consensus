# 🚀 Multi-Server Cluster Deployment Guide

Hệ thống deploy tự động: **build trên máy local → đẩy binary + config qua SSH → start nodes trên remote servers**.

## Kiến trúc Cluster

```
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  Server A    │    │  Server B    │    │  Server C    │
│ 192.168.1.40 │    │ 192.168.1.231│    │ 192.168.1.232│
│              │    │              │    │              │
│  Node 0      │    │  Node 1      │    │  Node 2      │
│  (Leader)    │    │              │    │  Node 3      │
└──────────────┘    └──────────────┘    └──────────────┘
```

Mỗi node gồm **3 process**:
| Process | Mô tả | tmux session |
|---------|--------|--------------|
| Go Master | Xử lý block, consensus | `go-master-N` |
| Go Sub | Ghi data song song | `go-sub-N` |
| Rust Metanode | Consensus protocol | `metanode-N` |

---

## Yêu cầu trước khi deploy

### Máy local (build machine)
- Go 1.23.5+
- Rust nightly toolchain
- `sshpass` (nếu dùng password auth): `sudo apt install sshpass`
- Source code tại `/home/abc/chain-n/`

### Mỗi server remote
- SSH accessible (key hoặc password)
- `tmux` installed: `sudo apt install tmux`
- `curl` installed
- Đủ RAM (khuyến nghị 8GB+)

---

## Cấu hình

Sửa file **`deploy.env`** trước khi chạy:

```bash
# SSH
SSH_USER="abc"
SSH_AUTH="password"          # "key" hoặc "password"
SSH_PASSWORD="1234@abcd"     # Nếu dùng password
SSH_KEY=""                   # Nếu dùng key, path tới private key

# Server IPs
SERVER_A="192.168.1.40"      # Node 0
SERVER_B="192.168.1.231"     # Node 1
SERVER_C="192.168.1.232"     # Node 2, Node 3

# Node → Server mapping
NODE_SERVER[0]="$SERVER_A"
NODE_SERVER[1]="$SERVER_B"
NODE_SERVER[2]="$SERVER_C"
NODE_SERVER[3]="$SERVER_C"
```

> **Lưu ý**: Nếu thay đổi số node hoặc server, cần cập nhật cả port mapping trong `deploy_status.sh`.

---

## Sử dụng

### Full Deploy (lần đầu hoặc update code)

```bash
cd /home/abc/chain-n/mtn-consensus/metanode/scripts/node
./deploy_cluster.sh --all
```

Thực hiện tuần tự:
1. ✅ Validate SSH tới tất cả servers
2. 🔨 Build Rust + Go binary từ source local
3. 🛑 Stop cluster cũ trên remote
4. 📦 Push binaries + configs tới từng server
5. 🌐 Update IPs trong config files
6. 🧹 Clean data cũ
7. 🚀 Start Go Masters → đợi socket → Start Go Subs → Start Rust Metanodes

### Các lệnh riêng lẻ

```bash
# Chỉ build (không push, không start)
./deploy_cluster.sh --build

# Chỉ push binary + config (đã build trước đó)
./deploy_cluster.sh --push --ips

# Chỉ start (đã push trước đó)
./deploy_cluster.sh --start

# Kết hợp: build + push (không start)
./deploy_cluster.sh --build --push --ips

# Stop toàn bộ cluster
./deploy_stop.sh

# Check trạng thái
./deploy_status.sh
```

---

## Kiểm tra trạng thái

```bash
./deploy_status.sh
```

Output mẫu:
```
╔═══════════════════════════════════════════════════════════════╗
║  📊 CLUSTER STATUS CHECK — 2026-03-06 03:45:00              ║
╚═══════════════════════════════════════════════════════════════╝

  📍 Server: 192.168.1.40 — Nodes: [0]
  ✅ SSH connected
  ── Node 0 ──
    ✅ Go Master   — tmux session active
    ✅ Go Sub      — tmux session active
    ✅ Rust Node   — tmux session active
    ✅ Go Master RPC (:8757/health) — responded
    ✅ Block height: 150
    ✅ Go Sub RPC   (:8646/health) — responded

  📊 SUMMARY
  Services: 12 up / 0 down
  Block heights: node0=150 node1=150 node2=149 node3=150
  Consensus: ✅ Nodes in sync (diff: 1 blocks)
  🎉 All systems operational!
```

---

## Cấu trúc thư mục trên Remote

Sau khi deploy, mỗi server sẽ có:

```
/home/abc/chain-n/
├── mtn-consensus/metanode/
│   ├── target/release/metanode          # Rust binary
│   ├── config/
│   │   ├── node_N.toml                  # Rust config (per node)
│   │   ├── node_N_network_key.json      # Network key
│   │   ├── node_N_protocol_key.json     # Protocol key
│   │   ├── committee.json               # Committee config
│   │   └── storage/node_N/              # Rust storage data
│   ├── logs/node_N/                     # Log files
│   │   ├── go-master-stdout.log
│   │   ├── go-sub-stdout.log
│   │   └── rust.log
│   └── scripts/node/                    # Deploy scripts
│       ├── update_ips.sh
│       └── ...
└── mtn-simple-2025/cmd/simple_chain/
    ├── simple_chain                     # Go binary
    ├── genesis.json
    ├── config-master-nodeN.json         # Go Master config
    ├── config-sub-nodeN.json            # Go Sub config
    └── sample/nodeN/                    # Node data
        ├── data/
        ├── data-write/
        ├── back_up/
        └── back_up_write/
```

---

## Port Mapping

| Node | Go Master RPC | Go Sub RPC | Rust Metrics | Rust P2P |
|------|--------------|------------|--------------|----------|
| 0    | 8757         | 8646       | 9100         | 19000    |
| 1    | 10747        | 10646      | 9101         | 19001    |
| 2    | 10749        | 10650      | 9102         | 19002    |
| 3    | 10750        | 10651      | 9103         | 19003    |

### RPC Endpoints

```bash
# Health check
curl http://<server_ip>:<master_rpc_port>/health

# Block number
curl http://<server_ip>:<master_rpc_port>/block_number

# Pipeline stats
curl http://<server_ip>:<master_rpc_port>/pipeline/stats
```

---

## Troubleshooting

### SSH không kết nối được
```bash
# Test SSH thủ công
sshpass -p "password" ssh -o StrictHostKeyChecking=no abc@192.168.1.40 "echo ok"

# Thiếu sshpass?
sudo apt install sshpass
```

### Node không start
```bash
# Xem log trên remote server
ssh abc@192.168.1.40 "tail -50 /home/abc/chain-n/mtn-consensus/metanode/logs/node_0/go-master-stdout.log"

# Xem tmux session trực tiếp
ssh abc@192.168.1.40 "tmux attach -t go-master-0"
```

### Nodes out of sync
```bash
# Chạy deploy_status.sh để xem block height
./deploy_status.sh

# Nếu chênh > 10 blocks, restart cluster
./deploy_stop.sh
./deploy_cluster.sh --start
```

### Clean restart (xóa tất cả data)
```bash
./deploy_cluster.sh --all   # Build + push + clean data + start
```

---

## Files tổng quan

| File | Mô tả |
|------|--------|
| `deploy.env` | Cấu hình SSH, IPs, paths |
| `deploy_cluster.sh` | Script deploy chính |
| `deploy_stop.sh` | Dừng cluster |
| `deploy_status.sh` | Check trạng thái |
| `update_ips.sh` | Cập nhật IPs trong configs |
