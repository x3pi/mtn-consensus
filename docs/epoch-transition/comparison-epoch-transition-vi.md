# So Sánh Kích Hoạt Chuyển Đổi Epoch: SyncOnly vs Validator

Tài liệu này phân tích sự khác biệt về cơ chế và quy trình kích hoạt chuyển đổi Epoch (Epoch Transition) giữa hai chế độ hoạt động của node: **SyncOnly (Full Node)** và **Validator (Consensus Node)**.

## 1. Bảng So Sánh Tổng Quan

| Đặc Điểm | Chế Độ Validator | Chế Độ SyncOnly (Full Node) |
|:---|:---|:---|
| **Vai Trò** | **Người Quyết Định (Decider)**: Tham gia tạo ra việc chuyển epoch. | **Người Quan Sát (Observer)**: Chờ đợi và đi theo network. |
| **Nguồn Sự Thật** | **Consensus Stream**: Dựa trên chuỗi khối block đã commit mà chính nó tham gia xác thực. | **Execution Layer (Go Master)**: Dựa trên dữ liệu đã đồng bộ từ peers về Go. |
| **Cơ Chế Kích Hoạt** | **Event-Driven (Push)**: `CommitProcessor` gặp `EndOfEpoch` transaction. | **Polling (Pull)**: `EpochMonitor` định kỳ hỏi trạng thái từ Go. |
| **Tính Thời Điểm** | **Tức Thời (Synchronous)**: Chuyển đổi ngay khi block chứa lệnh chuyển epoch được commit. | **Có Độ Trễ (Asynchronous)**: Chuyển đổi sau khi Go đã sync xong và Monitor phát hiện thay đổi. |
| **Yêu Cầu An Toàn** | **Quorum Checks**: Cần 2f+1 phiếu bầu để tạo EndOfEpoch. | **Wait Barriers**: Cần đợi Go Master đồng bộ hoàn toàn với Peers để tránh fork. |

---

## 2. Chi Tiết Cơ Chế: Validator Mode

Trong chế độ Validator, việc chuyển đổi epoch là một phần của giao thức đồng thuận (Consensus Protocol).

### Quy Trình Kích Hoạt:
1.  **Commit Rule**: Hệ thống BFT đạt đến giới hạn của epoch hiện tại (ví dụ: đủ số lượng blocks hoặc commits).
2.  **System Transaction**: Leader tạo ra một giao dịch hệ thống đặc biệt `AdvanceEpoch`.
3.  **Consensus**: Giao dịch này được đưa vào block và được commit bởi toàn bộ mạng lưới.
4.  **Processing**:
    *   `CommitProcessor` của node đọc tuần tự các committed blocks.
    *   Khi gặp `AdvanceEpoch` transaction, nó kích hoạt **Callback**.
5.  **Reconfiguration**:
    *   Callback gọi `transition_to_epoch`.
    *   Node dừng Authority cũ, tạo Authority mới với Committee mới.

> **Đặc điểm chính**: Node Validator **biết chính xác** block nào là block cuối cùng của epoch và chuyển đổi một cách xác định (deterministic) tại đúng index đó.

---

## 3. Chi Tiết Cơ Chế: SyncOnly Mode

Trong chế độ SyncOnly, node không tham gia tạo block nên không biết "khi nào" epoch kết thúc một cách trực tiếp. Nó phải dựa vào việc quan sát.

### Quy Trình Kích Hoạt:
1.  **External Sync**: Go Master (Execution Layer) đồng bộ blocks từ các peers khác trong mạng qua P2P.
2.  **Polling**: Task `Unified EpochMonitor` trong Rust Metanode định kỳ (mặc định 10s, cấu hình qua `epoch_monitor_poll_interval_secs`) gọi API `get_current_epoch()` sang Go và query các TCP peers.
3.  **Detection**:
    *   Monitor phát hiện `go_epoch > current_rust_epoch`.
    *   Điều này có nghĩa là mạng lưới đã đi sang epoch mới.
4.  **Safety Barrier (Quan Trọng)**:
    *   Node kiểm tra xem Go Master đã thực sự tải hết block của epoch cũ chưa.
    *   Sử dụng `EpochTransitionManager` để đảm bảo chỉ có 1 transition chạy tại một thời điểm.
5.  **Local Transition**:
    *   Nếu node phát hiện mình có tên trong Committee mới, nó gọi `transition_to_epoch_from_system_tx()`.
    *   Nếu không, nó chỉ advance Go epoch và tiếp tục sync.

> **Đặc điểm chính**: Node SyncOnly luôn đi sau (lag) mạng lưới một chút. Nó không tự quyết định thời điểm chuyển epoch mà "phản ứng" lại trạng thái của mạng.

---

## 4. Điểm Khác Biệt Critical Về An Toàn (Safety Checks)

### Validator: "No Gap, No Overlap" (Tại Consensus Level)
Validator đảm bảo tính liên tục bằng **Global Execution Index**:
*   Epoch N kết thúc tại index `X`.
*   Epoch N+1 **BẮT BUỘC** bắt đầu tại `X+1`.
*   Nếu không khớp -> **PANIC** ngay lập tức để tránh fork.

### SyncOnly: "Sync Verification Barrier"
Do SyncOnly không có consensus stream liên tục, nó rất dễ bị tình trạng "Gap" (thiếu block) khi chuyển đổi.
*   **Vấn đề**: Go Master báo Epoch 2 đã bắt đầu, nhưng có thể nó chưa tải xong 100 block cuối của Epoch 1.
*   **Giải pháp**: `EpochMonitor` phải thực hiện **Wait Barrier**:
    ```rust
    // Logic trong epoch_monitor.rs
    loop {
        if go_last_block >= peer_last_block {
             break; // Safe to transition
        }
        sleep(); // Wait for sync
    }
    ```
*   Nếu không có barrier này, node SyncOnly chuyển thành Validator sẽ bắt đầu tạo block trên một nền tảng dữ liệu cũ -> **Gây Fork Mạng**.

## 5. Kết Luận

*   Nếu bạn đang debug **Validator**: Hãy nhìn vào `CommitProcessor`, `EpochTransitionManager`, và các `SystemTransaction`. Lỗi thường nằm ở việc không thống nhất được index.
*   Nếu bạn đang debug **SyncOnly/Promotion**: Hãy nhìn vào `Unified EpochMonitor` và `rust_sync_node`. Lỗi thường nằm ở việc Go chưa sync kịp (network lag) hoặc cấu hình `peer_rpc_addresses` không đúng.
