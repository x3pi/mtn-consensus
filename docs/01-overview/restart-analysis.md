# Phân Tích Cơ Chế "No Fork" & An Toàn Dữ Liệu Khi Restart

Tài liệu này chi tiết hóa các cơ chế bảo vệ (protection mechanisms) đã được triển khai để đảm bảo **tuyệt đối không xảy ra Fork** và **không mất dữ liệu** khi Node 1 (Rust Validator) khởi động lại.

## 1. Nguyên Tắc Cốt Lõi: Go Master Là "Source of Truth"

Để tránh bất đồng bộ giữa lớp đồng thuận (Rust) và lớp thực thi (Go), hệ thống tuân thủ nguyên tắc:
*   **Go Master Authoritative:** Rust không bao giờ tự ý quyết định block number tiếp theo. Khi khởi động, Rust phải hỏi Go: *"Anh đã thực thi đến block nào?"* và bắt đầu từ `last_block + 1`.
*   **Không Reset Go Độc Lập:** Nếu Go Master còn giữ dữ liệu (block 1000), Rust bắt buộc phải tuân theo. Nếu Rust cố gửi block 500, Go sẽ từ chối hoặc buffer vĩnh viễn (nhưng hiện đã có cơ chế fix).

## 2. Các Cơ Chế Bảo Vệ "No Fork" Đã Triển Khai

Chúng tôi đã triển khai 4 lớp bảo vệ (defense layers) để triệt tiêu mọi rủi ro fork:

### Lớp 1: Timestamp & Genesis Synchronization (Chống lỗi "Ancestor Not Found")
*   **Vấn đề cũ:** Khi restart, Rust có thể lấy thời gian hiện tại (`SystemTime::now()`) làm `epoch_timestamp`. Nếu thời gian này lệch với Genesis Block của mạng, Hash của Genesis Block sẽ khác nhau -> **Fork ngay từ block 0**.
*   **Giải pháp (Đã Fix):**
    *   Trong `node/mod.rs`, khi khởi động, Rust **gọi RPC sang Go Master** (hoặc Peer nếu cần) để lấy chính xác `epoch_timestamp_ms`.
    *   Rust sử dụng timestamp này để khởi tạo `ConsensusAuthority`.
    *   **Kết quả:** Genesis Hash của Node 1 luôn khớp 100% với toàn mạng. **Fork bị ngăn chặn từ trứng nước.**

### Lớp 2: Strict Sequential Recovery (Chống lỗi "Go Buffering Forever")
*   **Vấn đề cũ:** Rust có thể quét DB và gửi block không theo thứ tự (ví dụ gửi 1002 trước 1001), hoặc bỏ sót block (gap). Go Master (với logic strict) sẽ buffer block 1002 và chờ 1001 mãi mãi -> **Hệ thống treo (Stall).**
*   **Giải pháp (Đã Fix trong `recovery.rs`):**
    *   **Con trỏ tuần tự (`next_required_global`):** Rust duy trì một biến đếm bắt đầu *chính xác* từ `go_last_block + 1`.
    *   **Kiểm tra Gap:** Trước khi gửi bất kỳ block nào, Rust kiểm tra:
        *   Nếu `block_index < next`: Bỏ qua (đã gửi rồi).
        *   Nếu `block_index == next`: Gửi và tăng biến đếm.
        *   Nếu `block_index > next`: **BÁO LỖI NGAY LẬP TỨC (Panic/Error).**
    *   **Kết quả:** Rust thà crash và báo lỗi để admin xử lý (restore backup) còn hơn là gửi block nhảy cóc khiến Go treo không rõ nguyên nhân. Đảm bảo dòng dữ liệu sang Go luôn liền mạch (contiguous).

### Lớp 3: Persistence `last_sent_index` (Chống lỗi "Future Block")
*   **Vấn đề cũ:** Nếu Rust crash sau khi gửi block 1100 nhưng chưa kịp cập nhật bộ nhớ, khi bật lại nó tưởng mới gửi 1099 và gửi lại 1100. Hoặc tệ hơn, tính sai index thành 1200.
*   **Giải pháp (Đã Fix):**
    *   Mỗi khi gửi thành công sang Go, Rust ghi `last_sent_index` xuống đĩa (`executor_state/last_sent_index.bin`).
    *   Khi bật lại, Rust đọc giá trị này để biết chính xác mình đã gửi đến đâu, kết hợp với việc hỏi lại Go Master để "double-check".

### Lớp 4: Fork Detection (Phát hiện sớm sự cố)
*   Trong quá trình chạy, `executor_client` liên tục so sánh `next_expected_index` của mình với `last_block_number` của Go.
*   Nếu phát hiện Go đang ở block thấp hơn block Rust đã gửi quá xa (Lag) hoặc Go bỗng nhiên *giảm* block number (Reorg/Reset bất thường), Rust sẽ cảnh báo `🚨 [FORK DETECTED]` để dừng hoạt động kịp thời.

## 3. Kịch Bản Restart (Walkthrough Check)

Khi bạn chạy `restart_node_1.sh`, hệ thống sẽ tuần tự thực hiện:

1.  **Stop:** Rust tắt. Go có thể vẫn chạy.
2.  **Start:** Rust bật lại.
3.  **Sync Time:** Rust hỏi Go lấy `epoch_timestamp_ms` -> **Genesis Hash khớp.**
4.  **Sync Index:** Rust hỏi Go lấy `last_block_number` (ví dụ 5000).
5.  **Recovery Check:**
    *   Rust đặt `next = 5001`.
    *   Rust quét DB cục bộ.
    *   Tìm thấy block 5001 -> Gửi -> `next = 5002`.
    *   Tìm thấy block 5002 -> Gửi -> `next = 5003`.
    *   ... Gửi hết đến block mới nhất trong DB (ví dụ 5010).
6.  **Join Network:** Sau khi replay đến 5010, Rust vào mạng và sync tiếp từ 5011+.

## 4. Kết Luận

Hệ thống hiện tại đã đạt chuẩn **Fork Safety**.
*   Không còn rủi ro sai Genesis Hash.
*   Không còn rủi ro gửi block nhảy cóc (Gaps).
*   Không còn rủi ro Go bị treo do Buffering.

Bạn có thể restart node an toàn bất cứ lúc nào, miễn là **không xóa folder DB** (`consensus_db` và `executor_state`).
