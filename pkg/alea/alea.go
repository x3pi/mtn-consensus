// file: pkg/alea/alea.go
package alea

import (
	"log"
	"sync"
	"time"
)

// Process đại diện cho một node chạy logic thỏa thuận của Alea.
type Process struct {
	mu       sync.Mutex
	id       int
	numNodes int
	ri       int
	queues   []*PriorityQueue
	S        map[string]bool

	// Các thành phần trừu tượng
	transport Transport
	aba       ABA

	// Kênh đầu ra cho ứng dụng
	appOutCh chan<- Request
}

// NewProcess khởi tạo một tiến trình Alea mới.
// Lưu ý rằng nó nhận vào các interface, không phải struct cụ thể.
func NewProcess(numNodes int, transport Transport, aba ABA, appOutCh chan<- Request) *Process {
	queues := make([]*PriorityQueue, numNodes)
	for i := 0; i < numNodes; i++ {
		queues[i] = NewPriorityQueue(i)
	}

	return &Process{
		id:        transport.ID(),
		numNodes:  numNodes,
		ri:        -1,
		queues:    queues,
		S:         make(map[string]bool),
		transport: transport,
		aba:       aba,
		appOutCh:  appOutCh,
	}
}

// Start khởi chạy các vòng lặp xử lý của tiến trình.
func (p *Process) Start() {
	go p.runEventLoop()
	go p.acStart()
}

// AddInitialBatch thêm một batch dữ liệu ban đầu vào hàng đợi của một node.
// Dùng cho việc thiết lập kịch bản mô phỏng.
func (p *Process) AddInitialBatch(ownerNodeID int, b Batch) {
	if ownerNodeID < len(p.queues) {
		p.queues[ownerNodeID].Add(b)
	}
}

// runEventLoop xử lý các tin nhắn đến từ các node khác.
func (p *Process) runEventLoop() {
	for msg := range p.transport.Events() {
		switch payload := msg.Payload.(type) {
		case FillGapMsg:
			p.handleFillGap(payload, msg.SenderID)
		case FillerMsg:
			p.handleFiller(payload)
		}
	}
}

// acStart triển khai vòng lặp thỏa thuận chính (dòng 5-16 trong thuật toán).
func (p *Process) acStart() {
	for {
		p.mu.Lock()
		p.ri++
		round := p.ri
		p.mu.Unlock()

		leaderID := round % p.numNodes
		Q := p.queues[leaderID]

		_, ok := Q.Peek()
		proposal := 0
		if ok {
			proposal = 1
		}

		log.Printf("[P%d][r%d] Leader là P%d. Đề xuất: %d", p.id, round, leaderID, proposal)

		// Tương tác với ABA thông qua interface
		p.aba.Propose(round, proposal)
		resultCh := p.aba.Result(round)
		b := <-resultCh

		log.Printf("[P%d][r%d] ABA quyết định: %d", p.id, round, b)

		if b == 1 {
			value, ok := Q.Peek()
			if !ok {
				log.Printf("[P%d][r%d] Bị thiếu dữ liệu! Gửi FILL-GAP cho hàng đợi Q%d.", p.id, round, Q.id)
				p.transport.Broadcast(FillGapMsg{QueueID: Q.id, Slot: Q.head})

				for {
					time.Sleep(50 * time.Millisecond) // Chờ một chút để tin nhắn được xử lý
					value, ok = Q.Peek()
					if ok {
						log.Printf("[P%d][r%d] Lỗ hổng đã được lấp đầy!", p.id, round)
						break
					}
				}
			}
			p.acDeliver(value)
		}
	}
}

// handleFillGap xử lý khi nhận tin nhắn FILL-GAP (dòng 17-21).
func (p *Process) handleFillGap(msg FillGapMsg, requesterID int) {
	log.Printf("[P%d] Nhận FILL-GAP từ P%d cho Q%d tại slot %d", p.id, requesterID, msg.QueueID, msg.Slot)

	p.mu.Lock()
	defer p.mu.Unlock()

	Q := p.queues[msg.QueueID]

	// Dòng 19: Kiểm tra nếu có thể giúp
	if Q.head >= msg.Slot {
		// Dòng 20: Tính toán các entry bị thiếu
		entries := make(map[int]Batch)
		for i := msg.Slot; i <= Q.head; i++ {
			if b, ok := Q.slots[i]; ok {
				entries[i] = b
			}
		}

		if len(entries) > 0 {
			// Dòng 21: Gửi FILLER
			log.Printf("[P%d] Gửi FILLER đến P%d với %d entry", p.id, requesterID, len(entries))
			// Trong mô phỏng, ta broadcast tin nhắn này. Một triển khai thực tế sẽ gửi trực tiếp (unicast).
			p.transport.Broadcast(FillerMsg{Entries: entries})
		}
	}
}

// handleFiller xử lý khi nhận tin nhắn FILLER (dòng 22-24).
func (p *Process) handleFiller(msg FillerMsg) {
	log.Printf("[P%d] Nhận FILLER với %d entry", p.id, len(msg.Entries))

	p.mu.Lock()
	defer p.mu.Unlock()

	// Dòng 23-24: "Deliver M to VCBC" - Ở đây ta mô phỏng bằng cách thêm trực tiếp vào hàng đợi
	for slot, batch := range msg.Entries {
		// Tìm hàng đợi tương ứng. Trong Alea, mỗi VCBC gắn với 1 node (leader).
		// Batch ID có thể chứa thông tin về leader, nhưng ở đây ta đơn giản hóa.
		// Giả sử ta biết batch này thuộc hàng đợi nào (dựa trên logic ứng dụng).
		// Để mô phỏng, ta sẽ thử thêm nó vào tất cả các hàng đợi nếu slot đó trống.
		for _, q := range p.queues {
			if _, exists := q.slots[slot]; !exists {
				// Đây là một cách đơn giản hóa, thực tế cần cơ chế chính xác hơn
				// để xác định batch này thuộc hàng đợi nào.
			}
		}
		// Cách tiếp cận chính xác hơn cho mô phỏng:
		// Logic của FILL-GAP/FILLER là để lấp đầy hàng đợi của leader vòng hiện tại.
		// Ta có thể giả định các entry này thuộc hàng đợi của leader.
		leaderID := (p.ri) % p.numNodes
		Q := p.queues[leaderID]
		if _, exists := Q.slots[slot]; !exists {
			Q.slots[slot] = batch
		}
	}
}

// acDeliver triển khai thủ tục AC-DELIVER (dòng 25-31).
func (p *Process) acDeliver(value Batch) {
	p.mu.Lock()
	defer p.mu.Unlock()

	log.Printf("[P%d] AC_DELIVER: %s", p.id, value)

	// Dòng 26-27: Xóa batch khỏi tất cả các hàng đợi
	for _, q := range p.queues {
		q.Dequeue(value)
	}

	// Dòng 28-31: Giao các request chưa từng thấy cho ứng dụng
	for _, req := range value.Requests {
		reqStr := string(req)
		if !p.S[reqStr] {
			p.S[reqStr] = true
			log.Printf("[P%d] ===> Giao cho ứng dụng: %s", p.id, reqStr)
			p.appOutCh <- req
		}
	}
}
