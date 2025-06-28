// file: pkg/alea/types.go
package alea

import (
	"bytes"
	"fmt"
	"sync"
)

// Request đại diện cho một yêu cầu từ client.
type Request []byte

// Batch là một lô các yêu cầu.
type Batch struct {
	ID       []byte
	Requests []Request
}

// Equals so sánh hai batch.
func (b Batch) Equals(other Batch) bool {
	return bytes.Equal(b.ID, other.ID)
}

// String để in ra cho dễ đọc.
func (b Batch) String() string {
	return fmt.Sprintf("Batch(%x)", b.ID)
}

// PriorityQueue là một hàng đợi ưu tiên đơn giản cho các batch.
type PriorityQueue struct {
	mu    sync.Mutex
	id    int
	head  int
	slots map[int]Batch // key là vị trí (slot), value là batch
}

// NewPriorityQueue tạo một hàng đợi ưu tiên mới.
func NewPriorityQueue(id int) *PriorityQueue {
	return &PriorityQueue{
		id:    id,
		head:  0,
		slots: make(map[int]Batch),
	}
}

// Add thêm một batch vào cuối hàng đợi.
func (pq *PriorityQueue) Add(b Batch) {
	pq.mu.Lock()
	defer pq.mu.Unlock()
	// Tìm slot trống tiếp theo
	nextSlot := pq.head
	for {
		if _, exists := pq.slots[nextSlot]; !exists {
			pq.slots[nextSlot] = b
			break
		}
		nextSlot++
	}
}

// Peek trả về batch ở đầu hàng đợi (tại con trỏ head).
func (pq *PriorityQueue) Peek() (Batch, bool) {
	pq.mu.Lock()
	defer pq.mu.Unlock()
	b, ok := pq.slots[pq.head]
	return b, ok
}

// Dequeue xóa một batch cụ thể khỏi tất cả các vị trí trong hàng đợi
// và tăng con trỏ head nếu batch bị xóa nằm ở đầu.
func (pq *PriorityQueue) Dequeue(b Batch) {
	pq.mu.Lock()
	defer pq.mu.Unlock()

	// Tăng head nếu batch ở đầu được xóa
	if currentHead, ok := pq.slots[pq.head]; ok && currentHead.Equals(b) {
		delete(pq.slots, pq.head)
		pq.head++
		// Tiếp tục tăng head nếu các slot tiếp theo cũng đã bị xóa bởi các lần Dequeue khác
		for {
			if _, exists := pq.slots[pq.head]; !exists && len(pq.slots) > 0 {
				// Đảm bảo không nhảy vô tận nếu có lỗ hổng lớn ở giữa
				isGap := false
				for i := pq.head + 1; i < pq.head+len(pq.slots)+1; i++ {
					if _, subsequentExists := pq.slots[i]; subsequentExists {
						isGap = true
						break
					}
				}
				if !isGap {
					break
				}
				pq.head++
			} else {
				break
			}
		}
	}

	// Xóa batch này ở tất cả các vị trí khác (xử lý trùng lặp)
	for slot, batchInQueue := range pq.slots {
		if batchInQueue.Equals(b) {
			delete(pq.slots, slot)
		}
	}
}

// ---- Các loại tin nhắn mạng ----

// Message là cấu trúc tin nhắn chung được truyền trên mạng.
type Message struct {
	SenderID int
	Payload  interface{}
}

// FillGapMsg tương ứng với tin nhắn FILL-GAP
type FillGapMsg struct {
	QueueID int // Hàng đợi q mà người gửi đang bị thiếu
	Slot    int // Vị trí s mà người gửi đang bị thiếu
}

// FillerMsg tương ứng với tin nhắn FILLER
type FillerMsg struct {
	Entries map[int]Batch // Dữ liệu bị thiếu
}
