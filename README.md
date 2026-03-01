# mtn-consensus Metanode

mtn-consensus là một high-performance consensus engine được viết bằng Rust, tích hợp với execution engine Go (Simple Chain) để tạo thành một blockchain hoàn chỉnh.

## Kiến Trúc Tổng Quan

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│   Rust Nodes    │     │   Go Execution   │     │   State DB      │
│   (Consensus)   │◄───►│   Engine         │◄───►│   (RocksDB)     │
│                 │     │   (Simple Chain) │     │                 │
└─────────────────┘     └──────────────────┘     └─────────────────┘
         │                           │                     │
         │ Send Committed Blocks     │                     │
         │ ─────────────────────────►│                     │
         │ executor{N}.sock          │                     │
         │                           │                     │
         │ Request State Info        │                     │
         │◄──────────────────────────│                     │
         │ rust-go.sock_{N+1}        │                     │
```

## Thành Phần Chính

### Consensus Engine (Rust)
- **mtn-consensus DAG**: High-throughput consensus algorithm
- **Multi-node**: Hỗ trợ 4 nodes với Byzantine fault tolerance
- **Epoch Management**: Tự động chuyển đổi epoch dựa trên thời gian
- **Network Layer**: gRPC-based peer-to-peer communication

### Execution Engine (Go)
- **Simple Chain**: Transaction processing và state management
- **Master-Sub Architecture**: 1 master + 3 sub nodes
- **State Synchronization**: Sync state giữa các sub nodes
- **Unix Socket API**: Giao tiếp với consensus engine

## Giao Tiếp Giữa Components

Chi tiết về các luồng giao tiếp, socket paths và cấu hình được mô tả trong [COMMUNICATION_PROTOCOLS.md](./COMMUNICATION_PROTOCOLS.md).

### Socket Paths Chính

| Component | Send Socket | Receive Socket | Mục đích |
|-----------|-------------|----------------|----------|
| **Node 0** | `/tmp/executor0.sock` | `/tmp/rust-go.sock_1` | Gửi blocks, đọc state |
| **Go Master** | `/tmp/rust-go.sock_1` | `/tmp/executor0.sock` | Nhận blocks, gửi state |

## Quick Start

### 1. Build và Setup

```bash
# Build Rust consensus engine
cd metanode
cargo build --release

# Build Go execution engine
cd ../../mtn-simple-2025
go build -o mtn-simple ./cmd/simple_chain
```

### 2. Cấu Hình

```bash
# Tạo cấu hình cho single node testing
cd metanode
./target/release/metanode generate --single-node

# Hoặc cấu hình multi-node
./target/release/metanode generate --multi-node
```

### 3. Chạy Nodes

```bash
# Terminal 1: Go Master Node
cd ../../mtn-simple-2025
./mtn-simple --config cmd/simple_chain/config_sv/config-master.json

# Terminal 2: Rust Node 0
cd metanode
./target/release/metanode start --config config_single/node_0.toml
```

## Cấu Hình Environment

### Development Mode
```bash
export LOCALHOST_TESTING=1  # Bypass network health checks
export SINGLE_NODE=1        # Force single-node consensus
```

### Production Mode
- Đảm bảo tất cả 4 nodes chạy
- Network connectivity giữa các nodes
- Shared storage cho epoch data

## Monitoring

### Logs
- **Consensus Logs**: `metanode/logs/latest/node_*.log`
- **Execution Logs**: Xem trong Go application logs
- **Epoch Logs**: `metanode/logs/latest/node_*.epoch.log`

### Metrics
- Prometheus metrics trên port 9090 (nếu enable)
- Health checks qua HTTP endpoints

## Troubleshooting

### Common Issues

1. **Epoch không chuyển đổi**
   - Kiểm tra network connectivity giữa nodes
   - Đảm bảo quorum đạt được (2f+1 votes)
   - Check `LOCALHOST_TESTING=1` cho local dev

2. **Socket connection errors**
   - Xóa socket files cũ: `rm /tmp/executor*.sock /tmp/rust-go.sock_*`
   - Đảm bảo Go master chạy trước Rust nodes
   - Check socket permissions

3. **Buffer overflow**
   - Tăng resources cho Go execution engine
   - Giảm consensus block production rate
   - Check backpressure mechanism hoạt động

### Debug Commands

```bash
# Check socket connections
ls -la /tmp/executor*.sock /tmp/rust-go.sock_*

# Monitor consensus state
tail -f metanode/logs/latest/node_0.log | grep EPOCH

# Check Go state
curl http://localhost:8080/health  # Nếu có health endpoint
```

## Architecture Details

### Consensus Algorithm
- **Naru+mtn-consensus**: DAG-based consensus
- **Leaderless**: Tất cả nodes đều có thể propose blocks
- **Byzantine Fault Tolerant**: Chịu được f/3 faulty nodes

### Epoch Management
- **Time-based**: Chuyển epoch sau N giây (mặc định 180s)
- **State Persistence**: Epoch data lưu trong RocksDB
- **Fork Safety**: Đảm bảo không có fork khi chuyển epoch

### Execution Model
- **Sequential Processing**: Blocks được execute theo thứ tự
- **State Commitments**: Atomic state updates
- **Validator Management**: Dynamic validator set

## Contributing

### Development Setup
1. Clone repository
2. Setup Rust toolchain (1.70+)
3. Setup Go (1.21+)
4. Build both components
5. Run tests: `cargo test` và `go test`

### Code Organization
- `metanode/src/`: Rust consensus engine
- `metanode/meta-consensus/`: Core consensus library
- `../mtn-simple-2025/`: Go execution engine

## License

Copyright 2024 Chain-N Project. All rights reserved.
