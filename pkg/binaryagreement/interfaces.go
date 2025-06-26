package binaryagreement

// Step chứa kết quả của một bước xử lý, bao gồm giá trị quyết định (nếu có)
// và các thông điệp cần gửi đi.
type Step struct {
	Decision    *bool
	Messages    []Message
	ToBroadcast bool // Cho biết có cần broadcast thông điệp không
}

// Message là interface chung cho tất cả các thông điệp trong giao thức.
type Message interface {
	GetSender() int
	GetRound() int64
}

// BinaryAgreement là interface cốt lõi cho một thuật toán thỏa thuận nhị phân.
type BinaryAgreement interface {
	// HandleInput bắt đầu giao thức với một giá trị đề xuất.
	HandleInput(value bool) (*Step, error)

	// HandleMessage xử lý một thông điệp nhận được từ một nút khác.
	HandleMessage(msg Message) (*Step, error)

	// HasTerminated kiểm tra xem giao thức đã kết thúc chưa.
	HasTerminated() bool

	// Deliver trả về giá trị quyết định cuối cùng.
	Deliver() (bool, bool) // value, ok
}
