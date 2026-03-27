# Meta Node (Consensus) 🚀

This repository contains the source code and documentation for the `mtn-consensus` blockchain module.

---

## 📚 Hướng Dẫn Xem Tài Liệu (How to View Documentation)

Toàn bộ tài liệu liên quan đến thuật toán đồng thuận (Consensus), chuyển giao kỷ nguyên (Epoch Transition), và thiết kế chống Phân nhánh (Fork Prevention) đã được tổ chức lại bằng hệ thống **MkDocs Material** chuyên nghiệp. 

The technical documentation is built using MkDocs. Follow the instructions below to serve the documentation website locally.

### 🛠️ Cài Đặt Lần Đầu (First-time Setup)

Do các hệ điều hành Linux mới (Ubuntu 24.04+, Debian 12+) chặn cài đặt thư viện Python toàn cục (quy định PEP 668), bạn cần tạo một môi trường ảo (Virtual Environment) để cài MkDocs:

```bash
# 1. Tạo môi trường ảo có tên là "venv"
python3 -m venv venv

# 2. Kích hoạt môi trường ảo
source venv/bin/activate

# 3. Cài đặt thư viện giao diện MkDocs
pip install mkdocs-material
```

### 🚀 Khởi Chạy Website (Running the Server)

Từ các lần sau, mỗi khi muốn xem tài liệu, bạn chỉ cần mở Terminal tại thư mục này và chạy 2 lệnh sau:

```bash
# 1. Bật môi trường ảo
source venv/bin/activate

# 2. Khởi chạy server tài liệu
mkdocs serve
```

👉 Sau đó, hãy mở trình duyệt web và truy cập vào link: **`http://127.0.0.1:8000`**

---

*Lưu ý: Nếu khi tạo môi trường ảo ở bước cài đặt bị lỗi báo thiếu gói, hãy chạy lệnh `sudo apt install python3-venv` để thiết lập đủ môi trường Python trên máy trước.*
