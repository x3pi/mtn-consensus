# 🚀 Multi-Server Cluster Deployment Guide

Hệ thống deploy tự động: **chỉ cần sửa `deploy.env` → script tự build, push, update IPs, start**.

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

Mỗi node = 3 process: `go-master-N` + `go-sub-N` + `metanode-N` (tmux sessions)

---

## Cấu hình — `deploy.env`

**Đây là file duy nhất cần sửa.** Tất cả IPs trong config files sẽ tự động cập nhật theo `NODE_SERVER` mapping.

```bash
# SSH
SSH_USER="abc"
SSH_AUTH="password"          # "key" hoặc "password"
SSH_PASSWORD="1234@abcd"

# Server IPs
SERVER_B="192.168.1.231"
SERVER_C="192.168.1.232"

# Node → Server mapping (chỉ cần sửa ở đây)
NODE_SERVER[0]="$SERVER_B"   # Node 0 chạy trên Server B
NODE_SERVER[1]="$SERVER_B"   # Node 1 chạy trên Server B
NODE_SERVER[2]="$SERVER_C"   # Node 2 chạy trên Server C
NODE_SERVER[3]="$SERVER_C"   # Node 3 chạy trên Server C
```

---

## Sử dụng

### Full Deploy
```bash
./deploy_cluster.sh --all
```

Tự động thực hiện:
1. ✅ Validate SSH tới tất cả servers
2. 🔨 Build Rust + Go binary local
3. 🛑 Stop cluster cũ trên remote
4. 📦 Push binaries + configs tới từng server
5. 🌐 **Update IPs** — tự lấy từ `deploy.env`, gọi `update_ips.sh` trên mỗi server
6. 🧹 Clean data + 🚀 Start nodes

### Các lệnh khác
```bash
./deploy_cluster.sh --build              # Chỉ build
./deploy_cluster.sh --push --ips         # Chỉ push + update IPs
./deploy_cluster.sh --start              # Chỉ start
./deploy_cluster.sh --build --push --ips # Build + push (không start)
./deploy_stop.sh                         # Dừng cluster
./deploy_status.sh                       # Check trạng thái
```

---

## Tự động Update IPs

> **Chỉ cần sửa `deploy.env` → tất cả config files tự cập nhật khi deploy.**

Khi chạy `--all` hoặc `--ips`, script tự động:
1. Đọc `NODE_SERVER[0..3]` từ `deploy.env`
2. SSH vào mỗi remote server
3. Gọi `update_ips.sh` → cập nhật IP vào **tất cả** config files

### Files được tự động update

| File | Field | Mô tả |
|------|-------|--------|
| `config/node_N.toml` | `network_address` | Rust P2P consensus |
| | `peer_rpc_addresses` | Peer discovery |
| `config-master-nodeN.json` | `meta_node_rpc_address` | Go → Rust RPC |
| `config-sub-nodeN.json` | `meta_node_rpc_address`, `master_address` | Go Sub → Rust, Sub → Master |
| `genesis.json` | `primary_address`, `worker_address`, `p2p_address` | Validator addresses |
| `tps_blast/config.json` | `parent_connection_address` | TPS tool |

> Chỉ thay IP, giữ nguyên port. **Không cần sửa thủ công bất kỳ config nào.**

---

## Port Mapping (cố định)

| | Node 0 | Node 1 | Node 2 | Node 3 |
|---|--------|--------|--------|--------|
| Go Master RPC | 8757 | 10747 | 10749 | 10750 |
| Go Sub RPC | 8646 | 10646 | 10650 | 10651 |
| Go Master Connection | 4201 | 6201 | 6211 | 6221 |
| Rust P2P | 9000 | 9001 | 9002 | 9003 |
| Rust Peer RPC | 19000 | 19001 | 19002 | 19003 |
| Rust Metrics | 9100 | 9101 | 9102 | 9103 |

---

## Kiểm tra trạng thái

```bash
./deploy_status.sh
```

Kiểm tra: tmux sessions, RPC health, block height, consensus sync, log tails.

---

## Troubleshooting

```bash
# SSH test
sshpass -p "password" ssh -o StrictHostKeyChecking=no abc@192.168.1.231 "echo ok"

# Xem log
ssh abc@192.168.1.231 "tail -50 .../logs/node_0/go-master-stdout.log"
ssh abc@192.168.1.231 "tail -50 .../logs/node_0/rust.log"

# Attach tmux
ssh abc@192.168.1.231 "tmux attach -t go-master-0"

# Disk đầy → dọn
ssh abc@192.168.1.231 "rm -rf .../metanode/target .../metanode/logs/*"

# Out of sync → restart
./deploy_stop.sh && ./deploy_cluster.sh --start

# Clean restart (xóa data)
./deploy_cluster.sh --all
```

---

## Files

| File | Mô tả |
|------|--------|
| `deploy.env` | **Cấu hình duy nhất cần sửa** — SSH, IPs, node mapping |
| `deploy_cluster.sh` | Deploy chính (build → push → update IPs → start) |
| `deploy_stop.sh` | Dừng cluster |
| `deploy_status.sh` | Check trạng thái |
| `update_ips.sh` | Cập nhật IPs (tự động gọi bởi deploy) |
