package aleaqueues

import (
	"bytes"
	"container/heap"
)

// Item đại diện cho một phần tử trong hàng đợi ưu tiên.
type Item struct {
	Value    []byte // Dữ liệu của đề xuất (batch of transactions)
	Priority int64  // Độ ưu tiên từ người gửi (ví dụ: block number)
	index    int    // Chỉ số của item trong heap.
}

// PriorityQueue triển khai heap.Interface và chứa các Item.
type PriorityQueue []*Item

func (pq PriorityQueue) Len() int { return len(pq) }

func (pq PriorityQueue) Less(i, j int) bool {
	return pq[i].Priority < pq[j].Priority
}

func (pq PriorityQueue) Swap(i, j int) {
	pq[i], pq[j] = pq[j], pq[i]
	pq[i].index = i
	pq[j].index = j
}

func (pq *PriorityQueue) Push(x interface{}) {
	n := len(*pq)
	item := x.(*Item)
	item.index = n
	*pq = append(*pq, item)
}

func (pq *PriorityQueue) Pop() interface{} {
	old := *pq
	n := len(old)
	item := old[n-1]
	old[n-1] = nil  // Tránh memory leak
	item.index = -1 // Để an toàn
	*pq = old[0 : n-1]
	return item
}

// Peek trả về phần tử đầu tiên mà không xóa nó.
func (pq *PriorityQueue) Peek() *Item {
	if len(*pq) == 0 {
		return nil
	}
	return (*pq)[0]
}

// Remove xóa một giá trị cụ thể khỏi hàng đợi.
func (pq *PriorityQueue) Remove(valueToRemove []byte) {
	newItems := make(PriorityQueue, 0)
	for _, item := range *pq {
		if !bytes.Equal(item.Value, valueToRemove) {
			newItems = append(newItems, item)
		}
	}
	*pq = newItems
	heap.Init(pq)
}
