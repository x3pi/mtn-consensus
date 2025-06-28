// file: cmd/demo/main.go
package main

import (
	"crypto/sha256"
	"log"
	"sync"

	"github.com/meta-node-blockchain/meta-node/pkg/alea"
	"github.com/meta-node-blockchain/meta-node/pkg/consensus"
	"github.com/meta-node-blockchain/meta-node/pkg/transport"
)

const NUM_NODES = 4

func main() {
	log.SetFlags(log.Ltime | log.Lmicroseconds)
	log.Println("--- Bắt đầu mô phỏng Alea-BFT (Modular) ---")

	var wg sync.WaitGroup

	// --- Thiết lập các thành phần mô phỏng ---

	// 1. Kênh bus mạng trung tâm
	networkBus := make(chan alea.Message, 100)

	// 2. Tạo một Mock ABA duy nhất cho tất cả các node
	abaInstance := consensus.NewMockABA(NUM_NODES)

	// 3. Kênh đầu ra cuối cùng của ứng dụng
	appOutCh := make(chan alea.Request, 100)

	// --- Khởi tạo các Process ---
	processes := make([]*alea.Process, NUM_NODES)
	nodeInboxes := make([]chan alea.Message, NUM_NODES)

	for i := 0; i < NUM_NODES; i++ {
		// Mỗi node có một inbox riêng
		nodeInboxes[i] = make(chan alea.Message, 100)

		// Tạo một transport cục bộ cho mỗi node
		transportInstance := transport.NewLocalTransport(i, networkBus, nodeInboxes[i])

		// Tạo process với các interface đã được triển khai MOCK
		processes[i] = alea.NewProcess(NUM_NODES, transportInstance, abaInstance, appOutCh)
	}

	// Goroutine định tuyến tin nhắn từ bus trung tâm đến inbox của từng node
	go func() {
		for msg := range networkBus {
			for i := range processes {
				// Không gửi lại cho chính người gửi
				if i != msg.SenderID {
					nodeInboxes[i] <- msg
				}
			}
		}
	}()

	// --- Thiết lập kịch bản ---
	batch1Reqs := []alea.Request{alea.Request([]byte("tx1")), alea.Request([]byte("tx2"))}
	batch1ID := sha256.Sum256([]byte("batch1"))
	batch1 := alea.Batch{ID: batch1ID[:], Requests: batch1Reqs}

	processes[0].AddInitialBatch(0, batch1) // P0 có batch1 trong hàng đợi của P0
	processes[1].AddInitialBatch(0, batch1) // P1 cũng có
	processes[2].AddInitialBatch(0, batch1) // P1 cũng có

	// processes[2].AddInitialBatch(0, batch1) // P1 cũng có
	// processes[3].AddInitialBatch(0, batch1) // P1 cũng có
	// processes[0].Start()
	// --- Bắt đầu chạy ---
	for _, p := range processes {
		p.Start()
	}

	wg.Add(1)
	go func() {
		defer wg.Done()
		for i := 0; i < 2; i++ { // Chờ nhận 2 request
			<-appOutCh
		}
	}()

	log.Println("--- Mô phỏng đang chạy ---")
	wg.Wait()
	log.Println("--- Mô phỏng kết thúc ---")
}
