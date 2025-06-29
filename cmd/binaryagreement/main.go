package main

import (
	"context"
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

func runSimulation(
	ctx context.Context, // Sử dụng context để quản lý vòng đời
	cancel context.CancelFunc, // Hàm để hủy context
	scenarioTitle string,
	nodeIDs []string,
	numFaulty int,
	proposalChannel chan ProposalEvent,
) {
	// Khi hàm runSimulation kết thúc, đảm bảo context được hủy
	// để dọn dẹp các goroutine liên quan (ví dụ: goroutine gửi proposal).
	defer cancel()

	logger.Info("\n\n==============================================================")
	logger.Info("🚀 KỊCH BẢN: %s (Mô phỏng bất đồng bộ)\n", scenarioTitle)
	logger.Info("==============================================================")

	nodes := make(map[string]*binaryagreement.BinaryAgreement[string, string])
	nodeChannels := make(map[string]chan MessageInTransit[string])
	var nodeWg sync.WaitGroup
	networkOutgoing := make(chan MessageInTransit[string], len(nodeIDs)*10)
	sessionID := "session-1"
	var closeOnce sync.Once // Dùng để đảm bảo việc đóng các channel chỉ thực hiện 1 lần

	for _, id := range nodeIDs {
		netinfo := binaryagreement.NewNetworkInfo(id, nodeIDs, numFaulty, true)
		nodes[id] = binaryagreement.NewBinaryAgreement[string, string](netinfo, sessionID)
		nodeChannels[id] = make(chan MessageInTransit[string], 100)
	}

	// Hàm dọn dẹp và kết thúc mô phỏng
	cleanupAndShutdown := func() {
		closeOnce.Do(func() {
			logger.Info("🎉 Đạt được đồng thuận! Bắt đầu quá trình kết thúc mô phỏng.")
			// 1. Hủy context để báo cho các goroutine bên ngoài (như proposal sender) dừng lại
			cancel()
			// 2. Đóng proposal channel để goroutine xử lý proposal kết thúc
			close(proposalChannel)
			// 3. Đóng network channel
			close(networkOutgoing)
		})
	}

	// --- 2. Khởi chạy các Node trên các Goroutine riêng biệt ---
	for _, id := range nodeIDs {
		nodeWg.Add(1)
		go func(nodeID string) {
			defer nodeWg.Done()
			nodeInstance := nodes[nodeID]

			for {
				select {
				case <-ctx.Done(): // Nếu context bị hủy, kết thúc goroutine
					return
				case transitMsg, ok := <-nodeChannels[nodeID]:
					if !ok { // Nếu channel đã đóng, kết thúc
						return
					}
					step, err := nodeInstance.HandleMessage(transitMsg.Sender, transitMsg.Message)
					if err != nil {
						continue
					}
					for _, msgToSend := range step.MessagesToSend {
						// Sử dụng select để tránh block nếu networkOutgoing đã đóng
						select {
						case networkOutgoing <- MessageInTransit[string]{Sender: nodeID, Message: msgToSend.Message}:
						case <-ctx.Done():
							return
						}
					}
				}
			}
		}(id)
	}

	// --- 3. Goroutine mạng để định tuyến thông điệp ---
	var networkWg sync.WaitGroup
	networkWg.Add(1)
	go func() {
		defer networkWg.Done()
		for transitMsg := range networkOutgoing {
			for _, recipientID := range nodeIDs {
				// Tạo bản sao để tránh race condition khi gửi bất đồng bộ
				msgCopy := transitMsg
				go func(recID string, msg MessageInTransit[string]) {
					select {
					case <-ctx.Done():
						return
					default:
						if nodes[recID] == nil || nodes[recID].Terminated() {
							return
						}
						latency := time.Duration(10+rand.Intn(50)) * time.Millisecond
						time.Sleep(latency)
						nodeChannels[recID] <- msg
					}
				}(recipientID, msgCopy)
			}
		}
		// Khi networkOutgoing đóng, đóng tất cả các channel của node để chúng kết thúc
		for _, ch := range nodeChannels {
			close(ch)
		}
	}()

	// --- 4. Goroutine xử lý các proposal đến ---
	logger.Info("--- Đang lắng nghe proposals từ channel. Mô phỏng đang chạy... ---")
	var proposalWg sync.WaitGroup
	proposalWg.Add(1)
	go func() {
		defer proposalWg.Done()
		for proposalEvent := range proposalChannel { // Dừng khi proposalChannel được đóng
			id := proposalEvent.NodeID
			value := proposalEvent.Value
			logger.Info("Nhận proposal từ channel - Nút %s đề xuất giá trị: %v\n", id, value)

			if nodes[id] == nil || nodes[id].Terminated() {
				continue
			}

			step, err := nodes[id].Propose(value)
			if err != nil {
				logger.Error("Nút %s không thể đề xuất: %v\n", id, err)
				continue
			}

			for _, msgToSend := range step.MessagesToSend {
				networkOutgoing <- MessageInTransit[string]{Sender: id, Message: msgToSend.Message}
			}
		}
	}()

	// --- 5. Goroutine giám sát trạng thái đồng thuận ---
	var monitorWg sync.WaitGroup
	monitorWg.Add(1)
	go func() {
		defer monitorWg.Done()
		requiredDecisions := len(nodeIDs) - numFaulty
		ticker := time.NewTicker(100 * time.Millisecond)
		defer ticker.Stop()

		for {
			select {
			case <-ticker.C:
				decidedCount := 0
				for _, node := range nodes {
					if node.Terminated() {
						decidedCount++
					}
				}
				if decidedCount >= requiredDecisions {
					cleanupAndShutdown()
					return // Kết thúc goroutine giám sát
				}
			case <-ctx.Done(): // Nếu context bị hủy từ bên ngoài, cũng kết thúc
				return
			}
		}
	}()

	// --- 6. Chờ các tiến trình hoàn tất ---
	proposalWg.Wait() // Chờ xử lý proposal xong (khi channel đóng)
	nodeWg.Wait()     // Chờ các node goroutine kết thúc
	networkWg.Wait()  // Chờ goroutine mạng kết thúc
	monitorWg.Wait()  // Chờ goroutine giám sát kết thúc

	// --- 7. In kết quả cuối cùng ---
	logger.Info("\n\n--- KẾT QUẢ CUỐI CÙNG ---")
	for id, node := range nodes {
		if decision, ok := node.GetDecision(); ok {
			logger.Info("✅ Nút %s đã kết thúc và quyết định: %v\n", id, decision)
		} else {
			logger.Warn("❌ Nút %s KHÔNG kết thúc hoặc không có quyết định.\n", id)
		}
	}
}

func runAllScenarios() {
	var wg sync.WaitGroup
	nodeIDs := []string{"1", "2", "3", "4", "5"}
	numFaulty := 1

	//==============================================================
	// Kịch bản 1: Tất cả các nút đều trung thực
	//==============================================================
	wg.Add(1)
	go func() {
		defer wg.Done()

		// Tạo context để có thể hủy goroutine gửi proposal từ xa
		ctx, cancel := context.WithCancel(context.Background())
		proposalChannel1 := make(chan ProposalEvent, len(nodeIDs))

		// Goroutine để gửi proposals
		go func() {
			proposals := []ProposalEvent{
				{NodeID: "1", Value: true},
				{NodeID: "2", Value: false},
				{NodeID: "3", Value: false},
				{NodeID: "4", Value: false},
				{NodeID: "5", Value: true},
			}
			for _, p := range proposals {
				select {
				case proposalChannel1 <- p:
					// Gửi thành công, ngủ một chút để mô phỏng thực tế
					time.Sleep(50 * time.Millisecond)
				case <-ctx.Done():
					// Context đã bị hủy (do simulation đã xong), ngừng gửi
					logger.Info("Goroutine gửi proposal đã dừng do context bị hủy.")
					return
				}
			}
			logger.Info("Đã gửi xong tất cả proposals theo kế hoạch.")
		}()

		runSimulation(
			ctx, cancel, // Truyền context và hàm cancel vào
			"Tất cả các nút đều trung thực",
			nodeIDs, numFaulty,
			proposalChannel1,
		)
	}()

	wg.Wait()
}

const NUM_RUNS = 1

func main() {
	rand.Seed(time.Now().UnixNano())
	for i := 1; i <= NUM_RUNS; i++ {
		logger.Info("\n================= LẦN CHẠY %d/%d =================\n", i, NUM_RUNS)
		runAllScenarios()
		logger.Info("\n================= KẾT THÚC LẦN CHẠY %d/%d =================\n", i, NUM_RUNS)
	}
}
