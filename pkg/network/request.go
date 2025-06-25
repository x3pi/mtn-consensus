package network

import "github.com/meta-node-blockchain/meta-node/types/network"

// Request đại diện cho một yêu cầu (request) nhận được từ một kết nối mạng.
// Nó bao gồm thông tin về kết nối gốc và tin nhắn chứa dữ liệu của request.
type Request struct {
	connection network.Connection // connection là kết nối mà request này được nhận từ đó.
	message    network.Message    // message chứa dữ liệu thực tế của request.
}

// NewRequest tạo một instance mới của Request.
// connection: Kết nối mạng nơi request được nhận.
// message: Tin nhắn chứa dữ liệu của request.
// Hàm trả về một đối tượng network.Request.
func NewRequest(
	connection network.Connection,
	message network.Message,
) network.Request {
	// Giả định connection và message không nil dựa trên ngữ cảnh sử dụng hiện tại (ví dụ trong Connection.ReadRequest).
	// Nếu chúng có thể nil, cần thêm kiểm tra và xử lý lỗi ở đây hoặc ở nơi gọi.
	return &Request{
		connection: connection,
		message:    message,
	}
}

// Message trả về đối tượng tin nhắn (network.Message) của request.
// Tin nhắn này chứa dữ liệu cụ thể của request, bao gồm header và body.
func (r *Request) Message() network.Message {
	// Nếu r là con trỏ nil, truy cập r.message sẽ gây panic.
	// Trong Go, các phương thức thường được gọi trên các đối tượng hợp lệ.
	return r.message
}

// Connection trả về đối tượng kết nối (network.Connection) mà request này đến từ đó.
// Điều này cho phép trình xử lý request biết được nguồn gốc của request và có thể gửi phản hồi nếu cần.
func (r *Request) Connection() network.Connection {
	// Tương tự như Message(), cần đảm bảo r không phải là con trỏ nil.
	return r.connection
}
