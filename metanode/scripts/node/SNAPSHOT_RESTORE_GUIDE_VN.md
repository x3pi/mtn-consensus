# 📸 Hướng dẫn khôi phục Node từ Snapshot

## Yêu cầu
- Cluster đang chạy (ít nhất Node 0 phải hoạt động với snapshot server)
- Snapshot tồn tại trên Node 0 (kiểm tra: `curl http://localhost:8700/api/snapshots`)

---

## Bước 1: Kiểm tra snapshot mới nhất

```bash
curl -s http://localhost:8700/api/snapshots | python3 -m json.tool
```

Output mẫu:
```json
[{
    "epoch": 1,
    "block_number": 50,
    "snapshot_name": "snap_epoch_1_block_50",
    "method": "hardlink"
}]
```

## Bước 2: Stop Node 2

```bash
cd ~/chain-n/mtn-consensus/metanode/scripts/node
./stop_node.sh 2
```

Kiểm tra đã dừng:
```bash
tmux ls | grep -E "master-2|sub-2|metanode-2"
# Không có kết quả → đã dừng
```

## Bước 3: Xóa sạch dữ liệu Node 2

```bash
NODE_DATA=~/chain-n/mtn-simple-2025/cmd/simple_chain/sample/node2

rm -rf $NODE_DATA/data
rm -rf $NODE_DATA/data-write
rm -rf $NODE_DATA/back_up
rm -rf $NODE_DATA/back_up_write
```

## Bước 4: Khôi phục từ Snapshot

### Cách A: Cùng máy (hardlink — tức thì)

```bash
SNAP_NAME="snap_epoch_1_block_50"  # ← thay bằng snapshot mới nhất
SNAP_DIR=~/chain-n/mtn-simple-2025/cmd/simple_chain/snapshot_data_node0/$SNAP_NAME
NODE_DATA=~/chain-n/mtn-simple-2025/cmd/simple_chain/sample/node2

# Copy toàn bộ snapshot sang data/data/ bằng hardlink (rất nhanh)
cp -rl "$SNAP_DIR" "$NODE_DATA/data/data"

# Tạo thư mục cần thiết
mkdir -p "$NODE_DATA/data/data/xapian_node"
mkdir -p "$NODE_DATA/data-write/data/xapian_node"
mkdir -p "$NODE_DATA/back_up"
mkdir -p "$NODE_DATA/back_up_write"
```

### Cách B: Khác máy (tải qua HTTP)

```bash
SNAP_NAME="snap_epoch_1_block_50"  # ← thay bằng snapshot mới nhất
SOURCE_IP="192.168.1.100"          # ← IP của Node 0
NODE_DATA=~/chain-n/mtn-simple-2025/cmd/simple_chain/sample/node2

mkdir -p "$NODE_DATA/data/data"

# Tải bằng wget (hỗ trợ resume)
cd "$NODE_DATA/data/data"
wget -c -r -np -nH --cut-dirs=2 "http://$SOURCE_IP:8700/files/$SNAP_NAME/"

# Hoặc tải bằng aria2c (nhanh hơn, 16 connections)
# aria2c -x 16 -s 16 -c "http://$SOURCE_IP:8700/files/$SNAP_NAME/"

# Tạo thư mục cần thiết
mkdir -p "$NODE_DATA/data/data/xapian_node"
mkdir -p "$NODE_DATA/data-write/data/xapian_node"
mkdir -p "$NODE_DATA/back_up"
mkdir -p "$NODE_DATA/back_up_write"
```

## Bước 5: Khởi động lại Node 2

```bash
cd ~/chain-n/mtn-consensus/metanode/scripts/node
./resume_node.sh 2
```

## Bước 6: Kiểm tra kết quả

```bash
# Kiểm tra tmux sessions
tmux ls | grep -E "master-2|sub-2|metanode-2"

# Kiểm tra Go Master hoạt động
tail -5 ~/chain-n/mtn-consensus/metanode/logs/node_2/go-master-stdout.log

# Kiểm tra Rust recovery (replay commits)
tail -10 ~/chain-n/mtn-consensus/metanode/logs/node_2/rust.log

# Đợi ~3 phút rồi kiểm tra đồng bộ consensus
tail -3 ~/chain-n/mtn-consensus/metanode/logs/node_2/rust.log | grep "round"
```

**Kết quả mong đợi:**
- 3 tmux sessions: `go-master-2`, `go-sub-2`, `metanode-2`
- Go: `App is running`, `last_committed_block` = block trong snapshot
- Rust: Recovery replay commits → sau ~3 phút → tham gia consensus ở round hiện tại

---

## Script tự động (1 lệnh)

```bash
#!/bin/bash
# restore_node.sh <node_id> [snapshot_name]
# Ví dụ: ./restore_node.sh 2 snap_epoch_1_block_50

NODE_ID="${1:?Usage: $0 <node_id> [snapshot_name]}"
SNAP_NAME="${2:-$(curl -sf http://localhost:8700/api/snapshots | python3 -c "import sys,json; snaps=json.load(sys.stdin); print(snaps[-1]['snapshot_name'])" 2>/dev/null)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SNAP_DIR=~/chain-n/mtn-simple-2025/cmd/simple_chain/snapshot_data_node0/$SNAP_NAME
NODE_DATA=~/chain-n/mtn-simple-2025/cmd/simple_chain/sample/node$NODE_ID

echo "🛑 Stop Node $NODE_ID..."
"$SCRIPT_DIR/stop_node.sh" "$NODE_ID" 2>/dev/null || true
sleep 3

echo "🗑️  Xóa data Node $NODE_ID..."
rm -rf "$NODE_DATA/data" "$NODE_DATA/data-write" "$NODE_DATA/back_up" "$NODE_DATA/back_up_write"

echo "📸 Restore từ $SNAP_NAME..."
cp -rl "$SNAP_DIR" "$NODE_DATA/data/data"
mkdir -p "$NODE_DATA/data/data/xapian_node" "$NODE_DATA/data-write/data/xapian_node"
mkdir -p "$NODE_DATA/back_up" "$NODE_DATA/back_up_write"

echo "🚀 Khởi động Node $NODE_ID..."
"$SCRIPT_DIR/resume_node.sh" "$NODE_ID"

echo "✅ Node $NODE_ID đã restore và khởi động!"
echo "   Kiểm tra: tail -f ~/chain-n/mtn-consensus/metanode/logs/node_$NODE_ID/rust.log"
```

---

## Lưu ý quan trọng

> ⚠️ **Không khôi phục Node 0** — Node 0 là nguồn snapshot và xử lý TX chính. Nếu cần restore Node 0, phải chọn node khác có snapshot.

> ⚠️ **Sau restore**, Rust Metanode sẽ mất ~3 phút replay commits. Trong thời gian này node **KHÔNG tham gia consensus** nhưng cluster vẫn hoạt động bình thường (chỉ cần 3/4 nodes).

> 💡 Port snapshot HTTP mỗi node: Node 0 = `8700`, Node 1 = `8747`, Node 2 = `8749`, Node 3 = `8727`.
