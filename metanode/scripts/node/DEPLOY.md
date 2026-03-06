# 🚀 Multi-Server Cluster Deployment Guide

Hệ thống deploy tự động: **build trên máy local → đẩy binary + config qua SSH → update IPs → start nodes trên remote servers**.

## Kiến trúc Cluster

```
┌─────────────────────┐    ┌─────────────────────┐
│  Server B           │    │  Server C           │
│  192.168.1.231      │    │  192.168.1.232      │
│                     │    │                     │
│  Node 0 (Leader)    │    │  Node 2             │
│  Node 1             │    │  Node 3             │
└─────────────────────┘    └─────────────────────┘
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
SERVER_B="192.168.1.231"     # Node 0, Node 1
SERVER_C="192.168.1.232"     # Node 2, Node 3

# Node → Server mapping
NODE_SERVER[0]="$SERVER_B"
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
1. ✅ **Phase 0** — Validate SSH tới tất cả servers
2. 🔨 **Phase 1** — Build Rust + Go binary từ source local
3. 🛑 **Phase 2** — Stop cluster cũ trên remote
4. 📦 **Phase 3** — Push binaries + configs tới từng server
5. 🌐 **Phase 4** — **Update IPs** trong config files (gọi `update_ips.sh`)
6. 🧹 **Phase 5** — Clean data cũ + Start nodes

### Các lệnh riêng lẻ

```bash
# Full deploy
./deploy_cluster.sh --all

# Chỉ build (không push, không start)
./deploy_cluster.sh --build

# Chỉ push binary + config + update IPs
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

## Update IPs — Chi tiết hoạt động

Khi deploy `--all` hoặc dùng flag `--ips`, script tự động gọi `update_ips.sh` trên **mỗi remote server** để cập nhật IP vào tất cả config files.

### Cách gọi
```bash
# deploy_cluster.sh tự động chạy trên remote:
bash update_ips.sh <NODE0_IP> <NODE1_IP> <NODE2_IP> <NODE3_IP> [NODE4_IP]

# Ví dụ thực tế:
bash update_ips.sh 192.168.1.231 192.168.1.231 192.168.1.232 192.168.1.232
```

IPs được lấy từ `NODE_SERVER[0..3]` trong `deploy.env`.

### Files bị ảnh hưởng

| File | Field được update | Mô tả |
|------|-------------------|--------|
| **Rust** `config/node_N.toml` | `network_address` | Địa chỉ P2P consensus (IP:900N) |
| | `peer_rpc_addresses` | Danh sách peer discovery của các node khác |
| **Go Master** `config-master-nodeN.json` | `meta_node_rpc_address` | Rust RPC endpoint (chỉ thay IP, giữ port) |
| **Go Sub** `config-sub-nodeN.json` | `meta_node_rpc_address` | Rust RPC endpoint |
| | `master_address` | Địa chỉ Go Master trên cùng server |
| **Genesis** `genesis.json` | `primary_address` | Validator primary address |
| | `worker_address` | Validator worker address |
| | `p2p_address` | P2P address dạng `/ip4/IP/tcp/PORT` |
| | `epoch_timestamp_ms` | Cập nhật timestamp epoch hiện tại |
| **TPS Tools** `run_multinode_load.sh` | `NODES`, `RPCS` | Danh sách nodes, RPC endpoints |
| | `block_hash_checker` | Nodes để check hash |
| **TPS configs** `tps_blast/config.json` | `parent_connection_address` | Node 0 Master port |
| | `tx_sender/config.json` | `parent_connection_address` | Node 0 Sub port |

### Port Mapping cố định

| Mục | Node 0 | Node 1 | Node 2 | Node 3 |
|-----|--------|--------|--------|--------|
| Consensus P2P | 9000 | 9001 | 9002 | 9003 |
| Peer RPC | 19000 | 19001 | 19002 | 19003 |
| Go Master Connection | 4201 | 6201 | 6211 | 6221 |
| Go Sub Connection | 4200 | 6200 | 6210 | 6220 |
| Go Master RPC | 8757 | 10747 | 10749 | 10750 |
| Go Sub RPC | 8646 | 10646 | 10650 | 10651 |
| Rust Metrics | 9100 | 9101 | 9102 | 9103 |

> **Quan trọng**: `update_ips.sh` chỉ thay đổi phần IP, giữ nguyên port. Ports là cố định và không cần thay đổi.

### Chạy thủ công (debug)

```bash
# Preview thay đổi (không ghi file)
./update_ips.sh --dry-run 192.168.1.231 192.168.1.231 192.168.1.232 192.168.1.232

# Thay đổi thật
./update_ips.sh 192.168.1.231 192.168.1.231 192.168.1.232 192.168.1.232

# Chạy localhost (single machine)
./update_ips.sh 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1
```

---

## Kiểm tra trạng thái

```bash
./deploy_status.sh
```

Kiểm tra trên mỗi server:
- ✅/❌ tmux sessions (go-master, go-sub, metanode)
- ✅/❌ RPC health (`/health` endpoint)
- 📊 Block height mỗi node
- 🔄 Consensus sync (chênh ≤2 blocks = OK)
- 📋 3 dòng log cuối Go Master

---

## Cấu trúc thư mục trên Remote

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
│   └── scripts/node/
│       ├── update_ips.sh                # Cập nhật IPs (tự động bởi deploy)
│       ├── deploy_cluster.sh
│       ├── deploy_stop.sh
│       └── deploy_status.sh
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

## RPC Endpoints

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
sshpass -p "password" ssh -o StrictHostKeyChecking=no abc@192.168.1.231 "echo ok"

# Thiếu sshpass?
sudo apt install sshpass
```

### Node không start
```bash
# Xem log trên remote server
ssh abc@192.168.1.231 "tail -50 /home/abc/chain-n/mtn-consensus/metanode/logs/node_0/go-master-stdout.log"
ssh abc@192.168.1.231 "tail -50 /home/abc/chain-n/mtn-consensus/metanode/logs/node_0/rust.log"

# Xem tmux session trực tiếp
ssh abc@192.168.1.231 "tmux attach -t go-master-0"
ssh abc@192.168.1.231 "tmux attach -t metanode-0"
```

### Nodes out of sync
```bash
# Check trạng thái
./deploy_status.sh

# Nếu chênh > 10 blocks, restart cluster
./deploy_stop.sh
./deploy_cluster.sh --start
```

### Clean restart (xóa tất cả data)
```bash
./deploy_cluster.sh --all   # Build + push + clean data + start
```

### Disk đầy trên remote
```bash
# Check disk
ssh abc@192.168.1.231 "df -h / && du -sh /home/abc/chain-n/*/"

# Dọn dẹp (xóa data cũ, logs, build cache)
ssh abc@192.168.1.231 "rm -rf /home/abc/chain-n/mtn-consensus/metanode/target"
ssh abc@192.168.1.231 "rm -rf /home/abc/chain-n/mtn-consensus/metanode/logs/*"
```

---

## Files tổng quan

| File | Mô tả |
|------|--------|
| `deploy.env` | Cấu hình SSH, IPs, paths, node mapping |
| `deploy_cluster.sh` | Script deploy chính (build → push → ips → start) |
| `deploy_stop.sh` | Dừng cluster trên tất cả servers |
| `deploy_status.sh` | Check trạng thái (tmux, RPC, block height, sync) |
| `update_ips.sh` | Cập nhật IPs trong Rust/Go/Genesis/TPS configs |
