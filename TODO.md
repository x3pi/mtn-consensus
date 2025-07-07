- Xử lý việc đồng bộ giữa các validator

- Cải thiện giao thức broadcast dùng reedsolomon thêm 1 ít dữ liệu sử lỗi vào và chia dữ liệu thành n phần chỉ cần n - k phần thì có thể phục hồi lại giữ liệu gốc giảm dung lượng truyền tải mỗi validator chỉ gửi 1 phần chỉ chỉ cần ít hơn k node không lỗi thì dữ liệu sẽ phục hồi lại được

- Hiện tại đang thực hiện đồng thuận theo `https://alea-bft.org/#code` dự kiến sẽ cải thiện hơn sau tham khảo `https://arxiv.org/pdf/2504.12766` ví dụ như áp dụng kết thúc sớm cho những block có thể  thực thi ngay sau GBC (Graded Broadcast) không chờ vote (Asynchronous Binary Agreement) nữa mà có thể thực thi luôn

- Hiện tại có hai loại tài khoản cần chữ ký bls và cần cả hai chữ ký bls và secp. Giờ bổ sung thêm loại tài khoản validator và stake dùng cho quá trình đồng thuận. 1 validator có thể có nhiều stake tham gia. 1 validator có tổng số stake đặt cược càng lớn tỷ lệ được chọn tham gia vote tạo block sẽ càng cao. 1 validator có thể đặt tỉ lệ chia phần thưởng khối ví dụ sẽ là 10% cho validator và còn lại 90% sẽ là cho stake tham gia đặt cược

- Thêm kỷ nguyên ví dụ sẽ 500000 block sẽ chọn lại nhóm validator để vote