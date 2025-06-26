package binaryagreement

// Các hằng số định danh loại thông điệp
const (
	BValMessageID = iota
	AuxMessageID
	ConfMessageID
	CoinMessageID
	TermMessageID
)

// BaseMessage chứa các trường chung cho tất cả các thông điệp.
type BaseMessage struct {
	Sender int
	Round  int64
}

func (bm BaseMessage) GetSender() int  { return bm.Sender }
func (bm BaseMessage) GetRound() int64 { return bm.Round }

// BValMessage chứa một giá trị đề xuất.
type BValMessage struct {
	BaseMessage
	Value bool
}

// AuxMessage được gửi khi có đủ sự ủng hộ cho một giá trị.
type AuxMessage struct {
	BaseMessage
	Value bool
}

// ConfMessage chứa một tập các giá trị đã được xác nhận.
type ConfMessage struct {
	BaseMessage
	Value *BoolSet
}

// CoinMessage chứa một phần của chữ ký ngưỡng (share).
type CoinMessage struct {
	BaseMessage
	Share []byte
}

// TermMessage thông báo giá trị quyết định cuối cùng.
type TermMessage struct {
	BaseMessage
	Value bool
}
