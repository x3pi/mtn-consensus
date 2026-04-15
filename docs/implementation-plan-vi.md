# Kế hoạch Triển khai: Snapshot Restore Redesign

**Ngày**: 2026-04-15  
**Trạng thái**: Phase 1-2 Hoàn thành, Phase 4-5 Chờ thực hiện

---

## Tóm tắt

Tài liệu này theo dõi việc triển khai Snapshot Restore Redesign để loại bỏ fork và đồng bộ chậm sau khi khôi phục snapshot.

**Nguyên nhân gốc**: P2P sync lưu blocks vào LevelDB mà không thực thi → GEI bị inflate → Rust bỏ qua commits → state cũ → FORK

**Giải pháp**: Thực thi tất cả blocks đã sync qua NOMT để GEI luôn phản ánh state đã thực thi thực sự.

---

## Trạng thái các Phase

| Phase | Mô tả | Trạng thái | Ưu tiên |
|-------|-------|------------|---------|
| 1 | Thực thi blocks qua NOMT (không chỉ lưu) | ✅ **HOÀN THÀNH** | Cực kỳ quan trọng |
| 2 | Đơn giản hóa khởi động validator | ✅ **HOÀN THÀNH** | Cao |
| 3 | Thêm cơ chế phát hiện chậm | ⏸️ **CHỜ** | Trung bình |
| 4 | Xóa machinery cold_start (cleanup) | ⏳ **CẦN LÀM** | Thấp |
| 5 | Xóa guards phía Go (cleanup) | ⏳ **CẦN LÀM** | Thấp |

---

## Phase 1: Thực thi Blocks qua NOMT ✅ HOÀN THÀNH

### Files Đã Sửa

#### 1. Định nghĩa Protobuf

**Rust**: `metanode/proto/validator_rpc.proto`
```protobuf
message SyncBlocksRequest {
    repeated BlockData blocks = 1;
    bool execute_mode = 2;  // MỚI: Khi true, Go thực thi blocks qua NOMT
}

message SyncBlocksResponse {
    uint64 synced_count = 1;
    uint64 last_synced_block = 2;
    string error = 3;
    uint64 last_executed_gei = 4;  // MỚI: GEI sau khi thực thi
}
```

**Go**: `pkg/proto/validator.proto` (thay đổi tương tự)

**Go**: `pkg/proto/validator.pb.go` (thêm fields + getters)

#### 2. Go: HandleSyncBlocksRequest (`executor/unix_socket_handler_epoch.go`)

**Thêm dispatch ở đầu hàm:**
```go
if request.GetExecuteMode() {
    return rh.handleSyncBlocksExecuteMode(request)
}
```

**Hàm mới: `handleSyncBlocksExecuteMode`**
- Áp dụng BackupDb state batches vào LevelDB
- Gọi `CommitBlockState(WithRebuildTries(), WithPersistToDB())` — KHÁC BIỆT CHÍNH
- Cập nhật con trỏ state trong bộ nhớ (header, epoch)
- Cập nhật counters lưu trữ (block number, GEI)
- Xóa cache MVM
- Lưu backup data + broadcast đến Sub nodes

**Helper: `persistBackupForSub`**

#### 3. Rust: ExecutorClient (`metanode/src/node/executor_client/block_sync.rs`)

**Thêm methods:**
```rust
pub async fn sync_and_execute_blocks(&self, blocks: Vec<proto::BlockData>) -> Result<(u64, u64, u64)>

async fn sync_blocks_inner(&self, blocks: Vec<proto::BlockData>, execute_mode: bool) -> Result<(u64, u64)>
```

**Sửa:**
- `sync_blocks()` gọi `sync_blocks_inner(blocks, false)`
- Chunk size nhỏ hơn cho execute mode (20 vs 50)
- Timeout dài hơn cho execute mode (120s vs 60s)

#### 4. Rust: Epoch Monitor (`metanode/src/node/epoch_monitor.rs`)

**Thay đổi:**
```rust
// CŨ:
client_arc.sync_blocks(blocks).await

// MỚI:
client_arc.sync_and_execute_blocks(blocks).await
```

Có fallback sang `sync_blocks` để tương thích ngược.

#### 5. Rust: Epoch Transition (`metanode/src/node/transition/epoch_transition.rs`)

**Cập nhật 2 vị trí gọi:**
- Dòng ~990: Deferred epoch sync
- Dòng ~1312: `fetch_and_sync_blocks_to_go`

#### 6. Rust: Catchup (`metanode/src/node/catchup.rs`)

**Cập nhật:**
- `sync_blocks_from_peers` gọi `sync_and_execute_blocks`

---

## Phase 2: Đơn giản hóa Khởi động Validator ✅ HOÀN THÀNH

### Files Đã Sửa

#### 1. `metanode/src/node/consensus_node.rs`

**Dòng ~1136: Đơn giản hóa `start_as_validator`**
```rust
// CŨ:
let start_as_validator = storage.is_in_committee
    && !storage.is_lagging
    && (dag_has_history || storage.current_epoch == 0);

// MỚI:
let start_as_validator = storage.is_in_committee;
```

**Dòng ~1270: Xóa SyncingUp mode**
```rust
// CŨ:
node_mode: if storage.is_in_committee {
    if storage.is_lagging || (!consensus.dag_has_history && storage.current_epoch > 0) {
        NodeMode::SyncingUp
    } else {
        NodeMode::Validator
    }
}

// MỚI:
node_mode: if storage.is_in_committee {
    NodeMode::Validator  // Luôn Validator cho in-committee
} else {
    NodeMode::SyncOnly
}
```

---

## Phase 3: Thêm Cơ chế Phát hiện Chậm ⏸️ CHỜ

### Chưa bắt đầu

Phase này thêm `LagMonitor`:
1. Theo dõi tiến độ DAG vs mạng
2. Phát hiện khi DAG chậm (gap > HIGH_THRESHOLD)
3. Chuyển sang P2P sync+execute fallback
4. Trở lại DAG mode khi đuổi kịp (gap < SMALL_THRESHOLD)

**Thiết kế đã có trong:**
- `docs/snapshot-restore-redesign.md` Section 2.4
- `docs/snapshot-restore-redesign-vi.md` Section 2.4

### Files cần sửa (khi triển khai)

| File | Thay đổi |
|------|----------|
| `metanode/src/node/lag_monitor.rs` | **FILE MỚI**: Implement state machine phát hiện chậm |
| `metanode/src/node/mod.rs` | Thêm module lag_monitor |
| `metanode/src/node/consensus_node.rs` | Khởi động lag_monitor, kết nối với commit_processor |
| `metanode/src/consensus/commit_processor/executor.rs` | Kiểm tra lag_monitor trước NORMAL PATH skip |

---

## Phase 4: Xóa Machinery cold_start ⏳ CẦN LÀM

### Mục đích

Cleanup: Xóa toàn bộ code cold_start vì không cần thiết sau Phase 1-2.

### Files cần sửa

| # | File | Thay đổi | Ghi chú |
|---|------|----------|---------|
| 4.1 | `metanode/src/consensus/commit_processor/executor.rs` | Xóa COLD-START PATH (dòng ~248-300) | Chỉ giữ NORMAL PATH |
| 4.2 | `metanode/src/node/transition/mode_transition.rs` | Xóa block `if node.cold_start` (dòng ~280-291) | Không cần truyền cold_start |
| 4.3 | `metanode/src/node/consensus_node.rs` | Xóa cold_start setup (dòng ~953-1009) | Xóa khởi tạo `cold_start` Arc |
| 4.4 | `metanode/src/node/mod.rs` | Xóa fields `cold_start` và `cold_start_snapshot_gei` khỏi ConsensusNode struct | Dòng ~189-199 |
| 4.5 | `metanode/src/node/consensus_node.rs` | Xóa cold_start khỏi khởi tạo ConsensusNode | Dòng ~1284-1289 |

### Code cần xóa

**executor.rs COLD-START PATH (dòng ~248-333):**
```rust
// Xóa toàn bộ block này
if self.cold_start.load(Ordering::SeqCst) {
    // COLD-START PATH
    let cold_start_skip_gei = self.cold_start_skip_gei.load(Ordering::SeqCst);
    // ... skip logic
}
```

**mode_transition.rs (dòng ~280-291):**
```rust
// Xóa block này
if node.cold_start {
    let cold_start_arc = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let snapshot_gei = node.cold_start_snapshot_gei;
    processor = processor
        .with_cold_start(cold_start_arc)
        .with_cold_start_skip_gei(snapshot_gei);
}
```

**ConsensusNode fields (mod.rs dòng ~189-199):**
```rust
// Xóa 2 fields này
pub(crate) cold_start: bool,
pub(crate) cold_start_snapshot_gei: u64,
```

---

## Phase 5: Xóa Guards phía Go ⏳ CẦN LÀM

### Mục đích

Cleanup: Xóa các guards phòng thủ không cần thiết sau Phase 1-2.

### Files cần sửa

| # | File | Thay đổi | Ghi chú |
|---|------|----------|---------|
| 5.1 | `block_processor_sync.go` | Xóa RESTORE-GAP-SKIP (dòng ~391-438) | Blocks giờ được thực thi tuần tự |
| 5.2 | `block_processor_sync.go` | Đơn giản hóa DB-SYNC gap jump (dòng ~348-360) | GEI luôn chính xác |
| 5.3 | `block_processor_network.go` | Xóa TRANSITION SYNC state-awareness guard | GEI chính xác, không bao giờ trigger |
| 5.4 | `app_blockchain.go` | Giữ SNAPSHOT FIX làm safety net | Phòng thủ sâu được phép |

### Code cần xóa

**RESTORE-GAP-SKIP (block_processor_sync.go dòng ~391-438):**
```go
// Xóa toàn bộ section RESTORE-GAP-SKIP
// Comment bắt đầu: "// ═══════════════════════════════════════════════════════════════════════════"
// Kết thúc trước: "// Case 3: Sequential block"
```

**TRANSITION SYNC guard (block_processor_network.go):**

Có thể xóa state-awareness check vì GEI giờ luôn chính xác:
```go
// Dòng ~217-271: Đơn giản hóa bằng cách xóa kiểm tra state mismatch
// Kiểm tra: if currentTrieRoot != targetStateRoot { ... skip ... }
```

---

## Checklist Kiểm thử

### Kiểm thử Phase 1
- [ ] Snapshot restore → node đồng bộ và thực thi blocks qua NOMT
- [ ] GEI sau sync = GEI đã thực thi NOMT (không bị inflate)
- [ ] Rust query Go GEI → nhận giá trị đúng → không skip commits cần thiết
- [ ] Không fork sau khi cold_start clear

### Kiểm thử Phase 2
- [ ] Validator không có DAG history khởi động ConsensusAuthority ngay lập tức
- [ ] DAG sync lấy blocks thiếu từ peers
- [ ] Không có độ trễ SyncingUp mode

### Kiểm thử Phase 3 (khi triển khai)
- [ ] Phát hiện chậm trigger khi DAG bị sau
- [ ] P2P fallback mode đuổi kịp node
- [ ] Tự động trở lại DAG mode khi đuổi kịp

---

## Lệnh Build

### Go
```bash
cd /home/abc/chain-n/mtn-simple-2025
go build ./...
```

### Rust
```bash
cd /home/abc/chain-n/mtn-consensus/metanode
cargo build --release
```

### Regenerate Protobuf (nếu cần)
```bash
# Go
protoc --go_out=. --go_opt=paths=source_relative pkg/proto/validator.proto

# Rust  
cd metanode && cargo build (prost-build xử lý proto)
```

---

## Ghi chú Migration

1. **Phase 1 là fix quan trọng** — loại bỏ nguyên nhân gốc của fork
2. **Phase 2 đơn giản hóa flow** — xóa transition SyncOnly→Validator không cần thiết
3. **Phase 4-5 là cleanup** — có thể làm bất cứ lúc nào, không thay đổi chức năng
4. **Phase 3 là enhancement** — thêm phát hiện chậm cho UX tốt hơn

**Triển khai an toàn**: Phase 1 một mình đã giảm rủi ro fork đáng kể. Triển khai 1+2 trước, sau đó 4+5 như cleanup.
