package aleaqueues

// item đại diện cho một phần tử trong hàng đợi ưu tiên.
// Nó không được export để đóng gói logic bên trong package.
type item struct {
	Value    []byte // Dữ liệu của đề xuất (batch of transactions)
	Priority int64  // Độ ưu tiên từ người gửi (ví dụ: block number)
	index    int    // Chỉ số của item trong heap.
}

// priorityQueue triển khai heap.Interface và chứa các item.
type priorityQueue []*item

func (pq priorityQueue) Len() int { return len(pq) }

func (pq priorityQueue) Less(i, j int) bool {
	// Giả định rằng số ưu tiên nhỏ hơn có nghĩa là độ ưu tiên cao hơn.
	return pq[i].Priority < pq[j].Priority
}

func (pq priorityQueue) Swap(i, j int) {
	pq[i], pq[j] = pq[j], pq[i]
	pq[i].index = i
	pq[j].index = j
}

func (pq *priorityQueue) Push(x interface{}) {
	n := len(*pq)
	i := x.(*item)
	i.index = n
	*pq = append(*pq, i)
}

func (pq *priorityQueue) Pop() interface{} {
	old := *pq
	n := len(old)
	i := old[n-1]
	old[n-1] = nil // Tránh memory leak
	i.index = -1   // Để an toàn
	*pq = old[0 : n-1]
	return i
}
