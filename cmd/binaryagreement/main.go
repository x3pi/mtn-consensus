package main

import (
	"fmt"
	"math/rand"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/binaryagreement"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
)

// MessageInTransit mô phỏng một thông điệp đang được gửi qua mạng.
type MessageInTransit[N binaryagreement.NodeIdT] struct {
	Sender  N
	Message binaryagreement.Message
}

// runSimulation thực hiện một kịch bản mô phỏng hoàn chỉnh và bất đồng bộ.
func runSimulation(
	scenarioTitle string,
	nodeIDs []string,
	numFaulty int,
	byzantineNodes map[string]struct{},
	proposals map[string]bool,
) {
	logger.Info("\n\n==============================================================")
	logger.Info("🚀 KỊCH BẢN: %s (Mô phỏng bất đồng bộ)\n", scenarioTitle)
	logger.Info("==============================================================")

	// --- 1. Thiết lập mạng và các Node ---
	nodes := make(map[string]*binaryagreement.BinaryAgreement[string, string])
	nodeChannels := make(map[string]chan MessageInTransit[string])
	var wg sync.WaitGroup
	networkOutgoing := make(chan MessageInTransit[string], len(nodeIDs)*10) // Kênh đệm
	sessionID := "session-1"

	for _, id := range nodeIDs {
		netinfo := binaryagreement.NewNetworkInfo(id, nodeIDs, numFaulty, true)
		nodes[id] = binaryagreement.NewBinaryAgreement[string, string](netinfo, sessionID)
		nodeChannels[id] = make(chan MessageInTransit[string], 100)
	}

	// --- 2. Khởi chạy các Node trên các Goroutine riêng biệt ---
	for _, id := range nodeIDs {
		wg.Add(1)
		go func(nodeID string) {
			defer wg.Done()
			nodeInstance := nodes[nodeID]

			for !nodeInstance.Terminated() {
				select {
				case transitMsg := <-nodeChannels[nodeID]:
					step, err := nodeInstance.HandleMessage(transitMsg.Sender, transitMsg.Message)
					if err != nil {
						logger.Info("  LỖI xử lý thông điệp tại nút %s: %v\n", nodeID, err)
						continue
					}
					for _, msgToSend := range step.MessagesToSend {
						networkOutgoing <- MessageInTransit[string]{Sender: nodeID, Message: msgToSend.Message}
					}
				case <-time.After(3 * time.Second): // Hết giờ nếu không có hoạt động
					logger.Info("!!! CẢNH BÁO: Nút %s đã hết giờ !!!\n", nodeID)
					return
				}
			}
		}(id)
	}

	// --- 3. Khởi chạy Goroutine mạng để định tuyến thông điệp bất đồng bộ ---
	networkDone := make(chan struct{})
	go func() {
		for transitMsg := range networkOutgoing {
			originalMessage := transitMsg.Message
			senderID := transitMsg.Sender

			// Gửi thông điệp đến tất cả các node khác
			for _, recipientID := range nodeIDs {
				messageToDeliver := originalMessage // Tạo bản sao cho mỗi người nhận

				// Mô phỏng hành vi Byzantine
				if _, isByzantine := byzantineNodes[senderID]; isByzantine {
					if content, ok := originalMessage.Content.(binaryagreement.SbvMessage); ok && content.Type == "BVal" {
						if recipientID == "A" || recipientID == "B" { // Lừa dối nút A và B
							invertedContent := binaryagreement.SbvMessage{Value: !content.Value, Type: content.Type}
							messageToDeliver.Content = invertedContent
						}
					}
				}

				// Gửi với độ trễ ngẫu nhiên
				go func(recID string, msg MessageInTransit[string]) {
					// Bỏ qua nếu node nhận đã kết thúc
					if nodes[recID].Terminated() {
						return
					}
					latency := time.Duration(10+rand.Intn(50)) * time.Millisecond
					time.Sleep(latency)
					nodeChannels[recID] <- msg
				}(recipientID, MessageInTransit[string]{Sender: senderID, Message: messageToDeliver})
			}
		}
		close(networkDone)
	}()

	// --- 4. Các Node bắt đầu đề xuất giá trị ---
	for id, value := range proposals {
		if _, isByzantine := byzantineNodes[id]; isByzantine {
			continue
		}
		logger.Info("Nút trung thực %s đề xuất giá trị: %v\n", id, value)
		step, err := nodes[id].Propose(value)
		if err != nil {
			panic(fmt.Sprintf("Nút %s không thể đề xuất: %v", id, err))
		}
		for _, msgToSend := range step.MessagesToSend {
			networkOutgoing <- MessageInTransit[string]{Sender: id, Message: msgToSend.Message}
		}
	}
	logger.Info("--- Các đề xuất ban đầu đã được gửi. Mô phỏng đang chạy... ---")

	// --- 5. Đợi tất cả các node kết thúc hoặc hết giờ ---
	wg.Wait()
	close(networkOutgoing) // Dừng goroutine mạng
	<-networkDone

	// --- 6. In kết quả cuối cùng ---
	logger.Info("\n\n--- KẾT QUẢ CUỐI CÙNG ---")
	logger.Info("Tất cả các goroutine của node đã kết thúc.")

	for id, node := range nodes {
		if decision, ok := node.GetDecision(); ok {
			logger.Info("Nút %s đã kết thúc và quyết định: %v\n", id, decision)
		} else {
			logger.Info("Nút %s KHÔNG kết thúc hoặc không có quyết định.\n", id)
		}
	}
}

func runAllScenarios() {
	nodeIDs := []string{"A", "B", "C", "D"}
	numFaulty := 1

	//==============================================================
	// Kịch bản 1: Tất cả các nút đều trung thực
	//==============================================================
	runSimulation(
		"Tất cả các nút đều trung thực",
		nodeIDs, numFaulty,
		map[string]struct{}{}, // Không có nút Byzantine
		map[string]bool{"A": true, "B": true, "C": false},
	)

	//==============================================================
	// Kịch bản 2: Có 1 nút Byzantine (f=1)
	//==============================================================
	runSimulation(
		"3 Nút trung thực + 1 Nút Byzantine",
		nodeIDs, numFaulty,
		map[string]struct{}{"D": {}}, // Nút D là Byzantine
		map[string]bool{"A": true, "B": false, "C": true},
	)

	//==============================================================
	// Kịch bản 3: Các nút trung thực bị chia rẽ
	//==============================================================
	runSimulation(
		"Các nút trung thực bị chia rẽ (50/50)",
		nodeIDs, numFaulty,
		map[string]struct{}{}, // Không có nút Byzantine
		map[string]bool{"A": true, "B": false, "C": true, "D": false},
	)
}

const NUM_RUNS = 1

func main() {
	rand.Seed(time.Now().UnixNano())
	for i := 1; i <= NUM_RUNS; i++ {
		logger.Info("\n================= LẦN CHẠY %d/%d =================\n", i, NUM_RUNS)
		runAllScenarios()
	}
}
