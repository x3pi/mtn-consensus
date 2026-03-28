# Vận hành Cluster Metanode — Khởi động, Tắt & Khởi động lại

> **Script chính**: `mtn-orchestrator.sh`
> **Vị trí**: `mtn-consensus/metanode/scripts/mtn-orchestrator.sh`
> **Symlink**: `chain-n/mtn-orchestrator.sh` → file trên

---

## Mục lục

1. [Kiến trúc Cluster](#kiến-trúc-cluster)
2. [Khởi động (Start)](#khởi-động-start)
3. [Tắt (Stop)](#tắt-stop)
4. [Khởi động lại (Restart)](#khởi-động-lại-restart)
5. [Quản lý từng Node](#quản-lý-từng-node)
6. [Cơ chế Crash Safety](#cơ-chế-crash-safety)
7. [Xử lý sự cố](#xử-lý-sự-cố)
8. [Kiểm tra trạng thái](#kiểm-tra-trạng-thái)

---

## Kiến trúc Cluster

Mỗi cluster gồm **5 nodes** (0–4), mỗi node có 3 process:

```
┌─────────────────────────────────────────────────────┐
│                    Node N                            │
│                                                      │
│  ┌──────────────┐   IPC    ┌──────────────────────┐ │
│  │   Rust        │ ◄──────► │   Go Master          │ │
│  │  Consensus    │  (UDS)   │  (State + Execution) │ │
│  └──────────────┘          └────────┬─────────────┘ │
│                                      │ broadcast     │
│                              ┌───────▼─────────────┐ │
│                              │   Go Sub             │ │
│                              │  (State Replica)     │ │
│                              └─────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

### Vai trò

| Process | Vai trò | Binary | Session tmux |
|:--------|:--------|:-------|:-------------|
| **Rust Consensus** | Đồng thuận DAG, sản xuất commits | `metanode` | `metanode-N` |
| **Go Master** | Thực thi transactions, quản lý state | `simple_chain` | `go-master-N` |
| **Go Sub** | Bản sao state từ Master, phục vụ queries | `simple_chain` | `go-sub-N` |

### Thứ tự khởi động / tắt

```
Khởi động: Go Master → Go Sub → Rust Consensus
Tắt:       Rust Consensus → Go Sub → Go Master
```

> **Quan trọng**: Go Master phải sẵn sàng TRƯỚC vì Rust sẽ query Go ngay khi khởi động.

---

## Khởi động (Start)

### Fresh Start (lần đầu hoặc reset hoàn toàn)

```bash
./mtn-orchestrator.sh start --fresh
```

**Hành động:**
1. Dừng tất cả session cũ (nếu có)
2. **Xóa toàn bộ dữ liệu**: Rust storage + Go data/backup
3. Go Master khởi tạo **genesis block** (deploy system contracts)
4. Go Sub kết nối tới Master
5. Rust bắt đầu đồng thuận

**Thời gian**: ~30-60 giây (genesis processing mất ~1-2 phút)

> ⚠️ **Lưu ý**: Sau `--fresh`, hệ thống cần vài phút xử lý genesis trước khi bắt đầu tạo blocks. Trong thời gian này `block=0` là bình thường.

### Start với dữ liệu cũ (restart sau khi stop)

```bash
./mtn-orchestrator.sh start
```

**Hành động:**
1. Go Master load block cuối cùng từ PebbleDB
2. Go Sub sync state từ Master
3. Xóa `executor_state` của Rust (tránh recovery gap)
4. Rust query Go Master để lấy block number hiện tại
5. Rust bắt đầu đồng thuận từ vị trí Go

**Log mong đợi (Go Master)**:
```
Using existing block (not init genesis)
✅ [STARTUP] Initialized LastGlobalExecIndex from last block header: gei=XXXX (block=#YY)
```

---

## Tắt (Stop)

```bash
./mtn-orchestrator.sh stop
```

### Quy trình tắt an toàn (3 phases)

```
Phase 1: SIGTERM → Rust Consensus (tất cả 5 nodes)
         ↓
         Chờ 10s (drain pipeline — Go xử lý hết blocks còn trong queue)
         ↓
Phase 2: SIGTERM → Go Sub (tất cả 5 nodes)
         ↓
         Chờ 5s (flush state xuống disk)
         ↓  
Phase 3: SIGTERM → Go Master (tất cả 5 nodes)
         Go Master thực hiện:
           1. StopWait(12s) — chờ pending operations hoàn thành
           2. SaveLastBlockSync() — ghi block cuối xuống disk (atomic)
           3. FlushAll() — flush toàn bộ PebbleDB memtable → SST files
           4. CloseAll() — đóng tất cả database handles
         ↓
         Dọn sạch UDS sockets
```

### Timeout & SIGKILL

- Mỗi process được chờ tối đa **30 giây** để tự dừng
- Nếu quá thời gian → gửi SIGKILL (có thể mất dữ liệu!)
- Log cảnh báo: `⚠️ ... chưa dừng sau 30s → SIGKILL`

### Kiểm tra sau khi stop

```bash
# Kiểm tra không còn process orphan
pgrep -f "simple_chain.*config-" && echo "WARN: Go orphans!" || echo "OK"
pgrep -f "metanode start" && echo "WARN: Rust orphans!" || echo "OK"
```

---

## Khởi động lại (Restart)

### Restart giữ dữ liệu

```bash
./mtn-orchestrator.sh restart
```

Tương đương: `stop` → chờ 3s → `start`

### Restart xóa dữ liệu

```bash
./mtn-orchestrator.sh restart --fresh
```

Tương đương: `stop` → chờ 3s → `start --fresh`

---

## Quản lý từng Node

### Dừng 1 node

```bash
./mtn-orchestrator.sh stop-node <node_id>     # VD: stop-node 2
```

### Khởi động 1 node

```bash
./mtn-orchestrator.sh start-node <node_id>    # VD: start-node 2
```

### Restart 1 node

```bash
./mtn-orchestrator.sh restart-node <node_id>  # VD: restart-node 2
```

---

## Cơ chế Crash Safety

### 1. PebbleDB Full Flush

- Mỗi 10 giây, Go flush **toàn bộ memtable → SST files** (không chỉ WAL)
- Đảm bảo dữ liệu tồn tại trên disk ngay cả khi process bị kill

### 2. Last Block Sync Save

- Khi shutdown, Go ghi block cuối cùng **đồng bộ** xuống disk
- Sử dụng `pebble.Sync` thay vì async batch write
- File backup: `back_up/last_block_backup.json` (dự phòng)

### 3. Safety Guard — Chống Genesis Re-init

Khi Go Master khởi động, nếu **không tìm thấy block cuối cùng**:

| Tình huống | Kiểm tra | Hành động |
|:-----------|:---------|:----------|
| Fresh start | Không có SST files trong `account_state/` | ✅ Khởi tạo genesis bình thường |
| Crash/corrupt | Có SST files nhưng mất block key | 🚨 **REFUSE** — không xóa state, yêu cầu operator xử lý |

### 4. Rust Storage Cleanup

- Mỗi lần `start` hoặc `restart`, orchestrator **xóa toàn bộ Rust storage** (`config/storage/node_*`)
- Bao gồm: DAG commits, executor_state, persisted indices
- Tránh lỗi "Gap detected in block sequence" do stale DAG commits sau epoch GC
- An toàn vì Rust luôn query Go Master để lấy vị trí hiện tại và rebuild consensus từ peers

---

## Xử lý sự cố

### ❌ Lỗi: "Gap detected in block sequence"

**Nguyên nhân**: Rust DAG storage chứa stale commits từ epoch cũ, không khớp Go block number.

**Giải pháp**: Đã được fix tự động — orchestrator xóa toàn bộ Rust storage trước mỗi lần start. Nếu vẫn gặp (chạy Rust thủ công):

```bash
# Xóa thủ công
for i in 0 1 2 3 4; do
  rm -rf mtn-consensus/metanode/config/storage/node_${i}
done
# Restart
./mtn-orchestrator.sh start
```

### ❌ Lỗi: "CORRUPTED BLOCK DATABASE: lastBlock not found but data exists"

**Nguyên nhân**: Block database mất key `lastBlockHash` nhưng account state vẫn còn (crash giữa chừng).

**Giải pháp**:

```bash
# Kiểm tra backup file
cat mtn-simple-2025/cmd/simple_chain/sample/node0/back_up/last_block_backup.json

# Nếu backup có dữ liệu → Go sẽ tự phục hồi từ backup khi restart
./mtn-orchestrator.sh start

# Nếu không có backup → cần fresh start
./mtn-orchestrator.sh start --fresh
```

### ❌ Lỗi: "No existing block found" sau restart (không dùng --fresh)

**Nguyên nhân**: PebbleDB chưa flush lastBlock xuống SST trước khi process bị kill.

**Giải pháp**: Luôn dùng `./mtn-orchestrator.sh stop` thay vì `kill -9`. Nếu đã xảy ra:

```bash
# Kiểm tra backup
cat mtn-simple-2025/cmd/simple_chain/sample/node0/back_up/last_block_backup.json

# Fresh start nếu không thể phục hồi
./mtn-orchestrator.sh start --fresh
```

### ❌ Lỗi: Sub node stuck ở "Block #N not found locally"

**Nguyên nhân**: Sub node yêu cầu block mà Master chưa có trong backup storage.

**Giải pháp**:

```bash
# Restart riêng Sub node
./mtn-orchestrator.sh restart-node <node_id>
```

### ❌ Hệ thống không tạo block (block=0 sau khi start)

**Nguyên nhân**: Chưa có transactions. Consensus chạy nhưng chỉ gửi empty commits.

**Giải pháp**: Gửi transactions:

```bash
cd mtn-simple-2025/cmd/tool/tx_sender
go run . -loop
```

---

## Kiểm tra trạng thái

### Status tổng quan

```bash
./mtn-orchestrator.sh status
```

Output mẫu:
```
╔═══════════════════════════════════════════════════════╗
║  Node  │ Rust Consensus │ Go Master │ Go Sub │ Sockets ║
╠═══════════════════════════════════════════════════════╣
║    0   │ ✅ 496078     │ ✅ 495358 │ ✅ 495741 │ 3/3 ✅ ║
║    1   │ ✅ 496201     │ ✅ 495441 │ ✅ 495794 │ 3/3 ✅ ║
╚═══════════════════════════════════════════════════════╝
```

### Xem log theo node

```bash
./mtn-orchestrator.sh logs 0           # Tất cả log node 0
./mtn-orchestrator.sh logs 0 master    # Chỉ Go Master
./mtn-orchestrator.sh logs 0 sub       # Chỉ Go Sub
./mtn-orchestrator.sh logs 0 rust      # Chỉ Rust

# Hoặc dùng script riêng
cd metanode/scripts/logs
./go-master.sh 0 -f     # Follow Go Master node 0
./rust.sh 0 -f           # Follow Rust node 0
```

### Kiểm tra block production

```bash
# Block hiện tại (Go Master)
grep "block=#" mtn-consensus/metanode/logs/node_0/go-master-stdout.log | tail -3

# GEI hiện tại (Rust → Go)
grep "INIT.*Returning.*block=" mtn-consensus/metanode/logs/node_0/go-master/epoch_*/App.log | tail -1

# Backup file
cat mtn-simple-2025/cmd/simple_chain/sample/node0/back_up/last_block_backup.json
```

---

## Tham chiếu nhanh

| Lệnh | Mô tả |
|:------|:------|
| `start --fresh` | Xóa hết, khởi động mới hoàn toàn |
| `start` | Khởi động giữ dữ liệu cũ |
| `stop` | Tắt an toàn (flush disk) |
| `restart` | Tắt rồi khởi động (giữ data) |
| `restart --fresh` | Tắt rồi khởi động mới |
| `status` | Hiển thị trạng thái cluster |
| `logs <N> [layer]` | Xem log node N |
| `stop-node <N>` | Tắt 1 node |
| `start-node <N>` | Khởi động 1 node |
| `restart-node <N>` | Restart 1 node |

---

## Cấu trúc thư mục dữ liệu

```
mtn-simple-2025/cmd/simple_chain/sample/
├── node0/
│   ├── data/data/              # PebbleDB chính (blocks, account_state, ...)
│   ├── back_up/                # Backup storage + last_block_backup.json
│   ├── data-write/             # Go Sub data (bản sao)
│   └── back_up_write/          # Go Sub backup
└── node1/ ... node4/

mtn-consensus/metanode/config/storage/
├── node_0/
│   ├── executor_state/         # Persisted GEI (tự động xóa khi restart)
│   └── ...                     # DAG storage
└── node_1/ ... node_4/
```
