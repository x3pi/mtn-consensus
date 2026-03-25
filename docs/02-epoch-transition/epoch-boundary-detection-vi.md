# Phân Tích Chi Tiết: Xác Định Block Cuối Epoch

Tài liệu này giải thích chi tiết cách **Full Node (SyncOnly)** và **Validator** biết chính xác block nào và `global_exec_index` nào là cuối cùng của một epoch, từ đó lấy committee và chuyển đổi epoch.

---

## 1. Tổng Quan Flow

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     EPOCH BOUNDARY DETECTION FLOW                        │
└──────────────────────────────────────────────────────────────────────────┘

VALIDATOR MODE:
┌─────────────────────────────────────────────────────────────────────────┐
│  1. SystemTransactionProvider kiểm tra: elapsed_time >= epoch_duration  │
│  2. Tạo EndOfEpoch SystemTransaction, đưa vào block                     │
│  3. Block được commit → CommitProcessor nhận CommittedSubDag            │
│  4. extract_end_of_epoch_transaction() tìm thấy EndOfEpoch              │
│  5. global_exec_index tại commit này = EPOCH BOUNDARY                   │
│  6. Callback → AdvanceEpoch(new_epoch, global_exec_index)               │
└─────────────────────────────────────────────────────────────────────────┘

FULL NODE (SyncOnly) MODE:
┌─────────────────────────────────────────────────────────────────────────┐
│  1. epoch_monitor.rs poll Go Master định kỳ (cấu hình, mặc định 10s) │
│  2. Gọi get_current_epoch() → phát hiện epoch thay đổi                  │
│  3. Gọi get_epoch_boundary_data(new_epoch) để lấy:                      │
│     - boundary_block (global_exec_index cuối epoch trước)               │
│     - validators (committee mới)                                        │
│     - epoch_timestamp_ms                                                │
│  4. Nếu node có trong committee → transition_to_validator()             │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Chi Tiết: EndOfEpoch System Transaction

### 2.1. Cấu Trúc

```rust
// File: metanode/meta-consensus/core/src/system_transaction.rs

pub enum SystemTransactionKind {
    EndOfEpoch {
        new_epoch: u64,              // Epoch mới
        new_epoch_timestamp_ms: u64, // Timestamp bắt đầu epoch mới
        commit_index: u32,           // Commit index khi tạo transaction
    },
}
```

### 2.2. Khi Nào EndOfEpoch Được Tạo?

```rust
// File: metanode/meta-consensus/core/src/system_transaction_provider.rs

impl SystemTransactionProvider for DefaultSystemTransactionProvider {
    fn get_system_transactions(&self, current_epoch: Epoch, current_commit_index: u32) 
        -> Option<Vec<SystemTransaction>> 
    {
        // Điều kiện trigger epoch change
        let elapsed_seconds = (now_ms - epoch_start_timestamp_ms) / 1000;
        
        if elapsed_seconds >= self.epoch_duration_seconds {
            // 🎯 EPOCH CHANGE TRIGGERED!
            let system_tx = SystemTransaction::end_of_epoch(
                current_epoch + 1,                              // new_epoch
                epoch_start_timestamp_ms + (epoch_duration * 1000), // deterministic timestamp
                current_commit_index + 50,                      // buffer commits
            );
            return Some(vec![system_tx]);
        }
        None
    }
}
```

**Điều kiện trigger:**
- `time_elapsed >= epoch_duration_seconds` (cấu hình trong genesis)
- Transaction được đưa vào block bởi Leader

---

## 3. Validator Phát Hiện Epoch Kết Thúc

### 3.1. Trong CommitProcessor

```rust
// File: metanode/src/consensus/commit_processor.rs

// Khi nhận CommittedSubDag từ consensus
if let Some((_block_ref, system_tx)) = subdag.extract_end_of_epoch_transaction() {
    if let Some((new_epoch, new_epoch_timestamp_ms, _commit_index)) = system_tx.as_end_of_epoch() {
        // 🎯 EPOCH BOUNDARY = global_exec_index của commit này
        info!("🎯 EndOfEpoch detected: global_exec_index={}, new_epoch={}", 
            global_exec_index, new_epoch);
        
        // Gọi callback để advance epoch
        epoch_transition_callback(new_epoch, new_epoch_timestamp_ms, global_exec_index);
    }
}
```

### 3.2. extract_end_of_epoch_transaction()

```rust
// File: metanode/meta-consensus/core/src/commit.rs

impl CommittedSubDag {
    pub fn extract_end_of_epoch_transaction(&self) -> Option<(BlockRef, SystemTransaction)> {
        for block in &self.blocks {
            for tx in block.transactions() {
                if let Some(system_tx) = extract_system_transaction(tx.data()) {
                    if system_tx.is_end_of_epoch() {
                        return Some((block.reference(), system_tx));
                    }
                }
            }
        }
        None
    }
}
```

---

## 4. Full Node (SyncOnly) Phát Hiện Epoch Thay Đổi

### 4.1. Epoch Monitor (Polling)

```rust
// File: metanode/src/node/epoch_monitor.rs

loop {
    // 1. Poll Go Master định kỳ (cấu hình qua epoch_monitor_poll_interval_secs, mặc định 10s)
    let go_epoch = client.get_current_epoch().await?;
    // Cũng query TCP peers để lấy network_epoch
    let network_epoch = query_peer_epochs_network(&peer_rpc_addresses).await?;
    
    // 2. Phát hiện epoch change
    if go_epoch > last_known_epoch {
        info!("🔄 Detected epoch change: {} -> {}", last_known_epoch, go_epoch);
        
        // 3. Lấy epoch boundary data từ Go
        let (epoch, timestamp_ms, boundary_block, validators) = 
            client.get_epoch_boundary_data(go_epoch).await?;
        
        // 4. boundary_block chính là global_exec_index cuối epoch trước
        info!("📊 Epoch boundary: epoch={}, boundary_block={}", epoch, boundary_block);
        
        // 5. Kiểm tra nếu node có trong committee -> transition
        if is_in_committee(&validators, &own_protocol_key) {
            transition_to_validator(epoch, timestamp_ms, boundary_block);
        }
    }
}
```

### 4.2. Go Backend: GetEpochBoundaryData

```go
// File: executor/unix_socket_handler.go

func HandleGetEpochBoundaryDataRequest(request *pb.GetEpochBoundaryDataRequest) (*pb.EpochBoundaryData, error) {
    epoch := request.GetEpoch()
    
    // 1. Lấy boundary block từ map
    boundaryBlock, _ := rh.chainState.GetEpochBoundaryBlock(epoch)
    
    // 2. Lấy validators TẠI boundary block (snapshot)
    validators, _ := rh.GetValidatorsAtBlockInternal(boundaryBlock)
    
    return &pb.EpochBoundaryData{
        Epoch:         epoch,
        BoundaryBlock: boundaryBlock, // 👈 Đây là global_exec_index cuối epoch trước
        Validators:    validators,
    }, nil
}
```

---

## 5. Công Thức Xác Định (QUAN TRỌNG)

| Khái niệm | Công thức | Giải thích |
|-----------|-----------|------------|
| **Epoch N kết thúc tại** | `global_exec_index` của commit chứa `EndOfEpoch(new_epoch=N+1)` | Block cuối cùng của epoch N |
| **Epoch N+1 bắt đầu từ** | `boundary_block + 1` | Block đầu tiên của epoch N+1 |
| **Committee cho Epoch N+1** | `GetValidatorsAtBlock(boundary_block)` | Validators tại thời điểm snapshot |

---

## 6. Ví Dụ Timeline

```
Epoch 0 (duration=300s, ~4000 blocks)
┌────────────────────────────────────────────────────────────────────────┐
│ Block 0 → Block 1 → ... → Block 4272 → Block 4273                      │
│                                            ↑                           │
│                                     EndOfEpoch TX                      │
│                              global_exec_index = 4273                  │
│                                   = BOUNDARY BLOCK                     │
└────────────────────────────────────────────────────────────────────────┘
                                            │
                                            ▼
                              GetEpochBoundaryData(epoch=1)
                              → boundary_block = 4273
                              → validators = [node-1, node-2, node-3, node-4]
                                            │
                                            ▼
Epoch 1 (starts from block 4274)
┌────────────────────────────────────────────────────────────────────────┐
│ Block 4274 → Block 4275 → ...                                          │
│ (epoch_base_index = 4273)                                              │
│ (global_exec_index = 4273 + commit_index)                              │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Kiểm Tra Trong Log

### 7.1. Validator Node Log

```
# Khi EndOfEpoch được tạo
📝 SystemTransactionProvider: Creating EndOfEpoch transaction - epoch 0 -> 1

# Khi CommitProcessor phát hiện EndOfEpoch
🎯 [SYSTEM TX] EndOfEpoch transaction detected in commit 4273: epoch 0 -> 1

# Khi epoch transition hoàn tất
✅ [ADVANCE EPOCH] Epoch state updated: old_epoch=0, new_epoch=1, boundary_block=4273
```

### 7.2. Full Node (SyncOnly) Log

```
# Poll phát hiện epoch change
🔄 [EPOCH MONITOR] Detected epoch change: 0 -> 1

# Lấy epoch boundary data
📊 [EPOCH MONITOR] Got epoch boundary data: epoch=1, boundary_block=4273, validators=4

# Nếu node trong committee
🚀 [EPOCH MONITOR] Node is in committee! Transitioning to Validator mode...
```

### 7.3. Go Master Log

```
# Khi AdvanceEpoch được gọi
🔄 [ADVANCE EPOCH] Starting epoch advancement: current_epoch=0, new_epoch=1, boundary_block=4273
📝 [ADVANCE EPOCH] Stored epoch boundary block: epoch=1, boundary_block=4273
✅ [ADVANCE EPOCH] Epoch advancement completed successfully
```

---

## 8. File Quan Trọng

| File | Vai trò |
|------|---------|
| `meta-consensus/core/src/system_transaction.rs` | Định nghĩa `SystemTransaction::EndOfEpoch` |
| `meta-consensus/core/src/system_transaction_provider.rs` | Tạo EndOfEpoch khi hết epoch duration |
| `meta-consensus/core/src/commit.rs` | `extract_end_of_epoch_transaction()` |
| `src/consensus/commit_processor.rs` | Xử lý commit, phát hiện EndOfEpoch |
| `src/node/epoch_monitor.rs` | Unified monitor cho tất cả node types |
| `src/node/epoch_transition_manager.rs` | Quản lý transition state |
| `executor/unix_socket_handler_epoch.go` | `GetEpochBoundaryData()` |
| `pkg/blockchain/chain_state.go` | `AdvanceEpochWithBoundary()`, `GetEpochBoundaryBlock()` |

---

## 9. Tóm Tắt

1. **Epoch kết thúc khi**: `elapsed_time >= epoch_duration` → `EndOfEpoch` SystemTransaction được tạo

2. **Block cuối epoch**: `global_exec_index` của commit chứa `EndOfEpoch` transaction

3. **Validator phát hiện**: Qua `extract_end_of_epoch_transaction()` trong `CommitProcessor`

4. **Full Node phát hiện**: Qua polling `get_current_epoch()` và `get_epoch_boundary_data()` từ Go Master

5. **Lấy committee**: `GetValidatorsAtBlock(boundary_block)` - snapshot tại đúng block cuối epoch
