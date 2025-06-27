package binaryagreement

import (
	"fmt"
	"sync"
)

// --- Generic Types ---
// NodeIdT đại diện cho một định danh nút có thể so sánh được.
type NodeIdT comparable

// SessionIdT đại diện cho một định danh phiên có thể so sánh được.
type SessionIdT comparable

// --- Network & Node Info ---
// NetworkInfo chứa thông tin về mạng lưới.
type NetworkInfo[N NodeIdT] struct {
	ourId       N
	allNodes    []N
	isValidator bool
	numFaulty   int
	mutex       sync.RWMutex
}

func NewNetworkInfo[N NodeIdT](ourId N, allNodes []N, numFaulty int, isValidator bool) *NetworkInfo[N] {
	return &NetworkInfo[N]{
		ourId:       ourId,
		allNodes:    allNodes,
		isValidator: isValidator,
		numFaulty:   numFaulty,
	}
}

func (ni *NetworkInfo[N]) OurId() N {
	ni.mutex.RLock()
	defer ni.mutex.RUnlock()
	return ni.ourId
}

func (ni *NetworkInfo[N]) NumCorrect() int {
	ni.mutex.RLock()
	defer ni.mutex.RUnlock()
	return len(ni.allNodes) - ni.numFaulty
}

func (ni *NetworkInfo[N]) NumFaulty() int {
	ni.mutex.RLock()
	defer ni.mutex.RUnlock()
	return ni.numFaulty
}

func (ni *NetworkInfo[N]) IsValidator() bool {
	ni.mutex.RLock()
	defer ni.mutex.RUnlock()
	return ni.isValidator
}

// --- Messages ---
// MessageContent là một interface cho các loại nội dung thông điệp khác nhau.
type MessageContent interface {
	isMessageContent()
}

// Các loại Message cụ thể
type SbvMessage struct {
	Value bool
	Type  string
} // Type: "BVal" hoặc "Aux"
type ConfMessage struct{ Values BoolSet }
type TermMessage struct{ Value bool }

func (m SbvMessage) isMessageContent()  {}
func (m ConfMessage) isMessageContent() {}
func (m TermMessage) isMessageContent() {}

// Message là cấu trúc thông điệp chính được gửi qua mạng.
type Message struct {
	Epoch   uint64
	Content MessageContent
}

// CanExpire cho biết liệu thông điệp có thể bị bỏ qua nếu epoch đã cũ.
func (m Message) CanExpire() bool {
	switch m.Content.(type) {
	case TermMessage:
		return false
	default:
		return true
	}
}

// --- Faults and Errors ---
// FaultKind định nghĩa các loại lỗi có thể xảy ra.
type FaultKind string

const (
	FaultDuplicateBVal  FaultKind = "DuplicateBVal"
	FaultDuplicateAux   FaultKind = "DuplicateAux"
	FaultMultipleConf   FaultKind = "MultipleConf"
	FaultMultipleTerm   FaultKind = "MultipleTerm"
	FaultAgreementEpoch FaultKind = "AgreementEpoch"
	FaultCoin           FaultKind = "CoinFault"
)

type Fault[N NodeIdT] struct {
	Sender N
	Kind   FaultKind
}

func (f Fault[N]) Error() string {
	return fmt.Sprintf("fault from %v: %s", f.Sender, f.Kind)
}

// --- Step ---
// Target xác định đích đến của một thông điệp.
type Target[N NodeIdT] struct {
	ToAll  bool
	NodeId N
}

// Step đại diện cho kết quả của một bước xử lý.
type Step[N NodeIdT] struct {
	MessagesToSend []struct {
		Target  Target[N]
		Message Message
	}
	// Output có thể là `bool` (kết quả cuối cùng) hoặc `BoolSet` (kết quả từ SBV).
	Output interface{}
	Faults []Fault[N]
}
