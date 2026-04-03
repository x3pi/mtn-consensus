# Hướng dẫn triển khai MetaNode phân tán (Multi-Node Deployment)

Tài liệu này hướng dẫn cách đưa hệ thống MetaNode từ môi trường chạy cục bộ (localhost) lên cụm máy chủ gồm nhiều Node khác nhau (ví dụ: Node 0, Node 1, Node 2, Node 3).

## 1. Kiến trúc phân tán (Distributed Architecture)
Mỗi máy chủ (Validator Node) sẽ chạy đồng thời 2 tiến trình:
1.  **Rust Process (metanode):** Xử lý đồng thuận (Consensus).
2.  **Go Process (simple_chain):** Xử lý thực thi giao dịch (Execution) và duy trì trạng thái (State).

- **Giao tiếp nội bộ (Local):** Dùng Unix Domain Sockets (UDS) để đạt hiệu năng cao nhất.
- **Giao tiếp liên máy (Cross-machine):** Dùng TCP (cho Consensus P2P và Peer Discovery).

---

## 2. Chuẩn bị hệ thống (Prerequisites)
Trước khi triển khai, hãy đảm bảo tất cả các máy chủ thỏa mãn các điều kiện sau:

### 2.1. Đồng bộ thời gian (NTP/Chrony)
**CỰC KỲ QUAN TRỌNG:** Thuật toán đồng thuận dựa trên timestamps. Nếu thời gian giữa các máy lệch > 1 giây, hệ thống có thể bị treo (halt).
```bash
sudo apt update && sudo apt install chrony -y
sudo systemctl enable --now chrony
# Kiểm tra trạng thái đồng bộ
chronyc tracking
```

### 2.2. Cấu hình Firewall (UFW)
Mở các cổng cần thiết để các node có thể nói chuyện với nhau:
```bash
# Consensus P2P (giữa các node Rust)
sudo ufw allow 9000:9005/tcp  
# Peer Discovery (RPC giữa Rust máy này và Go máy kia)
sudo ufw allow 19000:19005/tcp 
# Go P2P Communication
sudo ufw allow 4000:4300/tcp  
# RPC Client (JSON-RPC cho người dùng/ví)
sudo ufw allow 8545/tcp       
```

---

## 3. Bước 1: Cấu hình địa chỉ IP tự động
Thay vì sửa thủ công hàng chục file cấu hình, hãy sử dụng script tự động hóa.

1.  Tại máy quản lý source code, chuyển đến thư mục scripts:
    ```bash
    cd mtn-consensus/metanode/scripts/node
    ```
2.  Chạy script cập nhật IP (thay thế bằng danh sách IP thật của cụm máy chủ):
    ```bash
    ./update_ips.sh 192.168.1.100 192.168.1.101 192.168.1.102 192.168.1.103
    ```
    *Lưu ý: Script này sẽ tự động sửa IP trong `node_X.toml`, `config-master-nodeX.json`, và `genesis.json`.*

---

## 4. Bước 2: Build Binary
Build binary một lần trên máy có cấu hình mạnh nhất, sau đó phân phối sang các máy khác.

1.  **Build Go (Execution Engine):**
    ```bash
    cd mtn-simple-2025
    bash build.sh linux
    ```
2.  **Build Rust (Consensus Engine):**
    ```bash
    cd mtn-consensus/metanode
    cargo build --release
    ```

---

## 5. Bước 3: Phân phối dữ liệu (Deployment)
Sử dụng `scp` hoặc `rsync` để copy binary và config sang từng máy. 

**Ví dụ copy cho Node 1 (IP 192.168.1.101):**
```bash
# Tạo thư mục trên máy đích
ssh user@192.168.1.101 "mkdir -p ~/metanode/config"

# Copy binary
scp mtn-simple-2025/cmd/simple_chain/simple_chain user@192.168.1.101:~/metanode/
scp mtn-consensus/metanode/target/release/metanode user@192.168.1.101:~/metanode/

# Copy config tương ứng (Quan trọng: chỉ copy file config của node đó)
scp mtn-consensus/metanode/config/node_1.toml user@192.168.1.101:~/metanode/config/
scp mtn-simple-2025/cmd/simple_chain/config-master-node1.json user@192.168.1.101:~/metanode/
scp mtn-simple-2025/cmd/simple_chain/genesis.json user@192.168.1.101:~/metanode/
```

---

## 6. Bước 4: Khởi chạy cụm Node (Startup Order)
Tuân thủ thứ tự khởi chạy để các tiến trình tìm thấy nhau chính xác.

### Tại mỗi máy chủ (Thực hiện tuần tự):
1.  **Bước 1: Chạy Go Master:**
    Go Master cần được chạy trước để mở cổng socket chờ Rust Consensus kết nối.
    ```bash
    cd ~/metanode
    ./simple_chain -config=config-master-nodeN.json
    ```
    *(Thay N bằng ID của node tương ứng trên máy đó)*

2.  **Bước 2: Chạy Rust Consensus:**
    Mở một terminal mới (hoặc dùng `screen`/`tmux`):
    ```bash
    cd ~/metanode
    ./metanode start --config config/node_N.toml
    ```

---

## 7. Giám sát hệ thống (Monitoring)
Hãy kiểm tra log để đảm bảo các node đã "bắt tay" thành công qua mạng:

- **Kiểm tra Peer (Rust):**
  ```bash
  grep "Connected peers" rust.log
  ```
  Số lượng peers phải bằng `(Tổng số node - 1)`.

- **Kiểm tra Commit (Go):**
  ```bash
  tail -f master.log | grep "Block committed"
  ```
  Nếu thấy log block được commit liên tục, có nghĩa là cụm node đã đạt được đồng thuận qua mạng TCP thành công.

---

## Appendix: Khai báo Port mặc định
| Thành phần | Giao thức | Port | Mô tả |
| :--- | :--- | :--- | :--- |
| Rust P2P | TCP | 9000-9004 | Kết nối đồng thuận giữa các node |
| Peer RPC | TCP | 19000-19004 | Rust query thông tin từ Go ở máy khác |
| Go P2P | TCP | 4000, 4100... | Kết nối đồng bộ dữ liệu giữa các Go node |
| Rust RPC | TCP | 10100-10104 | RPC nội bộ giữa Go và Rust |
