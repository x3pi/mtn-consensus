# MetaNode Consensus Engine

Hệ thống consensus đa node dựa trên **Sui mtn-consensus Consensus Protocol**, tích hợp với Go executor để xử lý transactions và quản lý state.

## 📋 Mục lục

- [Tổng quan](#-tổng-quan)
- [Tính năng chính](#-tính-năng-chính)
- [Kiến trúc hệ thống](#-kiến-trúc-hệ-thống)
- [Quick Start](#-quick-start)
- [Cấu hình](#-cấu-hình)
- [Committee Management](#-committee-management)
- [Transaction Flow](#-transaction-flow)
- [Epoch Management](#-epoch-management)
- [Tài liệu tham khảo](#-tài-liệu-tham-khảo)

---

## 🎯 Tổng quan

**MetaNode Consensus Engine** là một wrapper production-ready trên Sui mtn-consensus consensus protocol, tích hợp với Go executor để:

- ✅ **Multi-node Consensus**: Chạy nhiều consensus nodes với cấu hình dễ dàng
- ✅ **Go Integration**: Tất cả nodes lấy committee từ Go state qua Unix Domain Socket
- ✅ **DAG-based Consensus**: Sử dụng Directed Acyclic Graph để đạt consensus
- ✅ **Byzantine Fault Tolerance**: Chịu được f faulty nodes trong 3f+1 nodes
- ✅ **Epoch Management**: Hỗ trợ epoch transitions với fork-safety
- ✅ **Transaction Queuing**: Queue transactions trong barrier phase để tránh mất giao dịch
- ✅ **Fork-Safety**: Đảm bảo tất cả nodes có cùng state và không fork

## ✨ Tính năng chính

### Consensus Engine
- **mtn-consensus Protocol**: DAG-based consensus với leader election
- **High Throughput**: Xử lý hàng trăm commits/second
- **Low Latency**: End-to-end transaction finalization ~300-600ms
- **Ordered Execution**: Đảm bảo commits được xử lý theo thứ tự

### Go Integration
- **Committee Loading**: Tất cả nodes lấy committee từ Go state qua Unix Domain Socket
- **Genesis Sync**: Script `sync_committee_to_genesis.py` sync committee vào `genesis.json` với `delegator_stakes`
- **Executor Communication**: Node 0 gửi commits đến Go Master qua Unix Domain Socket
- **Validator State**: Go Master quản lý validator state và stake

### Epoch Management
- **Time-based Epochs**: Tự động transition sau một khoảng thời gian
- **Fork-safe Transitions**: Commit index barrier đảm bảo tất cả nodes transition cùng lúc
- **Quorum-based Voting**: 2f+1 votes cần thiết cho epoch change
- **Per-epoch Storage**: Tách biệt consensus DB theo epoch
- **Committee from Go**: Tất cả nodes lấy committee mới từ Go state tại epoch transition

### Transaction Handling
- **Transaction Queuing**: Queue transactions trong barrier phase để xử lý trong epoch tiếp theo
- **Deterministic Ordering**: Sắp xếp transactions theo hash để đảm bảo fork-safety
- **No Transaction Loss**: Đảm bảo không mất transaction trong epoch transition
- **Unix Domain Socket**: Giao tiếp với Go Sub Node qua UDS cho hiệu suất cao

---

## 🏗️ Kiến trúc hệ thống

```
┌─────────────────────────────────────────────────────────────────┐
│                    GO MASTER (Executor)                        │
│  - Quản lý validator state và stake                            │
│  - Xử lý committed blocks từ Rust                             │
│  - Trả về validators qua Unix Domain Socket                     │
└───────────────────────┬─────────────────────────────────────────┘
                        │
                        │ GetValidatorsAtBlockRequest (UDS)
                        │
                        ▼
┌─────────────────────────────────────────────────────────────────┐
│                  RUST CONSENSUS NODES (0, 1, 2, 3)              │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐        │
│  │   Node 0     │  │   Node 1     │  │   Node N     │        │
│  │              │  │              │  │              │        │
│  │ ┌──────────┐ │  │ ┌──────────┐ │  │ ┌──────────┐ │        │
│  │ │   UDS    │ │  │ │   UDS    │ │  │ │   UDS    │ │        │
│  │ │  Server  │ │  │ │  Server  │ │  │ │  Server  │ │        │
│  │ │ (TX)     │ │  │ │ (TX)     │ │  │ │ (TX)     │ │        │
│  │ └────┬─────┘ │  │ └────┬─────┘ │  │ └────┬─────┘ │        │
│  │      │       │  │      │       │  │      │       │        │
│  │ ┌────▼─────┐ │  │ ┌────▼─────┐ │  │ ┌────▼─────┐ │        │
│  │ │Transaction│ │  │ │Transaction│ │  │ │Transaction│ │        │
│  │ │  Client   │ │  │ │  Client   │ │  │ │  Client   │ │        │
│  │ └────┬─────┘ │  │ └────┬─────┘ │  │ └────┬─────┘ │        │
│  │      │       │  │      │       │  │      │       │        │
│  │ ┌────▼─────┐ │  │ ┌────▼─────┐ │  │ ┌────▼─────┐ │        │
│  │ │Consensus │ │  │ │Consensus │ │  │ │Consensus │ │        │
│  │ │Authority │ │  │ │Authority │ │  │ │Authority │ │        │
│  │ └────┬─────┘ │  │ └────┬─────┘ │  │ └────┬─────┘ │        │
│  │      │       │  │      │       │  │      │       │        │
│  │ ┌────▼─────┐ │  │ ┌────▼─────┐ │  │ ┌────▼─────┐ │        │
│  │ │  Commit  │ │  │ │  Commit  │ │  │ │  Commit  │ │        │
│  │ │Processor │ │  │ │Processor │ │  │ │Processor │ │        │
│  │ └────┬─────┘ │  │ └──────────┘ │  │ └──────────┘ │        │
│  │      │       │  │               │  │               │        │
│  │ ┌────▼─────┐ │  │               │  │               │        │
│  │ │Executor  │ │  │               │  │               │        │
│  │ │  Client  │ │  │               │  │               │        │
│  │ │ (UDS)    │ │  │               │  │               │        │
│  │ └────┬─────┘ │  │               │  │               │        │
│  └──────┼───────┘  └───────────────┘  └───────────────┘        │
│         │                                                      │
│         │ CommittedEpochData (UDS)                             │
│         │                                                      │
│         ▼                                                      │
│  ┌──────────────────────────────────────┐                      │
│  │    Sui mtn-consensus Consensus Core      │                      │
│  │  - DAG-based consensus               │                      │
│  │  - Transaction ordering              │                      │
│  │  - Byzantine fault tolerance         │                      │
│  │  - Leader election                   │                      │
│  └──────────────────────────────────────┘                      │
│                                                                 │
│  ┌──────────────────────────────────────────────────────┐      │
│  │              Storage Layer (RocksDB)                 │      │
│  │  - Per-epoch consensus DB                           │      │
│  │  - DAG state                                         │      │
│  │  - Committed blocks                                  │      │
│  │  - Commit history                                    │      │
│  └──────────────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────────────┘
                        ▲
                        │
                        │ Transactions (UDS)
                        │
┌───────────────────────┴─────────────────────────────────────────┐
│                    GO SUB NODE                                  │
│  - Gửi transactions đến Rust qua Unix Domain Socket            │
│  - Nhận receipts từ Go Master                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Các thành phần chính

1. **ConsensusNode** (`src/node.rs`): Wrapper chính quản lý lifecycle của node
2. **ExecutorClient** (`src/executor_client.rs`): Client để giao tiếp với Go Master qua Unix Domain Socket
3. **TxSocketServer** (`src/tx_socket_server.rs`): Unix Domain Socket server để nhận transactions từ Go Sub Node
4. **Commit Processor** (`src/commit_processor.rs`): Xử lý commits theo thứ tự và gửi đến Go Master
5. **Epoch Change Manager** (`src/epoch_change.rs`): Quản lý epoch transitions
6. **Configuration** (`src/config.rs`): Quản lý cấu hình và keypairs

Xem chi tiết trong [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md).

---

## ⚡ Quick Start

### 1. Build project

```bash
cd /home/abc/chain-new/mtn-consensus/metanode
cargo build --release --bin metanode
```

### 2. Chạy full system

Script `run_full_system.sh` sẽ:
- Xóa dữ liệu cũ (sample, logs, storage)
- Tạo committee mới cho 4 nodes
- Sync committee vào `genesis.json` với `delegator_stakes`
- Xóa tất cả `committee_node_*.json` files (tất cả nodes lấy từ Go)
- Khởi động Go Master (init genesis với validators)
- Khởi động Go Sub Node
- Khởi động 4 Rust Consensus Nodes (tất cả lấy committee từ Go)

```bash
./scripts/run_full_system.sh
```

### 3. Xem logs

```bash
# Xem log node 0
tmux attach -t metanode-0

# Hoặc xem log file
tail -f logs/latest/node_0.log

# Xem Go Master log
tmux attach -t go-master

# Xem Go Sub log
tmux attach -t go-sub
```

### 4. Submit transaction

```bash
# Sử dụng Go Sub Node để gửi transaction
# Transaction sẽ được gửi đến Rust qua Unix Domain Socket
# Rust sẽ commit và gửi đến Go Master để execute
```

---

## ⚙️ Cấu hình

### Node Configuration

File `node_X.toml` có cấu trúc:

```toml
# Node identification
node_id = 0
network_address = "127.0.0.1:9000"

# Keypairs
protocol_key_path = "config/node_0_protocol_key.json"
network_key_path = "config/node_0_network_key.json"

# Committee (không còn dùng để load, chỉ để lưu metadata sau epoch transition)
committee_path = "config/committee_node_0.json"
storage_path = "config/storage/node_0"

# Executor (chỉ ảnh hưởng đến việc gửi commits đến Go, không ảnh hưởng đến việc lấy committee)
executor_enabled = true  # Node 0: true, Node 1-3: false

# Metrics
enable_metrics = true
metrics_port = 9100

# Performance tuning
speed_multiplier = 0.2
time_based_epoch_change = true
epoch_duration_seconds = 300
max_clock_drift_seconds = 5
```

### Executor Configuration

**LƯU Ý:** `executor_enabled` chỉ ảnh hưởng đến việc gửi commits đến Go Master:
- `executor_enabled = true`: Node gửi commits đến Go Master qua Unix Domain Socket
- `executor_enabled = false`: Node không gửi commits đến Go Master (consensus only)

**Tất cả nodes đều lấy committee từ Go**, không phụ thuộc vào `executor_enabled`.

Xem chi tiết trong [docs/CONFIGURATION.md](./docs/CONFIGURATION.md).

---

## 👥 Committee Management

### Tất cả Nodes Lấy từ Go State

**CRITICAL:** Tất cả nodes (0, 1, 2, 3) đều lấy committee từ Go state qua Unix Domain Socket:

1. **Startup (Block 0 - Genesis):**
   - Tất cả nodes tạo `ExecutorClient` để kết nối đến Go Master
   - Gọi `GetValidatorsAtBlockRequest(block_number=0)` qua Unix Domain Socket
   - Go Master trả về `ValidatorInfoList` với validators từ genesis state
   - Rust build committee từ validators này

2. **Epoch Transition:**
   - Tất cả nodes lấy committee từ Go state tại `last_global_exec_index` của epoch trước
   - Đảm bảo tất cả nodes có cùng committee từ cùng block number

3. **Unix Domain Socket Paths:**
   - Node 0: `/tmp/rust-go.sock_2` (Go Master)
   - Node 1-3: `/tmp/rust-go.sock_1` (Go Master)

### Genesis Sync

Script `sync_committee_to_genesis.py` sync committee vào `genesis.json`:

1. **Sync Keys:** Sync `hostname`, `authority_key`, `protocol_key`, `network_key` từ committee.json
2. **Create Delegator Stakes:** Tạo `delegator_stakes` từ stake trong committee.json (convert từ normalized về wei)
3. **Update Total Staked Amount:** Cập nhật `total_staked_amount` từ `delegator_stakes`

**Kết quả:**
- `genesis.json` có `delegator_stakes` với amount="1000000000000000000" (1 ETH = 10^18 wei)
- Go Master sẽ init genesis với stake đúng từ `delegator_stakes`
- Go Master sẽ trả về validators với stake đúng (không phải min stake=1)

Xem chi tiết trong [docs/COMMITTEE.md](./docs/COMMITTEE.md).

---

## 📨 Transaction Flow

### Luồng Transaction

```
Go Sub Node
  │
  ├─► ProcessTransactionsInPoolSub()
  ├─► MarshalTransactions(txs)
  ├─► SendTransaction() → Unix Domain Socket (/tmp/metanode-tx-0.sock)
  │
  ▼
Rust Consensus (MetaNode)
  │
  ├─► TxSocketServer nhận transaction
  ├─► Check transaction acceptance (barrier check)
  │   ├─► Nếu barrier set → Queue transaction
  │   └─► Nếu không → Submit vào consensus
  ├─► Consensus Authority xử lý và commit blocks
  ├─► CommitProcessor xử lý commits
  ├─► ExecutorClient gửi commits đến Go Master (nếu executor_enabled=true)
  │
  ▼
Go Master Executor
  │
  ├─► Nhận CommittedEpochData qua Unix Domain Socket
  ├─► Unmarshal transactions
  ├─► ProcessTransactions()
  ├─► Execute transactions và cập nhật state
  └─► Gửi receipts về Go Sub Node
```

### Transaction Queuing

**CRITICAL:** Transactions được queue trong barrier phase để tránh mất giao dịch:

1. **Barrier Phase Detection:**
   - Khi `transition_barrier` được set, tất cả transactions được queue
   - Transactions được queue với reason: "Barrier phase: barrier=X is set"

2. **Queue Processing:**
   - Transactions được lưu trong `pending_transactions_queue`
   - Sau epoch transition, transactions được submit lại
   - Transactions được sắp xếp theo hash để đảm bảo fork-safety

3. **Deterministic Ordering:**
   - Transactions được sort theo hash (lexicographic order)
   - Deduplicate để tránh submit trùng lặp
   - Tất cả nodes submit transactions trong cùng thứ tự

Xem chi tiết trong [docs/TRANSACTION_FLOW.md](./docs/TRANSACTION_FLOW.md).

---

## 🔄 Epoch Management

### Tổng quan

Hệ thống hỗ trợ **epoch transitions** với các tính năng:

- **Time-based Epochs**: Tự động transition sau `epoch_duration_seconds`
- **Fork-safe**: Commit index barrier đảm bảo tất cả nodes transition cùng lúc
- **Quorum Voting**: Cần 2f+1 votes để approve epoch change
- **In-process Restart**: Authority restart trong cùng process
- **Committee from Go**: Tất cả nodes lấy committee mới từ Go state

### Quy trình Epoch Transition

1. **Proposal**: Node nào đó propose epoch change khi thời gian đã hết
2. **Voting**: Các nodes vote cho proposal (auto-vote nếu hợp lệ)
3. **Quorum**: Khi đạt 2f+1 votes, proposal được approve
4. **Commit Index Barrier**: Đợi commit index vượt qua barrier (proposal_commit_index + 10)
5. **Fork-Safety Validations**: 
   - Verify quorum đạt
   - Verify đạt commit index barrier
   - Verify proposal hash consistency
   - Verify timestamp consistency
6. **Transition**: Tất cả nodes transition cùng lúc với cùng `last_commit_index` và `global_exec_index`
7. **Committee Loading**: Tất cả nodes lấy committee mới từ Go state tại `last_global_exec_index`
8. **Restart**: Authority restart với epoch mới và consensus DB mới
9. **Submit Queued Transactions**: Submit transactions đã queue trong barrier phase

Xem chi tiết trong:
- [docs/EPOCH.md](./docs/EPOCH.md) - Epoch và cách triển khai
- [docs/EPOCH_PRODUCTION.md](./docs/EPOCH_PRODUCTION.md) - Best practices cho production
- [docs/FORK_SAFETY.md](./docs/FORK_SAFETY.md) - Fork-safety mechanisms

---

## 📚 Tài liệu tham khảo

### Tài liệu MetaNode

Xem thêm tài liệu chi tiết trong thư mục [docs/](./docs/):

#### Tài liệu kỹ thuật
- [docs/README.md](./docs/README.md) - Mục lục và tổng quan tài liệu
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) - Kiến trúc hệ thống và các thành phần
- [docs/CONSENSUS.md](./docs/CONSENSUS.md) - Cơ chế consensus và DAG
- [docs/TRANSACTION_FLOW.md](./docs/TRANSACTION_FLOW.md) - Luồng transaction từ Go Sub Node → Rust Consensus → Go Master
- [docs/COMMITTEE.md](./docs/COMMITTEE.md) - Committee management và Go integration
- [docs/EPOCH.md](./docs/EPOCH.md) - Epoch và cách triển khai epoch transition
- [docs/FORK_SAFETY.md](./docs/FORK_SAFETY.md) - Fork-safety mechanisms

#### Hướng dẫn sử dụng
- [docs/CONFIGURATION.md](./docs/CONFIGURATION.md) - Cấu hình hệ thống
- [docs/DEPLOYMENT.md](./docs/DEPLOYMENT.md) - Triển khai và vận hành
- [docs/TROUBLESHOOTING.md](./docs/TROUBLESHOOTING.md) - Xử lý sự cố và debugging
- [docs/FAQ.md](./docs/FAQ.md) - Câu hỏi thường gặp

#### Scripts và Tools
- [scripts/README.md](./scripts/README.md) - Hướng dẫn sử dụng các script tiện ích
- [scripts/run_full_system.sh](./scripts/run_full_system.sh) - Script chạy full system

### Tài liệu Tham khảo

- [Sui Documentation](https://docs.sui.io/)
- [mtn-consensus Consensus Paper](https://arxiv.org/pdf/2310.14821)
- [Sui GitHub Repository](https://github.com/MystenLabs/sui)

---

## 📝 License

Apache 2.0 - Giống như Sui

---

**MetaNode Consensus Engine** - Production-ready consensus engine dựa trên Sui mtn-consensus Protocol với Go executor integration.
