package main

import (
	"fmt"

	"github.com/meta-node-blockchain/meta-node/pkg/binaryagreement"
)

// MessageInTransit mô phỏng một thông điệp đang được gửi qua mạng.
type MessageInTransit[N binaryagreement.NodeIdT] struct {
	Sender  N
	Message binaryagreement.Message
}

// processMessageBatch xử lý một lô thông điệp và trả về các thông điệp mới được tạo ra.
func processMessageBatch(
	messagesToProcess []MessageInTransit[string],
	nodes map[string]*binaryagreement.BinaryAgreement[string, string],
	byzantineNodes map[string]struct{},
	nodesToDelay map[string]struct{},
	delayedMessages *[]MessageInTransit[string],
) []MessageInTransit[string] {

	newlyGeneratedMessages := []MessageInTransit[string]{}
	nodeIDs := []string{"A", "B", "C", "D"} // Giả định ID cố định để dễ dàng

	for _, transitMsg := range messagesToProcess {
		for _, recipientID := range nodeIDs {
			// Nếu người nhận là node cần bị làm chậm, đưa vào hàng đợi trễ
			if _, shouldDelay := nodesToDelay[recipientID]; shouldDelay {
				*delayedMessages = append(*delayedMessages, MessageInTransit[string]{
					Sender:  transitMsg.Sender,
					Message: transitMsg.Message,
				})
				// Bỏ qua việc gửi ngay lập tức
				continue
			}

			recipientNode := nodes[recipientID]
			if recipientNode.Terminated() {
				continue
			}

			// Gửi thông điệp
			fmt.Printf("  Gửi thông điệp từ %s đến %s: %+v\n", transitMsg.Sender, recipientID, transitMsg.Message.Content)
			step, err := recipientNode.HandleMessage(transitMsg.Sender, transitMsg.Message)
			if err != nil {
				fmt.Printf("  LỖI xử lý thông điệp tại nút %s: %v\n", recipientID, err)
				continue
			}

			for _, msgToSend := range step.MessagesToSend {
				newlyGeneratedMessages = append(newlyGeneratedMessages, MessageInTransit[string]{
					Sender:  recipientID,
					Message: msgToSend.Message,
				})
			}
		}
	}
	return newlyGeneratedMessages
}

func main() {
	fmt.Println("\n\n==============================================================")
	fmt.Println("🚀 KỊCH BẢN: NODE NHANH GIÚP NODE CHẬM (Mô phỏng có kiểm soát)")
	fmt.Println("==============================================================")

	// --- 1. Thiết lập ---
	nodeIDs := []string{"A", "B", "C", "D"}
	numFaulty := 1
	nodes := make(map[string]*binaryagreement.BinaryAgreement[string, string])
	sessionID := "session-1"
	slowNodeID := "D" // Chủ động làm chậm nút D

	for _, id := range nodeIDs {
		netinfo := binaryagreement.NewNetworkInfo(id, nodeIDs, numFaulty, true)
		nodes[id] = binaryagreement.NewBinaryAgreement[string, string](netinfo, sessionID)
	}

	// --- 2. Các Node đề xuất ---
	proposals := map[string]bool{"A": true, "B": true, "C": true}
	initialMessages := []MessageInTransit[string]{}
	for id, value := range proposals {
		fmt.Printf("Nút %s đề xuất giá trị: %v\n", id, value)
		step, _ := nodes[id].Propose(value)
		for _, msgToSend := range step.MessagesToSend {
			initialMessages = append(initialMessages, MessageInTransit[string]{
				Sender:  id,
				Message: msgToSend.Message,
			})
		}
	}

	// --- 3. Vòng 1: Các node nhanh (A, B, C) xử lý, trong khi node D bị trễ ---
	fmt.Printf("\n--- Vòng 1: Các nút nhanh A, B, C xử lý. Thông điệp đến nút %s bị giữ lại ---\n", slowNodeID)
	delayedForD := []MessageInTransit[string]{}
	nodesToDelay := map[string]struct{}{slowNodeID: {}}

	// Các nút nhanh xử lý các đề xuất ban đầu
	newMessagesFromFastNodes := processMessageBatch(initialMessages, nodes, nil, nodesToDelay, &delayedForD)

	// Các nút nhanh tiếp tục xử lý các thông điệp mới mà chúng tạo ra
	finalMessagesFromFastNodes := processMessageBatch(newMessagesFromFastNodes, nodes, nil, nodesToDelay, &delayedForD)

	fmt.Printf("\n>>> Kết thúc Vòng 1: Các nút nhanh đã tạo ra %d thông điệp mới. Nút %s đang có %d thông điệp chờ.\n", len(finalMessagesFromFastNodes), slowNodeID, len(delayedForD))
	for id, node := range nodes {
		if id != slowNodeID {
			fmt.Printf("    Trạng thái nút nhanh %s: Epoch %d, Đã quyết định: %v\n", id, node.GetEpoch(), node.Terminated())
		}
	}

	// --- 4. Vòng 2: Node chậm D nhận TẤT CẢ thông điệp cùng lúc ---
	fmt.Printf("\n--- Vòng 2: Nút chậm %s bắt đầu nhận tất cả %d thông điệp bị trễ ---\n", slowNodeID, len(delayedForD))
	slowNode := nodes[slowNodeID]
	for _, transitMsg := range delayedForD {
		// Một số thông điệp có thể là từ kỷ nguyên mới, hoặc là TermMessage
		// giúp node D kết thúc nhanh chóng.
		if _, isTerm := transitMsg.Message.Content.(binaryagreement.TermMessage); isTerm {
			fmt.Printf("  >>> Nút nhanh %s gửi tin nhắn 'Term' để giúp nút %s kết thúc nhanh <<<\n", transitMsg.Sender, slowNodeID)
		} else {
			fmt.Printf("  Gửi thông điệp (trễ) từ %s đến %s: Epoch %d, %+v\n", transitMsg.Sender, slowNodeID, transitMsg.Message.Epoch, transitMsg.Message.Content)
		}

		slowNode.HandleMessage(transitMsg.Sender, transitMsg.Message)
		if slowNode.Terminated() {
			fmt.Printf("      *** Nút %s đã bắt kịp và quyết định! ***\n", slowNodeID)
			break
		}
	}

	// --- 5. In kết quả cuối cùng ---
	fmt.Println("\n\n--- KẾT QUẢ CUỐI CÙNG ---")
	for id, node := range nodes {
		if decision, ok := node.GetDecision(); ok {
			fmt.Printf("Nút %s đã kết thúc và quyết định: %v\n", id, decision)
		} else {
			fmt.Printf("Nút %s KHÔNG kết thúc.\n", id)
		}
	}
}
