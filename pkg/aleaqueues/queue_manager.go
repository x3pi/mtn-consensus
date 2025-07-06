package aleaqueues

import (
	"container/heap"
	"fmt"
	"sync"
)

// QueueManager quản lý một tập hợp các hàng đợi ưu tiên và con trỏ head của chúng.
type QueueManager struct {
	queues map[int32]*PriorityQueue
	heads  map[int32]uint64 // Thêm map để theo dõi head cho mỗi hàng đợi
	mu     sync.RWMutex
}

// NewQueueManager tạo và khởi tạo một QueueManager mới.
func NewQueueManager(peerIDs []int32) *QueueManager {
	qm := &QueueManager{
		queues: make(map[int32]*PriorityQueue),
		heads:  make(map[int32]uint64), // Khởi tạo map heads
	}
	for _, id := range peerIDs {
		pq := make(PriorityQueue, 0)
		heap.Init(&pq)
		qm.queues[id] = &pq
		qm.heads[id] = 0 // Khởi tạo head = 0 cho mỗi hàng đợi
	}
	return qm
}

// Enqueue thêm một đề xuất vào hàng đợi của người gửi tương ứng.
func (qm *QueueManager) Enqueue(proposerID int32, priority int64, payload []byte) {
	qm.mu.Lock()
	defer qm.mu.Unlock()

	q, ok := qm.queues[proposerID]
	if !ok {
		pq := make(PriorityQueue, 0)
		heap.Init(&pq)
		qm.queues[proposerID] = &pq
		q = &pq
	}

	newItem := &Item{ // Sử dụng Item đã export
		Value:    payload,
		Priority: priority,
	}
	heap.Push(q, newItem)
}

// DequeueByValue loại bỏ một giá trị cụ thể khỏi tất cả các hàng đợi.
func (qm *QueueManager) DequeueByValue(valueToRemove []byte) {
	qm.mu.Lock()
	defer qm.mu.Unlock()

	for _, q := range qm.queues {
		q.Remove(valueToRemove)
	}
}

// GetByPriority trả về phần tử có priority đúng bằng giá trị truyền vào.
func (qm *QueueManager) GetByPriority(proposerID int32, priority int64) ([]byte, error) {
	qm.mu.RLock()
	defer qm.mu.RUnlock()

	q, ok := qm.queues[proposerID]
	if !ok {
		return nil, fmt.Errorf("không tìm thấy hàng đợi cho proposer ID %d", proposerID)
	}
	if q.Len() == 0 {
		return nil, fmt.Errorf("hàng đợi của proposer ID %d rỗng", proposerID)
	}

	for _, it := range *q {
		if it.Priority == priority {
			return it.Value, nil
		}
	}
	return nil, fmt.Errorf("không tìm thấy phần tử với priority %d trong hàng đợi proposer ID %d", priority, proposerID)
}

// GetQueue trả về hàng đợi ưu tiên cho một ID node cụ thể.
// === THÊM PHƯƠNG THỨC NÀY VÀO ===
func (qm *QueueManager) GetQueue(id int32) *PriorityQueue {
	qm.mu.RLock()
	defer qm.mu.RUnlock()

	q, ok := qm.queues[id]
	if !ok {
		return nil
	}
	return q
}

func (qm *QueueManager) Head(id int32) uint64 {
	qm.mu.RLock()
	defer qm.mu.RUnlock()
	return qm.heads[id]
}

// IncrementHead tăng con trỏ head của một hàng đợi lên 1.
func (qm *QueueManager) IncrementHead(id int32) {
	qm.mu.Lock()
	defer qm.mu.Unlock()
	qm.heads[id]++
}
