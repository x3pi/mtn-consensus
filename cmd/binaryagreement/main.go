package main

import (
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

// ProposalEvent struct để truyền proposal events
type ProposalEvent struct {
	NodeID string
	Value  bool
}

// Thay đổi signature của hàm runSimulation
func runSimulation(
	scenarioTitle string,
	nodeIDs []string,
	numFaulty int,
	byzantineNodes map[string]struct{},
	proposalChannel <-chan ProposalEvent, // Thay thế proposals bằng channel
) {
	logger.Info("\n\n==============================================================")
	logger.Info("🚀 KỊCH BẢN: %s (Mô phỏng bất đồng bộ)\n", scenarioTitle)
	logger.Info("==============================================================")

	// --- 1. Thiết lập mạng và các Node ---
	nodes := make(map[string]*binaryagreement.BinaryAgreement[string, string])
	nodeChannels := make(map[string]chan MessageInTransit[string])
	var wg sync.WaitGroup
	networkOutgoing := make(chan MessageInTransit[string], len(nodeIDs)*10)
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
				transitMsg := <-nodeChannels[nodeID]
				step, err := nodeInstance.HandleMessage(transitMsg.Sender, transitMsg.Message)
				if err != nil {
					logger.Info("  LỖI xử lý thông điệp tại nút %s: %v\n", nodeID, err)
					continue
				}
				for _, msgToSend := range step.MessagesToSend {
					networkOutgoing <- MessageInTransit[string]{Sender: nodeID, Message: msgToSend.Message}
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
				messageToDeliver := originalMessage

				// Mô phỏng hành vi Byzantine
				if _, isByzantine := byzantineNodes[senderID]; isByzantine {
					if content, ok := originalMessage.Content.(binaryagreement.SbvMessage); ok && content.Type == "BVal" {
						if recipientID == "A" || recipientID == "B" {
							invertedContent := binaryagreement.SbvMessage{Value: !content.Value, Type: content.Type}
							messageToDeliver.Content = invertedContent
						}
					}
				}

				// Gửi với độ trễ ngẫu nhiên
				go func(recID string, msg MessageInTransit[string]) {
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

	// --- 4. Lắng nghe proposals từ channel bên ngoài ---
	proposalDone := make(chan struct{})
	var proposalWg sync.WaitGroup
	proposalWg.Add(1)
	go func() {
		defer proposalWg.Done()
		for proposalEvent := range proposalChannel {
			id := proposalEvent.NodeID
			value := proposalEvent.Value

			if _, isByzantine := byzantineNodes[id]; isByzantine {
				logger.Info("Bỏ qua proposal từ nút Byzantine %s\n", id)
				continue
			}

			logger.Info("Nhận proposal từ channel - Nút %s đề xuất giá trị: %v\n", id, value)
			step, err := nodes[id].Propose(value)
			if err != nil {
				logger.Error("Nút %s không thể đề xuất: %v\n", id, err)
				continue
			}

			for _, msgToSend := range step.MessagesToSend {
				select {
				case networkOutgoing <- MessageInTransit[string]{Sender: id, Message: msgToSend.Message}:
					// Gửi thành công
				case <-proposalDone:
					// Channel đã bị đóng, thoát
					return
				}
			}
		}
	}()

	logger.Info("--- Đang lắng nghe proposals từ channel. Mô phỏng đang chạy... ---")

	// --- 5. Đợi tất cả các node kết thúc hoặc hết giờ ---
	wg.Wait()

	// Đóng proposal channel và đợi goroutine proposal kết thúc
	close(proposalDone)
	proposalWg.Wait()

	close(networkOutgoing)
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

// Sửa lại hàm runAllScenarios để sử dụng channel
func runAllScenarios() {
	nodeIDs := []string{"A", "B", "C", "D", "E"}
	numFaulty := 1

	//==============================================================
	// Kịch bản 1: Tất cả các nút đều trung thực
	//==============================================================
	proposalChannel1 := make(chan ProposalEvent, 10)
	go func() {
		// Gửi các proposals cho kịch bản 1
		proposalChannel1 <- ProposalEvent{NodeID: "A", Value: true}
		time.Sleep(1000 * time.Millisecond)
		proposalChannel1 <- ProposalEvent{NodeID: "B", Value: true}
		time.Sleep(1000 * time.Millisecond)
		proposalChannel1 <- ProposalEvent{NodeID: "C", Value: true}
		time.Sleep(1000 * time.Millisecond)
		proposalChannel1 <- ProposalEvent{NodeID: "D", Value: false}
		proposalChannel1 <- ProposalEvent{NodeID: "E", Value: false}

		close(proposalChannel1)
	}()

	runSimulation(
		"Tất cả các nút đều trung thực",
		nodeIDs, numFaulty,
		map[string]struct{}{},
		proposalChannel1,
	)

	// //==============================================================
	// // Kịch bản 2: Có 1 nút Byzantine (f=1)
	// //==============================================================
	// proposalChannel2 := make(chan ProposalEvent, 10)
	// go func() {
	// 	// Gửi các proposals cho kịch bản 2
	// 	proposalChannel2 <- ProposalEvent{NodeID: "A", Value: true}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	proposalChannel2 <- ProposalEvent{NodeID: "B", Value: false}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	proposalChannel2 <- ProposalEvent{NodeID: "C", Value: true}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	close(proposalChannel2)
	// }()

	// runSimulation(
	// 	"3 Nút trung thực + 1 Nút Byzantine",
	// 	nodeIDs, numFaulty,
	// 	map[string]struct{}{"D": {}},
	// 	proposalChannel2,
	// )

	// //==============================================================
	// // Kịch bản 3: Các nút trung thực bị chia rẽ
	// //==============================================================
	// proposalChannel3 := make(chan ProposalEvent, 10)
	// go func() {
	// 	// Gửi các proposals cho kịch bản 3
	// 	proposalChannel3 <- ProposalEvent{NodeID: "A", Value: true}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	proposalChannel3 <- ProposalEvent{NodeID: "B", Value: false}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	proposalChannel3 <- ProposalEvent{NodeID: "C", Value: true}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	proposalChannel3 <- ProposalEvent{NodeID: "D", Value: false}
	// 	time.Sleep(1000 * time.Millisecond)
	// 	close(proposalChannel3)
	// }()

	// runSimulation(
	// 	"Các nút trung thực bị chia rẽ (50/50)",
	// 	nodeIDs, numFaulty,
	// 	map[string]struct{}{},
	// 	proposalChannel3,
	// )
}

const NUM_RUNS = 1

func main() {
	rand.Seed(time.Now().UnixNano())
	for i := 1; i <= NUM_RUNS; i++ {
		logger.Info("\n================= LẦN CHẠY %d/%d =================\n", i, NUM_RUNS)
		runAllScenarios()
	}
}
