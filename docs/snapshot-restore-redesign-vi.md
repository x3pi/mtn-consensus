# Thiết kế Khôi phục Snapshot "Slow Node"

**Ngày**: 2026-04-15
**Trạng thái**: ĐÃ TRIỂN KHAI (2026-04-15)
**Mục tiêu**: Đơn giản hóa việc khôi phục snapshot bằng cách xử lý node được khôi phục như node khởi động chậm. Loại bỏ toàn bộ cơ chế cold_start. Đảm bảo không fork.

---

## 1. Loại Node

Hai loại node với hành vi khác biệt:

| Loại | Vai trò | Nguồn Block | Thực thi Block? |
|------|---------|-------------|----------------|
| **Validator** | Tham gia đồng thuận | DAG commits + P2P sync (đuổi epoch) | CÓ — luôn thực thi |
| **SyncOnly** | Quan sát thụ động | Chỉ P2P sync | CÓ — luôn thực thi |

**Điểm khác biệt cốt lõi so với thiết kế hiện tại**: MỌI block nhận được đều được **THỰC THI** (NOMT state transition). Không còn "lưu vào LevelDB mà không thực thi" — đó là nguyên nhân gốc rễ của việc GEI bị inflate và fork.

---

## 2. Node Validator

### 2.1 Phát hiện khi khởi động

Khi khởi động, Rust query Go để lấy trạng thái hiện tại:

```
từ_go:
  go_epoch          = Epoch hiện tại của Go
  go_gei            = GEI đã thực thi cuối cùng của Go (từ NOMT, không bị inflate)
  go_block_number   = Số block cuối của Go

từ_network:
  net_epoch         = Epoch hiện tại của mạng (từ peers)
  net_block         = Số block mới nhất của mạng (từ peers)
```

Logic quyết định:

```
nếu go_epoch < net_epoch:
    → CHẬM EPOCH (Case 1: Đuổi epoch)
ngược lại:
    → CÙNG EPOCH (Case 2: Đuổi DAG)
```

---

### 2.2 Case 1: Chậm Epoch (Khởi động chậm / Snapshot cũ)

**Tình huống**: Node chậm một hoặc nhiều epoch.
Ví dụ: go_epoch=0, net_epoch=2.

**Luồng xử lý**:

- Lấy blocks đã commit từ peers (P2P sync)
- THỰC THI ngay từng block (NOMT state)
- Khi gặp epoch boundary → gọi advance_epoch trên Go
- Chuyển sang epoch tiếp theo

**Điểm quan trọng**: Mọi block đều được THỰC THI, không chỉ lưu trữ.
GEI của Go tăng tự nhiên với mỗi lần thực thi.
Trạng thái NOMT luôn nhất quán với GEI.

**Triển khai**: Rust lấy blocks từ peers, gửi đến Go qua RPC MỚI kích hoạt thực thi (không phải HandleSyncBlocksRequest chỉ lưu trữ). Hoặc sửa HandleSyncBlocksRequest để thực thi khi ở chế độ catch-up.

---

### 2.3 Case 2: Cùng Epoch (Đuổi DAG)

**Tình huống**: Node đang ở cùng epoch với mạng nhưng có thể thiếu blocks trong epoch. Xảy ra sau:
- Hoàn thành Case 1 (đuổi epoch)
- Khôi phục snapshot trong cùng epoch
- Khởi động chậm bình thường

**Luồng xử lý**:

1. Tham gia đồng thuận (Validator mode, amnesia recovery)
2. DAG synchronizer lấy blocks còn thiếu từ peers
3. Linearizer sản xuất commits từ DAG
4. CommitProcessor gửi commits đến Go:
   - GEI <= go_gei -> BO QUA (đã thực thi bởi NOMT)
   - GEI > go_gei -> THỰC THI (Go nâng cao state)
5. Node tham gia đồng thuận (vote, propose)

**Nguồn CHÍNH**: DAG commits
**DỰ PHÒNG**: Nếu DAG không cung cấp blocks (phát hiện chậm epoch), chuyển sang P2P sync để lấy và thực thi blocks còn thiếu.

**Cơ chế phát hiện chậm (Lag detection)** (dùng GEI, KHÔNG PHẢI block number):

KIỂM TRA ĐỊNH KỲ (mỗi N giây):
- dag_commit_gei = GEI cuối từ DAG commits
- go_gei = GEI đã thực thi của Go
- net_gei = GEI mới nhất của mạng (từ peers)
- gap = net_gei - go_gei

Nếu gap > NGƯỠNG && dag không tiến triển:
  -> Chuyển sang P2P sync (lấy + thực thi blocks)
  -> Khi đuổi kịp, chuyển lại sang DAG commits

Nếu gap <= NGƯỠNG && dag đang tiến triển:
  -> Tiếp tục với DAG commits (đồng thuận bình thường)

**Tại sao dùng GEI, không phải block number?**
- Block number có thể bằng nhau (ví dụ: cùng ở #100) trong khi GEI chênh lệch lớn
- Empty commits làm GEI tăng nhưng KHÔNG làm block number tăng
- Node "đuổi kịp" về block number có thể vẫn còn thiếu hàng trăm empty commits
- Chỉ GEI phản ánh đúng tiến độ thực thi thực sự

**Cơ chế phát hiện đuổi kịp (Catch-up detection)**:

Khi P2P sync đưa go_gei gần đến net_gei (gap <= NGƯỠNG_NHỎ):
  -> Dừng thực thi P2P sync
  -> DAG commits tiếp tục làm nguồn chính
  -> CommitProcessor NORMAL PATH xử lý dedup (bỏ qua GEI <= go_gei)

---

### 2.4 Cơ chế Chi tiết Phát hiện Chậm/Đuổi Kịp

**State Machine cho Chuyển đổi Nguồn:**

```
┌─────────────────┐     gap > NGƯỠNG_CAo      ┌─────────────────┐
│  DAG_CHÍNH      │ ───────────────────────────→│  P2P_DỰ_PHÒNG   │
│  (bình thường)  │  VÀ dag đứng yên N giây    │  (đang đuổi)    │
└────────┬────────┘                             └────────┬────────┘
         │                                               │
         │ gap <= NGƯỠNG_NHỎ                            │
         │ VÀ tiến triển liên tục M lần                 │
         │←──────────────────────────────────────────────┘
         ▼
┌─────────────────┐
│  DAG_CHÍNH      │
│  (tiếp tục)     │
└─────────────────┘
```

**Tham số:**

| Tham số | Giá trị | Mục đích |
|---------|---------|----------|
| `NGƯỠNG_CAO` | 100-200 GEI | Kích hoạt P2P dự phòng khi chậm đáng kể |
| `NGƯỠNG_NHỎ` | 10-20 GEI | An toàn để chuyển lại DAG |
| `DAG_STALL_TIMEOUT` | 5-10 giây | DAG được coi là "không tiến triển" nếu không có commit mới |
| `PROGRESS_WINDOW` | 3-5 lần kiểm tra liên tiếp | Xác nhận DAG thực sự tiến triển trước khi chuyển lại |
| `CHECK_INTERVAL` | 1-2 giây | Tần suất đánh giá độ chậm |

**Thuật toán (Rust side - ConsensusNode/EpochMonitor):**

```rust
struct LagMonitor {
    last_dag_gei: u64,
    last_check_time: Instant,
    dag_stall_count: u32,
    consecutive_progress: u32,
    current_source: BlockSource,
}

enum BlockSource {
    DagPrimary,      // Bình thường: DAG commits đến Go
    P2pFallback,     // Đuổi kịp: P2P blocks thực thi ở Go
}

impl LagMonitor {
    fn check_and_switch(&mut self, go_gei: u64, net_gei: u64, dag_gei: u64) {
        let gap = net_gei.saturating_sub(go_gei);
        let dag_making_progress = dag_gei > self.last_dag_gei;

        match self.current_source {
            BlockSource::DagPrimary => {
                // Kiểm tra xem cần chuyển SANG P2P dự phòng không
                if gap > NGƯỠNG_CAO && !dag_making_progress {
                    self.dag_stall_count += 1;
                    if self.dag_stall_count >= DAG_STALL_TIMEOUT / CHECK_INTERVAL {
                        // DAG đứng yên và ta đang chậm xa
                        self.switch_to_p2p_fallback(go_gei, net_gei);
                    }
                } else {
                    self.dag_stall_count = 0; // Reset nếu DAG tiến triển
                }
            }
            BlockSource::P2pFallback => {
                // Kiểm tra xem có thể chuyển LẠI DAG không
                if gap <= NGƯỠNG_NHỎ && dag_making_progress {
                    self.consecutive_progress += 1;
                    if self.consecutive_progress >= PROGRESS_WINDOW {
                        // Xác nhận: đuổi kịp và DAG đang chảy
                        self.switch_to_dag_primary();
                    }
                } else {
                    self.consecutive_progress = 0; // Reset nếu không đủ điều kiện
                }
            }
        }

        self.last_dag_gei = dag_gei;
    }

    fn switch_to_p2p_fallback(&mut self, go_gei: u64, net_gei: u64) {
        self.current_source = BlockSource::P2pFallback;
        // Khởi động P2P sync task lấy và thực thi blocks
        // từ go_gei+1 đến net_gei (hoặc đến khi chuyển lại)
        start_p2p_sync_execution(go_gei + 1);
    }

    fn switch_to_dag_primary(&mut self) {
        self.current_source = BlockSource::DagPrimary;
        // Dừng P2P sync task
        // DAG commits sẽ tự nhiên chảy đến Go
        stop_p2p_sync_execution();
    }
}
```

**Chế độ Thực thi P2P Sync (Go side):**

Khi ở chế độ P2P_DỰ_PHÒNG, HandleSyncBlocksRequest (hoặc RPC mới) phải:
1. Nhận blocks từ Rust
2. Thực thi ngay từng block qua NOMT (không chỉ lưu)
3. Cập nhật GEI sau mỗi lần thực thi
4. Xác nhận hoàn thành để Rust theo dõi tiến độ

**Xử lý Race Condition:**

```
Tình huống: Chuyển từ P2P_DỰ_PHÒNG về DAG_CHÍNH

1. Rust dừng P2P sync
2. Một số P2P blocks có thể vẫn đang trên đường đến Go
3. DAG commits có thể bắt đầu đến trước khi P2P kết thúc

Giải pháp:
- NORMAL PATH của Go (dựa trên GEI) tự động xử lý điều này
- Nếu DAG commit đến với GEI mà P2P chưa đạt,
  Go sẽ thực thi nó (GEI > go_gei)
- Nếu P2P block đến sau khi DAG đã thực thi GEI đó,
  Go sẽ bỏ qua nó (GEI <= go_gei)
- Không cần đồng bộ hóa đặc biệt — GEI guard là idempotent
```

**Metrics cần Theo dõi:**

```
# HELP mtn_lag_detected_total Tổng số lần phát hiện chậm
# TYPE mtn_lag_detected_total counter
mtn_lag_detected_total{node="validator-1"} 3

# HELP mtn_catch_up_duration_seconds Thời gian ở chế độ P2P dự phòng
# TYPE mtn_catch_up_duration_seconds histogram
mtn_catch_up_duration_seconds_bucket{le="10"} 12
mtn_catch_up_duration_seconds_bucket{le="60"} 5
mtn_catch_up_duration_seconds_bucket{le="300"} 2

# HELP mtn_gei_gap_current Khoảng cách GEI hiện tại so với mạng
# TYPE mtn_gei_gap_current gauge
mtn_gei_gap_current{node="validator-1"} 47

# HELP mtn_block_source_current Nguồn block hiện tại (0=DAG, 1=P2P)
# TYPE mtn_block_source_current gauge
mtn_block_source_current{node="validator-1"} 0
```

---

## 3. Node SyncOnly

### 3.1 Hoạt động bình thường

SyncOnly nodes KHÔNG tham gia đồng thuận. Chỉ nhận blocks từ P2P sync và thực thi chúng.

1. Nhận blocks từ peers qua P2P sync
2. THỰC THI từng block (NOMT state transition)
3. GEI và block number tăng tự nhiên
4. Phục vụ RPC queries từ clients

KHÔNG tham gia đồng thuận.
KHÔNG tương tác DAG.

### 3.2 Thăng cấp: SyncOnly -> Validator

Khi SyncOnly node được thăng cấp lên Validator (ví dụ: thay đổi staking):

1. Phát hiện thăng cấp (EpochMonitor thấy node trong committee mới)
2. Kiểm tra trạng thái epoch:
   - Chậm epoch? -> Case 1 (Đuổi epoch)
   - Cùng epoch?   -> Case 2 (Đuổi DAG)
3. Dừng P2P sync của SyncOnly
4. Khởi động Validator mode (tham gia đồng thuận)
5. Cùng luồng như khởi động Validator

**Điểm quan trọng**: thăng cấp tái sử dụng luồng Validator startup. Không cần code "chuyển đổi" đặc biệt.

---

## 4. Nguyên tắc Thiết kế Cốt lõi

### 4.1 THỰC THI MỌI BLOCK

**Mọi block nhận được (từ bất kỳ nguồn nào) đều được THỰC THI bởi Go (NOMT state).**

Không còn "lưu mà không thực thi". Điều này loại bỏ:
- GEI inflation (GEI chỉ tăng khi thực thi)
- State mismatch (NOMT luôn khớp với GEI)
- cold_start guards (không cần — GEI của Go luôn chính xác)
- State-awareness guards (không cần — không có state cũ)

### 4.2 GEI Source of Truth Duy nhất

GEI của Go = số commits đã THỰC THI bởi NOMT
          = luôn chính xác
          = không bao giờ bị inflate bởi P2P sync (vì P2P sync cũng thực thi)

Rust query GEI của Go -> luôn nhận giá trị thực thi thực sự
CommitProcessor bỏ qua GEI <= go_gei -> deduplication chính xác

**QUAN TRỌNG**: Theo dõi tiến độ dựa vào GEI, KHÔNG PHẢI block number. Block number và GEI là hai thứ riêng biệt:
- Block number chỉ tăng khi commit có transactions (non-empty)
- GEI tăng với MỌI commit (cả empty commits)

Để đồng bộ và phát hiện chậm, luôn dùng GEI. Block number có thể giống nhau giữa các node trong khi GEI chênh lệch đáng kể do empty commits.

### 4.3 Hai Nguồn Block, Một Đường Thực Thi

Nguồn 1: DAG commits (Validator, cùng epoch)
  -> CommitProcessor -> send_committed_subdag -> Go thực thi

Nguồn 2: P2P sync blocks (đuổi epoch, hoặc DAG chậm dự phòng)
  -> MỚI: sync_and_execute -> Go thực thi (KHÔNG chỉ lưu)

Cả hai nguồn -> Go THỰC THI -> NOMT tiến -> GEI tiến

### 4.4 Dự phòng Tự động

DAG commits (chính cho Validator)
    |
    |-- Đang tiến triển? -> Tiếp tục với DAG
    |
    |-- Chậm lại? -> Chuyển sang P2P sync + thực thi
                      |
                      |-- Đuổi kịp? -> Chuyển lại sang DAG

---

## 5. So sánh với Thiết kế Hiện tại

| Khía cạnh | Thiết kế Hiện tại | Thiết kế Mới |
|-----------|-------------------|--------------|
| P2P sync blocks | Lưu vào LevelDB, KHÔNG thực thi | **THỰC THI bởi NOMT** |
| GEI sau P2P sync | Inflated (LevelDB GEI > NOMT GEI) | **Chính xác** (GEI = NOMT) |
| cold_start flag | Cần thiết (phức tạp, 7 layers) | **Không cần** |
| cold_start_snapshot_gei | Cần thiết (field immutable) | **Không cần** |
| State-awareness guards | Cần thiết (L5, L6) | **Không cần** |
| Rủi ro fork | Nhiều vector (GEI inflation, state cũ) | **Tối thiểu** (đường thực thi đơn) |
| Độ phức tạp code | ~150 dòng guards | **~0 dòng guards** |
| Tốc độ đuổi kịp | Nhanh (chỉ lưu) rồi chậm (thực thi) | **Đều (thực thi khi nhận)** |

---

## 6. State Machine

Startup
   |
   v
Kiểm tra committee membership
   |
   |-- TRONG COMMITTEE -> Kiểm tra gap epoch
   |                      |
   |                      |-- Chậm epoch -> ĐUỔI EPOCH (P2P sync + thực thi)
   |                      |-- Cùng epoch  -> ĐUỔI DAG (DAG consensus + thực thi)
   |
   |-- NGOÀI COMMITTEE -> SYNCONLY (P2P sync + thực thi tất)
                           |
                           |-- Nếu được thăng cấp vào committee
                               -> khởi động lại như Validator

---

## 7. Ghi chú Triển khai

### 7.1 Thay đổi Rust

1. **consensus_node.rs**: start_as_validator = is_in_committee (bỏ kiểm tra dag_has_history + is_lagging). Validator luôn khởi động ConsensusAuthority.

2. **consensus_node.rs**: Xóa các field cold_start, cold_start_snapshot_gei khỏi ConsensusNode.

3. **executor.rs**: Xóa COLD-START PATH. Chỉ giữ NORMAL PATH (query Go GEI, bỏ qua đã-thực-thi).

4. **mode_transition.rs** + **consensus_setup.rs**: Xóa việc truyền cold_start.

5. **epoch_monitor.rs**: Đuổi epoch gửi blocks đến Go để THỰC THI (không chỉ lưu trữ).

6. MỚI: Cơ chế phát hiện chậm — theo dõi tiến độ DAG vs mạng, chuyển sang P2P sync+thực thi khi DAG chậm.

### 7.2 Thay đổi Go

1. RPC MỚI hoặc sửa HandleSyncBlocksRequest: P2P synced blocks phải được THỰC THI bởi NOMT, không chỉ lưu vào LevelDB. Đây là thay đổi Go quan trọng nhất.

2. **block_processor_sync.go**: Xóa logic DB-SYNC gap jump (GEI giờ luôn chính xác).

3. **block_processor_network.go**: Xóa TRANSITION SYNC state-awareness guard (không còn cần).

4. **app_blockchain.go**: SNAPSHOT FIX có thể giữ làm safety net hoặc xóa (GEI sẽ không inflate).

### 7.3 Bất biến Quan trọng

LUÔN LUÔN:
  storage.GetLastGlobalExecIndex() == GEI đã thực thi của NOMT

Điều này có nghĩa:
- Rust luôn nhận GEI chính xác từ Go
- CommitProcessor NORMAL PATH luôn đưa ra quyết định bỏ qua chính xác
- Không thể fork từ mismatch GEI

---

## 8. Lộ trình Di chuyển

## 8. Lộ trình Di chuyển (Đã Hoàn Thành)

Tất cả các giai đoạn đã được triển khai thành công:
1. ✅ **Giai đoạn 1**: Sửa HandleSyncBlocksRequest để thực thi blocks (không chỉ lưu) + Thêm RPC `sync_and_execute`.
2. ✅ **Giai đoạn 2**: Thay đổi khởi động Validator để luôn khởi động ConsensusAuthority (bỏ kiểm tra dag_has_history).
3. ✅ **Giai đoạn 3**: Thêm cơ chế phát hiện chậm (DAG -> P2P dự phòng -> DAG).
4. ✅ **Giai đoạn 4**: Xóa hoàn toàn máy móc cold_start (dọn dẹp 7 file Rust).
5. ✅ **Giai đoạn 5**: Xóa/đánh dấu các guards kế thừa ở phía Go (L5, L6, SNAPSHOT FIX).
6. ✅ **Hậu-Giai đoạn 5 (Sửa Fork)**: Loại bỏ `broadcastBackupToSub` khỏi các sync handlers.

---

## 9. Kiến trúc Mạng Trọng yếu: Truyền Block tới Sub Node

Trong quá trình triển khai, tôi đã phát hiện một kiến trúc gây fork nghiêm trọng khi block counter của Go Sub node tiến xa hơn consensus goroutine của Master (ví dụ: Sub ở #275 trong khi Master consensus ở #150).

### Nguyên nhân Gốc
Handler `HandleSyncBlocksRequest` lập tức đẩy MỌI block vừa được sync tới các Sub nodes qua hàm `broadcastBackupToSub(backupBytes, blockNum)`.
- Rust P2P catchup → Lấy từ peers → Gửi tới Go Master cục bộ
- Go Master `HandleSyncBlocksRequest` → Lưu vào LevelDB + **Broadcast tới Sub**
- Sub nhận được block và cập nhật state *trước khi* Master consensus xử lý block đó!

### Bản Sửa Lỗi (Fix)

```
✅ Kiến trúc Chuẩn cho việc Master -> Sub Push:

Master Consensus (Đường DUY NHẤT tới Sub):
  CommitWorker → GenerateBlock → CommitToDb → BroadcastToNetwork → Sub 🟢

Master Sync (Chỉ lưu, KHÔNG BAO GIỜ broadcast tới Sub):
  HandleSyncBlocksRequest → Lưa LevelDB → PersistBackup → STOP 🟢
```

Bằng cách loại bỏ `broadcastBackupToSub` khỏi cả hai luồng store-only và execute-mode của `HandleSyncBlocksRequest`, Sub node được đảm bảo chỉ đi theo Nguồn Sự thật Duy nhất (single source of truth) của Master (tức pipeline consensus). Các block được sync về chỉ được lưu vào PebbleDB (`block_data_topic-N`) để Sub có thể tự fetch chúng qua quá trình 3-tier recovery (backup → PebbleDB → peer) khi cần thiết. Việc này giữ nguyên nguyên tắc single writer principle và triệt tiêu fork sinh ra do race condition.
