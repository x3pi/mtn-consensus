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
			logger.Info("[AGREEMENT] Starting agreement for round %d", blockNumber+1)
			p.executeRound(blockNumber + 1)
		}
	}()
}

func (p *Process) Stop() {
	logger.Info("Agreement Component Stopping.")
}

// executeRound thực hiện một vòng thỏa thuận duy nhất.
func (p *Process) executeRound(round uint64) {

	if !p.start {
		p.start = true
	}

	time.Sleep(10 * time.Millisecond)

	if p.host.MasterConn() == nil || !p.host.MasterConn().IsConnect() {
		logger.Error("[AGREEMENT] Node %d: MASTER CONNECTION IS DOWN at start of round %d!", p.host.ID(), round)
	}

	// Kiểm tra xem có phải lượt của node này không
	isMyTurnForNextBlock := ((int(round) + p.host.Config().NumValidator - 1) % p.host.Config().NumValidator) == (int(p.host.Config().ID) - 1)

	if isMyTurnForNextBlock {
		p.rbcProcess.MessageSender.SendBytes(
			p.host.MasterConn(),
			common.GetTransactionsPool,
			[]byte{},
		)
	}

	// Chọn leader và hàng đợi tương ứng
	numValidators := p.host.Config().NumValidator
	leaderID := int32((round % uint64(numValidators)) + 1)

	queue := p.rbcProcess.QueueManager().GetQueue(leaderID)
	if queue == nil {
		return // Kết thúc vòng này nếu không có queue
	}

	// Lấy proposal từ đầu hàng đợi
	headItem := queue.Peek()
	var headValue []byte = nil
	if headItem != nil {
		headValue = headItem.Value
	}

	proposal := false
	if headValue != nil {
		proposal = true
	}

	// Chạy BBA (ABA) và chờ kết quả
	sessionID := fmt.Sprintf("agreement_round_%d", round)

	decision, err := p.bbaProcess.StartAgreementAndWait(sessionID, proposal, 600*time.Second)
	if err != nil {
		logger.Error("[AGREEMENT] Round %d: BBA failed: %v", round, err)
		// panic("AGREEMENT")
	}

	// Nếu BBA quyết định 1 (true)
	if decision {
		if headValue == nil {
			logger.Warn("[AGREEMENT] Round %d: BBA decided 1, but my queue is empty. Sending FILL-GAP.", round)

			fillGapMsg := &pb.FillGapRequest{
				QueueId:  leaderID,
				Head:     p.rbcProcess.QueueManager().Head(leaderID),
				SenderId: p.host.ID(),
			}
			payload, _ := proto.Marshal(fillGapMsg)
			p.host.Broadcast(AGREEMENT_FILL_GAP, payload)

			logger.Info("[AGREEMENT] Round %d: Waiting for FILLER message...", round)
			headValue = <-p.fillGapChan
			logger.Info("[AGREEMENT] Round %d: Gap filled!", round)
		}

		p.acDeliver(headValue, leaderID, round)
	} else {
		// Nếu quyết định là FALSE, tạo và gửi một sự kiện rỗng
		logger.Info("[AGREEMENT] Round %d: Decision is false. Finalizing an empty event.", round)

		// Tạo một batch rỗng, chỉ chứa số thứ tự block/round
		emptyBatch := &pb.Batch{BlockNumber: round}
		payload, err := proto.Marshal(emptyBatch)
		if err != nil {
			logger.Error("[AGREEMENT] Round %d: Failed to marshal empty batch: %v", round, err)
			return
		}
		logger.Info("PushFinalizeEvent rỗng")
		// Đẩy sự kiện rỗng để hoàn tất block
		p.rbcProcess.PushFinalizeEvent(payload)

		// Tăng head của hàng đợi của leader đã bị bỏ qua để đánh dấu tiến độ
		p.rbcProcess.QueueManager().IncrementHead(leaderID)
	}
}

// acDeliver delivers the value to the application layer and cleans up queues.
func (p *Process) acDeliver(value []byte, queueID int32, round uint64) {
	logger.Info("[AGREEMENT] Delivering value for round %d from queue %d", round, queueID)

	p.rbcProcess.QueueManager().DequeueByValue(value)
	p.rbcProcess.QueueManager().IncrementHead(queueID)

	logger.Info("PushFinalizeEvent")
	p.rbcProcess.PushFinalizeEvent(value)
}

// Xử lý khi nhận tin nhắn FILL-GAP
func (p *Process) handleFillGap(req t_network.Request) error {
	msg := &pb.FillGapRequest{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		return err
	}
	senderID := msg.GetSenderId()

	logger.Info("[AGREEMENT] Received FILL-GAP from Node %d for queue %d", senderID, msg.QueueId)

	// Lấy hàng đợi từ QueueManager
	localQueue := p.rbcProcess.QueueManager().GetQueue(msg.QueueId)
	// Lấy con trỏ head cũng từ QueueManager bằng ID của hàng đợi
	localHead := p.rbcProcess.QueueManager().Head(msg.QueueId)

	// So sánh giá trị head đã lấy được
	if localHead >= msg.Head {
		if localQueue != nil {
			item := localQueue.Peek()
			if item != nil {
				// Gửi lại tin nhắn FILLER
				fillerMsg := &pb.FillerRequest{
					Entries: [][]byte{item.Value},
				}
				payload, _ := proto.Marshal(fillerMsg)
				p.host.Send(senderID, AGREEMENT_FILLER, payload)
				logger.Info("[AGREEMENT] Sent FILLER to Node %d for queue %d", senderID, msg.QueueId)
			}
		}
	}
	return nil
}

// Xử lý khi nhận tin nhắn FILLER
func (p *Process) handleFiller(req t_network.Request) error {
	msg := &pb.FillerRequest{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		return err
	}

	logger.Info("[AGREEMENT] Received FILLER message")

	// Xử lý từng entry trong tin nhắn FILLER
	for _, entry := range msg.Entries {
		// Giả định: RBC có cơ chế để "ép" delivery một tin nhắn.
		// Điều này sẽ kích hoạt logic trong RBC để đưa tin nhắn vào hàng đợi.
		p.rbcProcess.ForceDeliver(entry)

		// Báo hiệu cho agreementLoop rằng gap đã được lấp đầy
		p.fillGapChan <- entry
	}

	return nil
}
