# MetaNode Client

Client để gửi transactions lên MetaNode Consensus Engine qua RPC endpoint.

## Cài đặt

```bash
cd /home/abc/chain-n/mtn-consensus/client
cargo build --release
```

## Sử dụng

### Submit transaction qua RPC

**Submit với text data:**
```bash
./target/release/metanode-client submit \
    --endpoint http://127.0.0.1:10100 \
    --data "Hello, Blockchain!"
```

**Submit với hex data:**
```bash
./target/release/metanode-client submit \
    --endpoint http://127.0.0.1:10100 \
    --data "48656c6c6f2c20426c6f636b636861696e21"
```

**Submit từ file:**
```bash
echo "Transaction data" > tx.txt
./target/release/metanode-client submit \
    --endpoint http://127.0.0.1:10100 \
    --file tx.txt
```

### Các tham số

- `--endpoint, -e`: RPC endpoint của node (mặc định: `http://127.0.0.1:10100`)
- `--data, -d`: Dữ liệu transaction dạng text hoặc hex
- `--file, -f`: Đường dẫn đến file chứa transaction data

**Lưu ý:** Phải cung cấp một trong hai: `--data` hoặc `--file`

## RPC Endpoint

Mỗi node expose một RPC endpoint để submit transactions:
- **Node 0**: `http://127.0.0.1:10100` (metrics_port 9100 + 1000)
- **Node 1**: `http://127.0.0.1:10101` (metrics_port 9101 + 1000)
- **Node 2**: `http://127.0.0.1:10102` (metrics_port 9102 + 1000)
- **Node 3**: `http://127.0.0.1:10103` (metrics_port 9103 + 1000)

## Ví dụ

### Ví dụ 1: Submit transaction đơn giản

```bash
# Đảm bảo node đang chạy
cd ../metanode
./run_nodes.sh

# Trong terminal khác, submit transaction
cd ../client
./target/release/metanode-client submit \
    --endpoint http://127.0.0.1:10100 \
    --data "My first transaction"
```

Output:
```json
✅ Transaction submitted successfully!
{
  "success": true,
  "block_ref": "B123([0],...)",
  "indices": [0]
}
```

### Ví dụ 2: Submit từ file JSON

```bash
# Tạo file transaction
cat > transaction.json <<EOF
{
  "from": "0x123",
  "to": "0x456",
  "amount": 100
}
EOF

# Submit
./target/release/metanode-client submit \
    --endpoint http://127.0.0.1:10100 \
    --file transaction.json
```

### Ví dụ 3: Submit binary data

```bash
# Submit binary file
./target/release/metanode-client submit \
    --endpoint http://127.0.0.1:10100 \
    --file image.png
```

## Kiến trúc

Client hoạt động như sau:

1. **Kết nối**: Client gửi HTTP POST request đến RPC endpoint của node
2. **Submit**: Node nhận transaction và submit vào TransactionClient
3. **Response**: Node trả về block reference và transaction indices

## Transaction Format

MetaNode hiện tại chấp nhận transaction dưới dạng `Vec<u8>` (raw bytes). Bạn có thể:

- Gửi text: `--data "text"`
- Gửi hex: `--data "48656c6c6f"`
- Gửi file: `--file path/to/file`

Transaction sẽ được serialize và gửi đến consensus engine.

## Giới hạn

- **Max transaction size**: Được định nghĩa trong ProtocolConfig
- **Max transactions per block**: Được định nghĩa trong ProtocolConfig
- **Max block size**: Được định nghĩa trong ProtocolConfig

Nếu transaction vượt quá giới hạn, sẽ nhận lỗi khi submit.

## Troubleshooting

### Lỗi: "Failed to send request"

- Kiểm tra node đang chạy: `ps aux | grep metanode`
- Kiểm tra RPC endpoint có đúng không
- Kiểm tra firewall settings

### Lỗi: "Server error"

- Kiểm tra logs của node: `tail -f ../metanode/logs/node_0.log`
- Kiểm tra transaction size có vượt quá limit không

### Lỗi: "Either --data or --file must be provided"

- Phải cung cấp một trong hai: `--data` hoặc `--file`

### Transaction không được commit

- Kiểm tra logs của node: `tail -f ../metanode/logs/node_0.log`
- Tìm "Received commit" để xem transaction đã được commit chưa
- Transaction có thể bị reject nếu không pass validation

## Xem thêm

- [../metanode/Readme.md](../metanode/Readme.md) - Tài liệu MetaNode
- [../docs/metanode/DEPLOYMENT.md](../docs/metanode/DEPLOYMENT.md) - Hướng dẫn triển khai nodes
