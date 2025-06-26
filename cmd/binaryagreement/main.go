package main

import (
	"crypto/sha256"
	"fmt"
	"math/rand"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/binaryagreement"
)

//================================================================================
// Phần Mock - Giả lập các thành phần bên ngoài
//================================================================================

// MockThreshSigLib là một phiên bản giả của thư viện chữ ký ngưỡng.
type MockThreshSigLib struct{}

func (m *MockThreshSigLib) Sign(data []byte) []byte {
	return []byte(fmt.Sprintf("share-for-%s-%d", string(data), rand.Int()))
}

func (m *MockThreshSigLib) Combine(data []byte, shares map[int][]byte) ([]byte, error) {
	var combined []byte
	// Sắp xếp các share để đảm bảo kết quả tất định (mặc dù trong thực tế không cần)
	for i := 0; i < len(shares); i++ {
		if share, ok := shares[i]; ok {
			combined = append(combined, share...)
		}
	}
	hash := sha256.Sum256(combined)
	return hash[:], nil
}

// RealisticMockNetwork mô phỏng một mạng lưới thực tế hơn với độ trễ và mất gói.
type RealisticMockNetwork struct {
	numNodes   int
	nodeChans  []chan binaryagreement.Message
	wg         sync.WaitGroup
	shutdown   chan struct{}
	latencyMin time.Duration
	latencyMax time.Duration
	lossRate   float64 // Tỷ lệ mất gói (0.0 đến 1.0)
}

// NewRealisticMockNetwork tạo một mạng mới.
func NewRealisticMockNetwork(numNodes int, latencyMin, latencyMax time.Duration, lossRate float64) *RealisticMockNetwork {
	chans := make([]chan binaryagreement.Message, numNodes)
	for i := 0; i < numNodes; i++ {
		chans[i] = make(chan binaryagreement.Message, numNodes*10) // Buffer lớn
	}
	return &RealisticMockNetwork{
		numNodes:   numNodes,
		nodeChans:  chans,
		shutdown:   make(chan struct{}),
		latencyMin: latencyMin,
		latencyMax: latencyMax,
		lossRate:   lossRate,
	}
}

// Broadcast gửi một thông điệp đến tất cả các nút khác với độ trễ và khả năng mất gói.
func (n *RealisticMockNetwork) Broadcast(senderID int, msg binaryagreement.Message) {
	for i := 0; i < n.numNodes; i++ {
		if i == senderID {
			continue // Nút sẽ tự xử lý thông điệp của mình, không gửi qua mạng
		}

		// Mô phỏng mất gói
		if rand.Float64() < n.lossRate {
			fmt.Printf("🔥 [Network] Message from %d to %d lost!\n", senderID, i)
			continue
		}

		// Mô phỏng độ trễ mạng
		latency := n.latencyMin + time.Duration(rand.Int63n(int64(n.latencyMax-n.latencyMin)))

		// Gửi tin nhắn sau một khoảng thời gian trễ
		go func(targetNode int, m binaryagreement.Message, delay time.Duration) {
			time.Sleep(delay)
			select {
			case n.nodeChans[targetNode] <- m:
			case <-n.shutdown:
			}
		}(i, msg, latency)
	}
}

// GetMessagesChannel trả về kênh nhận thông điệp cho một nút.
func (n *RealisticMockNetwork) GetMessagesChannel(nodeID int) <-chan binaryagreement.Message {
	return n.nodeChans[nodeID]
}

// Stop dừng mạng lưới và chờ tất cả các goroutine của nút kết thúc.
func (n *RealisticMockNetwork) Stop() {
	close(n.shutdown)
	n.wg.Wait()
}

// ================================================================================
// Logic chạy cho mỗi loại nút
// ================================================================================
// runHonestNodeLogic là hàm chính cho một nút trung thực.
func runHonestNodeLogic(
	nodeID int,
	initialValue bool,
	aba binaryagreement.BinaryAgreement,
	network *RealisticMockNetwork,
) {
	defer network.wg.Done()
	fmt.Printf("💡 [Node %d] Starting with proposal: %v\n", nodeID, initialValue)

	// 1. Bắt đầu bằng cách đề xuất giá trị ban đầu.
	step, err := aba.HandleInput(initialValue)
	if err != nil {
		fmt.Printf("❌ [Node %d] Error on input: %v\n", nodeID, err)
		return
	}

	// 2. SỬA LỖI: Tự xử lý các thông điệp khởi tạo trước, sau đó mới broadcast.
	// Điều này đảm bảo trạng thái của nút được cập nhật trước khi gửi đi.
	var messagesToBroadcast []binaryagreement.Message
	if step.ToBroadcast {
		messagesToBroadcast = append(messagesToBroadcast, step.Messages...)
		for _, msg := range messagesToBroadcast {
			if _, err := aba.HandleMessage(msg); err != nil {
				fmt.Printf("❌ [Node %d] Error self-handling initial message: %v\n", nodeID, err)
			}
		}
	}
	// Bây giờ mới gửi ra mạng
	for _, msg := range messagesToBroadcast {
		network.Broadcast(nodeID, msg)
	}

	// 3. Vòng lặp chính: lắng nghe và xử lý thông điệp từ mạng.
	msgChan := network.GetMessagesChannel(nodeID)
	for {
		// Kiểm tra kết thúc ở đầu vòng lặp
		if aba.HasTerminated() {
			val, _ := aba.Deliver()
			fmt.Printf("✅ [Node %d] Terminated! Final Decision: %v\n", nodeID, val)
			return
		}

		select {
		case msg := <-msgChan:
			step, err := aba.HandleMessage(msg)
			if err != nil {
				fmt.Printf("⚠️ [Node %d] Error handling message: %v\n", nodeID, err)
				continue
			}

			if step.ToBroadcast {
				// Tương tự như trên: tự xử lý trước rồi mới broadcast
				var newMessagesToBroadcast []binaryagreement.Message
				newMessagesToBroadcast = append(newMessagesToBroadcast, step.Messages...)

				for _, newMsg := range newMessagesToBroadcast {
					if _, err := aba.HandleMessage(newMsg); err != nil {
						fmt.Printf("❌ [Node %d] Error self-handling message: %v\n", nodeID, err)
					}
				}
				// Bây giờ mới gửi
				for _, newMsg := range newMessagesToBroadcast {
					network.Broadcast(nodeID, newMsg)
				}
			}

		case <-network.shutdown:
			fmt.Printf("🛑 [Node %d] Shutdown signal received.\n", nodeID)
			return
		case <-time.After(4 * time.Second): // Tăng timeout lên một chút
			if !aba.HasTerminated() {
				fmt.Printf("⏳ [Node %d] Timed out. Has not terminated.\n", nodeID)
			}
			return
		}
	}
}

// runByzantineNodeLogic mô phỏng một nút độc hại gửi thông tin mâu thuẫn.
func runByzantineNodeLogic(
	nodeID int,
	network *RealisticMockNetwork,
) {
	defer network.wg.Done()
	fmt.Printf("😈 [Node %d] Starting as a BYZANTINE node!\n", nodeID)

	// Nút Byzantine sẽ gửi BVAL(true) cho nửa đầu và BVAL(false) cho nửa sau.
	msgTrue := binaryagreement.BValMessage{
		BaseMessage: binaryagreement.BaseMessage{Sender: nodeID, Round: 0},
		Value:       true,
	}
	msgFalse := binaryagreement.BValMessage{
		BaseMessage: binaryagreement.BaseMessage{Sender: nodeID, Round: 0},
		Value:       false,
	}

	for i := 0; i < network.numNodes; i++ {
		if i == nodeID {
			continue
		}
		if i < network.numNodes/2 {
			go network.Broadcast(nodeID, msgTrue) // Gửi broadcast nhưng chỉ có một số nút nhận được
		} else {
			go network.Broadcast(nodeID, msgFalse)
		}
	}
	fmt.Printf("😈 [Node %d] Sent conflicting BVAL messages.\n", nodeID)

	// Nút Byzantine không làm gì thêm, chỉ chờ bị tắt.
	<-network.shutdown
	fmt.Printf("🛑 [Node %d] Byzantine node shutting down.\n", nodeID)
}

//================================================================================
// Hàm chạy mô phỏng và hàm Main
//================================================================================

func runSimulation(
	numNodes, f int,
	byzantineNodes map[int]struct{},
	initialProposals map[int]bool,
) {
	// --- Cấu hình mô phỏng ---
	netInfo := &binaryagreement.NetworkInfo{N: numNodes, F: f}
	sigLib := &MockThreshSigLib{}
	// Mạng với độ trễ từ 5ms đến 20ms và 5% mất gói
	network := NewRealisticMockNetwork(numNodes, 5*time.Millisecond, 20*time.Millisecond, 0.00) // <--- THAY ĐỔI Ở ĐÂY

	// --- Khởi tạo các nút ---
	nodes := make([]binaryagreement.BinaryAgreement, numNodes)
	for i := 0; i < numNodes; i++ {
		// Không cần khởi tạo ABA cho nút Byzantine vì nó sẽ không tuân theo giao thức
		if _, isByzantine := byzantineNodes[i]; !isByzantine {
			pid := fmt.Sprintf("aba-instance-%d", i)
			nodes[i] = binaryagreement.NewMoustefaouiABA(pid, i, netInfo, sigLib)
		}
	}

	// --- Bắt đầu chạy các nút trong các goroutine riêng biệt ---
	network.wg.Add(numNodes)
	for i := 0; i < numNodes; i++ {
		if _, isByzantine := byzantineNodes[i]; isByzantine {
			go runByzantineNodeLogic(i, network)
		} else {
			go runHonestNodeLogic(i, initialProposals[i], nodes[i], network)
		}
	}

	// Cho mô phỏng chạy trong một khoảng thời gian.
	time.Sleep(1 * time.Second)
	network.Stop()

	// --- Phân tích kết quả ---
	fmt.Println("\n--- Final Results ---")
	var finalDecision *bool
	allHonestTerminated := true
	allAgree := true
	correctNodeCount := 0

	for i := 0; i < numNodes; i++ {
		if _, isByzantine := byzantineNodes[i]; isByzantine {
			continue // Bỏ qua nút byzantine khi kiểm tra kết quả
		}
		correctNodeCount++
		if nodes[i].HasTerminated() {
			val, _ := nodes[i].Deliver()
			if finalDecision == nil {
				finalDecision = &val
			} else if *finalDecision != val {
				allAgree = false
			}
		} else {
			allHonestTerminated = false
		}
	}

	fmt.Println("\n--- Summary ---")
	if !allHonestTerminated {
		fmt.Println("❌ FAILURE: Not all honest nodes terminated.")
	} else if !allAgree {
		fmt.Println("❌ FAILURE: Honest nodes did not agree on a common value!")
	} else if finalDecision != nil {
		fmt.Printf("✅ SUCCESS: All %d honest nodes terminated and agreed on the value: %v\n", correctNodeCount, *finalDecision)
	} else {
		fmt.Println("🤔 RESULT: All honest nodes terminated, but no decision was made (might happen if all timed out).")
	}
}

func main() {
	// Khởi tạo seed cho bộ sinh số ngẫu nhiên
	rand.Seed(time.Now().UnixNano())

	const NUM_NODES = 4
	const F = 1 // Số nút lỗi tối đa có thể chịu được

	//==============================================================
	// Kịch bản 1: Tất cả các nút trung thực đề xuất TRUE
	//==============================================================
	fmt.Println("=================================================")
	fmt.Println("🚀 SCENARIO 1: ALL HONEST NODES PROPOSE 'TRUE'")
	fmt.Println("=================================================")
	runSimulation(
		NUM_NODES, F,
		map[int]struct{}{}, // Không có nút Byzantine
		map[int]bool{0: true, 1: true, 2: true, 3: true},
	)
	time.Sleep(2 * time.Second) // Nghỉ giữa các kịch bản

	//==============================================================
	// Kịch bản 2: Các nút trung thực bị chia rẽ
	//==============================================================
	fmt.Println("\n\n=================================================")
	fmt.Println("🚀 SCENARIO 2: HONEST NODES HAVE SPLIT VOTES")
	fmt.Println("=================================================")
	runSimulation(
		NUM_NODES, F,
		map[int]struct{}{}, // Không có nút Byzantine
		map[int]bool{0: true, 1: true, 2: false, 3: false},
	)
	time.Sleep(2 * time.Second)

	//==============================================================
	// Kịch bản 3: Có 1 nút Byzantine (f=1)
	//==============================================================
	fmt.Println("\n\n=================================================")
	fmt.Printf("🚀 SCENARIO 3: %d HONEST NODES + 1 BYZANTINE NODE\n", NUM_NODES-1)
	fmt.Println("=================================================")
	runSimulation(
		NUM_NODES, F,
		map[int]struct{}{3: {}},                  // Nút 3 là Byzantine
		map[int]bool{0: true, 1: false, 2: true}, // Đề xuất của các nút trung thực
	)
}
