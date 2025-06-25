package aleaqueues

import (
	"container/heap"
	"fmt"
	"sync"
)

// QueueManager quản lý một tập hợp các hàng đợi ưu tiên, mỗi hàng đợi cho một peer.
type QueueManager struct {
	queues map[int32]*priorityQueue
	mu     sync.RWMutex
}

// NewQueueManager tạo và khởi tạo một QueueManager mới.
// peerIDs là một slice chứa ID của tất cả các node trong mạng.
func NewQueueManager(peerIDs []int32) *QueueManager {
	qm := &QueueManager{
		queues: make(map[int32]*priorityQueue),
	}
	// Khởi tạo một hàng đợi cho mỗi peer
	for _, id := range peerIDs {
		pq := make(priorityQueue, 0)
		heap.Init(&pq)
		qm.queues[id] = &pq
	}
	return qm
}

// Enqueue là phương thức công khai để thêm một đề xuất vào hàng đợi của người gửi tương ứng.
// Đây là giao diện chính mà module rbc sẽ sử dụng.
func (qm *QueueManager) Enqueue(proposerID int32, priority int64, payload []byte) {
	qm.mu.Lock()
	defer qm.mu.Unlock()

	q, ok := qm.queues[proposerID]
	if !ok {
		// Trường hợp này không nên xảy ra nếu tất cả peer đã được cung cấp lúc khởi tạo,
		// nhưng vẫn xử lý để đảm bảo an toàn.
		pq := make(priorityQueue, 0)
		heap.Init(&pq)
		qm.queues[proposerID] = &pq
		q = &pq
	}

	newItem := &item{
		Value:    payload,
		Priority: priority,
	}
	heap.Push(q, newItem)
}

// Dequeue lấy ra phần tử có độ ưu tiên cao nhất từ hàng đợi của một proposer.
// (Hữu ích cho thành phần Agreement sau này)
func (qm *QueueManager) Dequeue(proposerID int32) ([]byte, error) {
	qm.mu.Lock()
	defer qm.mu.Unlock()

	q, ok := qm.queues[proposerID]
	if !ok {
		return nil, fmt.Errorf("không tìm thấy hàng đợi cho proposer ID %d", proposerID)
	}
	if q.Len() == 0 {
		return nil, fmt.Errorf("hàng đợi của proposer ID %d rỗng", proposerID)
	}

	i := heap.Pop(q).(*item)
	return i.Value, nil
}
