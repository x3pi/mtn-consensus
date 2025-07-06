package agreement

import (
	"encoding/binary"
	"fmt"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/bba"
	"github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/core"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/loggerfile"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
	"google.golang.org/protobuf/proto"
)

const (
	AGREEMENT_FILL_GAP = "agreement_fill_gap"
	AGREEMENT_FILLER   = "agreement_filler"
)

// Process implement a single-shot consensus instance
type Process struct {
	host            core.Host
	bbaProcess      *bba.Process
	rbcProcess      *rbc.Process
	round           int
	fillGapChan     chan []byte // Channel để báo hiệu khi một gap đã được lấp đầy
	mu              sync.RWMutex
	BlockNumberChan chan uint64 // <-- THÊM KÊNH VÀO ĐÂY
	start           bool
}

// NewProcess creates a new agreement process
func NewProcess(host core.Host, rbcProcess *rbc.Process, bbaProcess *bba.Process) *Process {
	return &Process{
		host:            host,
		rbcProcess:      rbcProcess,
		bbaProcess:      bbaProcess,
		round:           0,
		fillGapChan:     make(chan []byte, 1),
		BlockNumberChan: make(chan uint64, 1024),
		start:           false,
	}
}

func (p *Process) CommandHandlers() map[string]func(req t_network.Request) error {
	return map[string]func(req t_network.Request) error{
		AGREEMENT_FILL_GAP: p.handleFillGap,
		AGREEMENT_FILLER:   p.handleFiller,
		common.BlockNumber: func(req t_network.Request) error {
			responseData := req.Message().Body()
			if len(responseData) < 8 {
				return fmt.Errorf("invalid block number response data length")
			}
			validatorBlockNumber := binary.BigEndian.Uint64(responseData)

			// === THÊM DÒNG LOG NÀY ===
			logger.Info("[AGREEMENT-TRIGGER] Node %d RECEIVED trigger for block number: %d", p.host.ID(), validatorBlockNumber)
			// ==========================

			p.BlockNumberChan <- validatorBlockNumber
			return nil
		},
	}
}

// Start initiates the agreement component.
func (p *Process) Start() {
	logger.Info("Agreement Component Started. Waiting for block numbers.")
	go func() {
		// Lắng nghe trên p.BlockNumberChan thay vì p.rbcProcess.BlockNumberChan
		for blockNumber := range p.BlockNumberChan {
			logger.Info("[AGREEMENT] Received new block number %d. Starting agreement for round %d.", blockNumber, blockNumber+1)
			p.executeRound(blockNumber + 1)
		}
	}()
}

func (p *Process) Stop() {
	logger.Info("Agreement Component Stopping.")
}

// executeRound thực hiện một vòng thỏa thuận duy nhất.
// Tên hàm đã đổi từ agreementLoop và giờ nhận `round` làm tham số.
// executeRound thực hiện một vòng thỏa thuận duy nhất.
// Tên hàm đã đổi từ agreementLoop và giờ nhận `round` làm tham số.
func (p *Process) executeRound(round uint64) {
	// ADDED: Create file logger for this round
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + fmt.Sprintf("agreement-r-%d", round) + ".log")

	// ADDED: Log round start
	fileLogger.Info("🚀 Node %d: Starting agreement round %d", p.host.ID(), round)

	if !p.start {
		p.start = true
		fileLogger.Info("Node %d: First round started", p.host.ID())
	}

	time.Sleep(20 * time.Millisecond)

	if p.host.MasterConn() == nil || !p.host.MasterConn().IsConnect() {
		fileLogger.Info("Node %d: MASTER CONNECTION IS DOWN at start of round %d!", p.host.ID(), round)
		logger.Error("[AGREEMENT] Node %d: MASTER CONNECTION IS DOWN at start of round %d!", p.host.ID(), round)
	}

	// ADDED: Log turn calculation
	isMyTurnForNextBlock := ((int(round) + p.host.Config().NumValidator - 1) % p.host.Config().NumValidator) == (int(p.host.Config().ID) - 1)
	fileLogger.Info("Node %d: Is my turn for next block? %v (round: %d, validator count: %d, my ID: %d)",
		p.host.ID(), isMyTurnForNextBlock, round, p.host.Config().NumValidator, p.host.Config().ID)

	if isMyTurnForNextBlock {
		fileLogger.Info("Node %d: Đến lượt mình đề xuất cho block %d. Đang yêu cầu transactions...", p.host.ID(), round)
		p.rbcProcess.MessageSender.SendBytes(
			p.host.MasterConn(),
			common.GetTransactionsPool,
			[]byte{},
		)
	}

	// Dòng 6: Chọn leader và hàng đợi tương ứng
	numValidators := p.host.Config().NumValidator
	leaderID := int32((round % uint64(numValidators)) + 1)
	fileLogger.Info("Node %d: Round %d - Selected leader: Node %d (total validators: %d)", p.host.ID(), round, leaderID, numValidators)

	queue := p.rbcProcess.QueueManager().GetQueue(leaderID)
	if queue == nil {
		fileLogger.Info("Node %d: Round %d - Could not find queue for leader %d", p.host.ID(), round, leaderID)
		return // Kết thúc vòng này nếu không có queue
	}

	// Lấy proposal từ đầu hàng đợi
	headItem := queue.Peek()
	var headValue []byte = nil
	if headItem != nil {
		headValue = headItem.Value
		fileLogger.Info("Node %d: Round %d - Queue head item found, value length: %d bytes", p.host.ID(), round, len(headValue))
	} else {
		fileLogger.Info("Node %d: Round %d - Queue head is empty", p.host.ID(), round)
	}

	proposal := false
	if headValue != nil {
		proposal = true
	}

	fileLogger.Info("Node %d: Round %d - Leader is Node %d. Proposing '%v'", p.host.ID(), round, leaderID, proposal)

	// Dòng 9-10: Chạy BBA (ABA) và chờ kết quả
	sessionID := fmt.Sprintf("agreement_round_%d", round)
	fileLogger.Info("Node %d: Round %d - Starting BBA agreement with session ID: %s", p.host.ID(), round, sessionID)

	decision, err := p.bbaProcess.StartAgreementAndWait(sessionID, proposal, 30*time.Second)
	if err != nil {
		fileLogger.Info("Node %d: Round %d - BBA failed: %v", p.host.ID(), round, err)
		logger.Error("[AGREEMENT] Round %d: BBA failed: %v", round, err)
		panic("AGREEMENT")
	}

	fileLogger.Info("Node %d: Round %d - BBA decided '%v'", p.host.ID(), round, decision)

	// Dòng 11: Nếu BBA quyết định 1 (true)
	if decision {
		fileLogger.Info("Node %d: Round %d - BBA decided TRUE, processing proposal", p.host.ID(), round)

		if headValue == nil {
			fileLogger.Info("Node %d: Round %d - BBA decided 1, but my queue is empty. Sending FILL-GAP.", p.host.ID(), round)
			logger.Warn("[AGREEMENT] Round %d: BBA decided 1, but my queue is empty. Sending FILL-GAP.", round)

			fillGapMsg := &pb.FillGapRequest{
				QueueId:  leaderID,
				Head:     p.rbcProcess.QueueManager().Head(leaderID),
				SenderId: p.host.ID(),
			}
			payload, _ := proto.Marshal(fillGapMsg)
			p.host.Broadcast(AGREEMENT_FILL_GAP, payload)
			fileLogger.Info("Node %d: Round %d - Broadcasted FILL-GAP message for queue %d, head: %d", p.host.ID(), round, leaderID, fillGapMsg.Head)

			fileLogger.Info("Node %d: Round %d - Waiting for FILLER message...", p.host.ID(), round)
			logger.Info("[AGREEMENT] Round %d: Waiting for FILLER message...", round)
			headValue = <-p.fillGapChan
			fileLogger.Info("Node %d: Round %d - Gap filled! Received value of length: %d bytes", p.host.ID(), round, len(headValue))
			logger.Info("[AGREEMENT] Round %d: Gap filled!", round)
		}

		fileLogger.Info("Node %d: Round %d - Delivering value to application layer", p.host.ID(), round)
		p.acDeliver(headValue, leaderID, round) // Truyền round vào acDeliver để logging
	} else {
		// Nếu quyết định là FALSE, tạo và gửi một sự kiện rỗng
		fileLogger.Info("Node %d: Round %d - BBA decided FALSE. Finalizing an empty event.", p.host.ID(), round)
		logger.Info("[AGREEMENT] Round %d: Decision is false. Finalizing an empty event.", round)

		// Tạo một batch rỗng, chỉ chứa số thứ tự block/round
		emptyBatch := &pb.Batch{BlockNumber: round}
		payload, err := proto.Marshal(emptyBatch)
		if err != nil {
			fileLogger.Info("Node %d: Round %d - Failed to marshal empty batch: %v", p.host.ID(), round, err)
			logger.Error("[AGREEMENT] Round %d: Failed to marshal empty batch: %v", round, err)
			return
		}
		fileLogger.Info("Node %d: Round %d - Pushing empty finalize event", p.host.ID(), round)
		logger.Info("PushFinalizeEvent rỗng")
		// Đẩy sự kiện rỗng để hoàn tất block
		p.rbcProcess.PushFinalizeEvent(payload)

		// Tăng head của hàng đợi của leader đã bị bỏ qua để đánh dấu tiến độ
		p.rbcProcess.QueueManager().IncrementHead(leaderID)
		fileLogger.Info("Node %d: Round %d - Incremented head for queue %d", p.host.ID(), round, leaderID)
	}

	fileLogger.Info("Node %d: Round %d - Agreement round completed", p.host.ID(), round)
	// KHÔNG CÓ p.round++ ở đây nữa
}

// acDeliver delivers the value to the application layer and cleans up queues.
// acDeliver delivers the value and increments the queue's head.
func (p *Process) acDeliver(value []byte, queueID int32, round uint64) {
	// ADDED: Create file logger for this delivery
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + fmt.Sprintf("agreement-r-%d", round) + ".log")

	fileLogger.Info("Node %d: Round %d - Delivering value for queue %d, value length: %d bytes", p.host.ID(), round, queueID, len(value))
	logger.Info("[AGREEMENT] Delivering value for round %d from queue %d", round, queueID)

	p.rbcProcess.QueueManager().DequeueByValue(value)
	fileLogger.Info("Node %d: Round %d - Dequeued value from queue %d", p.host.ID(), round, queueID)

	p.rbcProcess.QueueManager().IncrementHead(queueID)
	fileLogger.Info("Node %d: Round %d - Incremented head for queue %d", p.host.ID(), round, queueID)

	fileLogger.Info("Node %d: Round %d - Pushing finalize event", p.host.ID(), round)
	logger.Info("PushFinalizeEvent")

	p.rbcProcess.PushFinalizeEvent(value)
	fileLogger.Info("Node %d: Round %d - Value successfully delivered and finalized", p.host.ID(), round)
}

// Dòng 17: Xử lý khi nhận tin nhắn FILL-GAP
func (p *Process) handleFillGap(req t_network.Request) error {
	msg := &pb.FillGapRequest{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		return err
	}
	senderID := msg.GetSenderId()

	// ADDED: Create file logger for this message handling
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + "fillgap.log")

	fileLogger.Info("Node %d: Received FILL-GAP from Node %d for queue %d, head: %d", p.host.ID(), senderID, msg.QueueId, msg.Head)
	logger.Info("[AGREEMENT] Received FILL-GAP from Node %d for queue %d", senderID, msg.QueueId)

	// === SỬA LỖI TẠI ĐÂY ===
	// Lấy hàng đợi từ QueueManager
	localQueue := p.rbcProcess.QueueManager().GetQueue(msg.QueueId)
	// Lấy con trỏ head cũng từ QueueManager bằng ID của hàng đợi
	localHead := p.rbcProcess.QueueManager().Head(msg.QueueId)
	// === KẾT THÚC SỬA LỖI ===

	fileLogger.Info("Node %d: Local queue %d head: %d, requested head: %d", p.host.ID(), msg.QueueId, localHead, msg.Head)

	// Dòng 19: So sánh giá trị head đã lấy được
	if localHead >= msg.Head {
		if localQueue != nil {
			item := localQueue.Peek()
			if item != nil {
				// Dòng 21: Gửi lại tin nhắn FILLER
				fillerMsg := &pb.FillerRequest{
					Entries: [][]byte{item.Value},
				}
				payload, _ := proto.Marshal(fillerMsg)
				p.host.Send(senderID, AGREEMENT_FILLER, payload)
				fileLogger.Info("Node %d: Sent FILLER to Node %d for queue %d, value length: %d bytes", p.host.ID(), senderID, msg.QueueId, len(item.Value))
				logger.Info("[AGREEMENT] Sent FILLER to Node %d for queue %d", senderID, msg.QueueId)
			} else {
				fileLogger.Info("Node %d: Queue %d exists but head item is nil", p.host.ID(), msg.QueueId)
			}
		} else {
			fileLogger.Info("Node %d: Local queue %d is nil", p.host.ID(), msg.QueueId)
		}
	} else {
		fileLogger.Info("Node %d: Local head %d < requested head %d, cannot fill gap", p.host.ID(), localHead, msg.Head)
	}
	return nil
}

// Dòng 22: Xử lý khi nhận tin nhắn FILLER
func (p *Process) handleFiller(req t_network.Request) error {
	msg := &pb.FillerRequest{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		return err
	}

	// ADDED: Create file logger for this message handling
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + "filler.log")

	fileLogger.Info("Node %d: Received FILLER message with %d entries", p.host.ID(), len(msg.Entries))
	logger.Info("[AGREEMENT] Received FILLER message")

	// Dòng 23-24: Xử lý từng entry trong tin nhắn FILLER
	for i, entry := range msg.Entries {
		fileLogger.Info("Node %d: Processing FILLER entry %d, length: %d bytes", p.host.ID(), i+1, len(entry))

		// Giả định: RBC có cơ chế để "ép" delivery một tin nhắn.
		// Điều này sẽ kích hoạt logic trong RBC để đưa tin nhắn vào hàng đợi.
		p.rbcProcess.ForceDeliver(entry)
		fileLogger.Info("Node %d: Force delivered entry %d", p.host.ID(), i+1)

		// Báo hiệu cho agreementLoop rằng gap đã được lấp đầy
		p.fillGapChan <- entry
		fileLogger.Info("Node %d: Sent entry %d to fillGapChan", p.host.ID(), i+1)
	}

	fileLogger.Info("Node %d: FILLER message processing completed", p.host.ID())
	return nil
}
