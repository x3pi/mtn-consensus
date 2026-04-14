# 🚀 Hướng Dẫn Triển Khai Cluster Metanode (Multi-Machine)

Bộ script này cung cấp giải pháp tự động hóa quá trình build, thiết lập, cấu hình địa chỉ IP, và khởi chạy một cụm (cluster) Metanode phân tán qua nhiều máy chủ Linux.

---

## 🛠 `deploy_cluster.sh` - Kịch bản Deploy Chính

Script `deploy_cluster.sh` đảm nhận 5 Phase tự động:

1. **Build**: Dịch mã nguồn Rust (Metanode) và Go (Simple Chain) tại máy local.
2. **Stop**: Dọn dẹp và dừng các tiến trình `tmux`, `simple_chain`, `metanode` cũ trên các remote servers.
3. **Push**: Gửi binaries (đã build), genesis, configs và keys sang remote servers qua SSH/rsync.
4. **Update IPs**: Tự động tìm/cập nhật IPs của các machines để thay Core configs cho liên lạc RPC/P2P.
5. **Start**: Dọn dẹp data cũ và khởi chạy `Go Master`, đợi Unix Sockets sẵn sàng -> mở `Go Sub` -> mở `Rust Metanode` qua các `tmux` sessions nền.

### Lưu ý cần cập nhật trước khi chạy

``` bash
# ở deploy_cluster.sh
LOCAL_CHAIN_DIR="/home/abc/nhat/con-chain-v2"
REMOTE_DEPLOY_DIR="/home/${SSH_USER}/nhat/con-chain-v2"
# ở update_ips.sh
GO_DIR="$(cd "$METANODE_DIR/../../mtn-simple-2025-xapian" && pwd)"
```

### 📜 Cách dùng

Mặc định đọc cấu hình server tại `deploy.env`. Có thể đổi sang file sinh cho cụm test net khác bằng flag `--env`.

``` bash
./update_ips.sh 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1
./fetch_logs.sh --env deploy-3machines.env

```bash
# Thực hiện toàn bộ (Build -> Stop -> Push -> Update IPs -> Start)
./deploy_cluster.sh --env deploy-3machines.env --all

# Chỉ Build các Binaries ở máy hiện tại
./deploy_cluster.sh --env deploy-3machines.env --build

# Chỉ gửi dữ liệu qua các nodes từ máy Build (Gửi lại configs, binaries)
./deploy_cluster.sh --env deploy-3machines.env --push --ips
# Không muốn build lại từ đầu do đã có sẵn binary, bạn có thể chạy Push + Setup + Start:
./deploy_cluster.sh --env deploy-3machines.env --push --ips --start

# Chỉ Start/Restart lại cluster, dọn rác và boot toàn mạng (Bỏ qua build/push)
./deploy_cluster.sh --env deploy-3machines.env --start

# Dừng toàn bộ cluster (Kill tmux, processes, socket cache)
./deploy_cluster.sh --env deploy-3machines.env --stop

# Khởi động lại cụm mạng nhưng GIỮ NGUYÊN lịch sử data (bỏ qua bước xóa trắng /data)
./deploy_cluster.sh --env deploy-3machines.env --start --keep-data

# Chỉ định thao tác Stop/Start/Push trên MỘT NODE CỤ THỂ (Không chạm tới các node khác)
./deploy_cluster.sh --env deploy-3machines.env --stop --only-node 1
./deploy_cluster.sh --env deploy-3machines.env --start --keep-data --only-node 1
```

---

## 📋 `deploy_logs.sh` - Quản lý Logs qua SSH

Lệnh hỗ trợ theo dõi file log và trạng thái cụm nhanh chóng ngay trên node điều phối. (Không cần ssh từng máy).

### 1. Xem trạng thái tổng quan cluster

Kiểm tra tmux sessions/tiến trình đang chạy hay không + peek log cuối.

```bash
./deploy_logs.sh --env deploy-3machines.env status
```

### 2. Theo dõi log realtime (Tail logs theo time-thực)

Theo dõi Go Master, Go Sub, Rust, hay C++ EVM logs.

```bash
# Log của Rust node 0 (trên máy 192.168.1.234)
./deploy_logs.sh --env deploy-3machines.env 0 rust

# Log của Go Master node 0
./deploy_logs.sh --env deploy-3machines.env 0 master

# Log của Go Sub node 0
./deploy_logs.sh --env deploy-3machines.env 0 sub

# EVM (C++) Smart Contract logs trên node 0
./deploy_logs.sh --env deploy-3machines.env 0 evm

# Tất cả layer của node 2 (gộp chung Output trên mạng 192.168.1.233)
./deploy_logs.sh --env deploy-3machines.env 2 all
```

### 3. Tailing đa-node cùng lúc trên 1 màn hình

Tính năng theo dõi (Tail) log chéo nhiều servers. Sẽ tự in logs vào terminal local hiện tại, prefix dễ nhìn như `[node-0 192.168....]`.

```bash
# Xem đồng thời Rust log của tất cả các node đang chạy 
./deploy_logs.sh --env deploy-3machines.env all rust
```

### 4. Fetch N dòng cuối (không tail real-time)

```bash
# Xem 100 dòng log cuối cùng của process Rust từ node 0
./deploy_logs.sh --env deploy-3machines.env last 0 rust 100
```

### 5. Attach nhanh vào Tmux đang chạy

Trường hợp kịch bản logs không làm nổi thì ta đi thẳng vào tmux container:

```bash
# Attach vào tmux layer Rust node 1
./deploy_logs.sh --env deploy-3machines.env tmux 1 rust
```

*(Lưu ý: Bạn có thể nhấn phím `Ctrl + B` rồi nhấn `D` để Detach/thoát terminal mà không làm chết Node)*
