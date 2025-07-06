package bba

import (
	"fmt"
	"sync"

	"github.com/meta-node-blockchain/meta-node/pkg/core"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
	"google.golang.org/protobuf/proto"
)

const (
	BBA_COMMAND = "bba_message"
)

var _ core.Module = (*Process)(nil)

// BBAInstance đại diện cho một phiên đồng thuận BBA cụ thể.
type BBAInstance struct {
	process        *Process
	sessionID      string
	N, F           int
	nodeID         int32
	epoch          uint32
	binValues      []bool
	sentBvals      []bool
	recvBval       map[int32]bool
	recvAux        map[int32]bool
	done           bool
	decision       interface{}
	estimated      interface{}
	futureMessages map[int32][]*pb.BBAMessage
	lock           sync.RWMutex
}

// Process quản lý tất cả các phiên BBA và các tin nhắn đang chờ.
type Process struct {
	host            core.Host
	instances       map[string]*BBAInstance
	pendingMessages map[string][]*pb.BBAMessage // Bộ đệm cho các session chưa được khởi tạo
	mu              sync.RWMutex
}

// NewProcess khởi tạo BBA Process.
func NewProcess(host core.Host) (*Process, error) {
	return &Process{
		host:            host,
		instances:       make(map[string]*BBAInstance),
		pendingMessages: make(map[string][]*pb.BBAMessage), // Khởi tạo bộ đệm
	}, nil
}

// CommandHandlers triển khai interface core.Module.
func (p *Process) CommandHandlers() map[string]func(t_network.Request) error {
	return map[string]func(t_network.Request) error{
		BBA_COMMAND: p.handleNetworkRequest,
	}
}

// Start triển khai interface core.Module.
func (p *Process) Start() {
	logger.Info("BBA Process started.")
}

// Stop triển khai interface core.Module.
func (p *Process) Stop() {
	logger.Info("BBA Process stopping.")
}

// handleNetworkRequest là điểm tiếp nhận tất cả tin nhắn BBA từ mạng.
func (p *Process) handleNetworkRequest(req t_network.Request) error {
	msg := &pb.BBAMessage{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		logger.Error("Failed to unmarshal BBA message: %v", err)
		return err
	}

	p.mu.Lock()
	instance, ok := p.instances[msg.SessionId]
	if !ok {
		// Nếu session chưa tồn tại, lưu tin nhắn vào bộ đệm
		logger.Warn("Received BBA message for unknown session '%s'. Buffering message.", msg.SessionId)
		p.pendingMessages[msg.SessionId] = append(p.pendingMessages[msg.SessionId], msg)
		p.mu.Unlock()
		return nil
	}
	p.mu.Unlock()

	// Nếu session đã tồn tại, xử lý bình thường
	return instance.handleMessage(msg)
}

// StartAgreement bắt đầu một phiên đồng thuận mới.
func (p *Process) StartAgreement(sessionID string, value bool) {
	p.mu.Lock()
	if _, exists := p.instances[sessionID]; exists {
		p.mu.Unlock()
		logger.Warn("BBA instance for session %s already exists", sessionID)
		return
	}

	// Tạo instance mới
	instance := p.NewBBAInstance(sessionID)
	p.instances[sessionID] = instance

	// Kiểm tra bộ đệm xem có tin nhắn nào đang chờ cho session này không
	if pending, found := p.pendingMessages[sessionID]; found {
		logger.Info("Processing %d buffered messages for new BBA session '%s'", len(pending), sessionID)
		for _, msg := range pending {
			// Chuyển các tin nhắn đang chờ cho instance mới xử lý
			go instance.handleMessage(msg)
		}
		// Xóa các tin nhắn đã xử lý khỏi bộ đệm
		delete(p.pendingMessages, sessionID)
	}
	p.mu.Unlock()

	logger.Info("Node %d starting BBA for session '%s' with value %v", p.host.ID(), sessionID, value)
	instance.inputValue(value)
}

// NewBBAInstance tạo một BBAInstance mới.
func (p *Process) NewBBAInstance(sessionID string) *BBAInstance {
	config := p.host.Config()
	return &BBAInstance{
		process:        p,
		sessionID:      sessionID,
		N:              config.NumValidator,
		F:              (config.NumValidator - 1) / 3,
		nodeID:         p.host.ID(),
		recvBval:       make(map[int32]bool),
		recvAux:        make(map[int32]bool),
		sentBvals:      []bool{},
		binValues:      []bool{},
		futureMessages: make(map[int32][]*pb.BBAMessage),
	}
}

// ... (Các hàm còn lại của BBAInstance không thay đổi) ...

func (bba *BBAInstance) broadcast(msg *pb.BBAMessage) {
	payload, err := proto.Marshal(msg)
	if err != nil {
		logger.Error("Failed to marshal BBA message: %v", err)
		return
	}
	bba.process.host.Broadcast(BBA_COMMAND, payload)
}

func (bba *BBAInstance) inputValue(val bool) {
	bba.lock.Lock()
	defer bba.lock.Unlock()
	if bba.epoch != 0 || bba.estimated != nil {
		return
	}
	bba.estimated = val
	bba.sentBvals = append(bba.sentBvals, val)
	bba.broadcastBval(val)
	bba.handleBvalRequest(bba.nodeID, val)
}

func (bba *BBAInstance) handleMessage(msg *pb.BBAMessage) error {
	bba.lock.Lock()
	defer bba.lock.Unlock()

	if bba.done {
		return nil
	}

	if msg.Epoch > int32(bba.epoch) {
		logger.Info("Node %d BBA session '%s': Đệm tin nhắn cho epoch %d (hiện tại là %d)", bba.nodeID, bba.sessionID, msg.Epoch, bba.epoch)
		bba.futureMessages[msg.Epoch] = append(bba.futureMessages[msg.Epoch], msg)
		return nil
	}

	if msg.Epoch < int32(bba.epoch) {
		return nil
	}

	return bba.processMessage(msg)
}

func (bba *BBAInstance) processMessage(msg *pb.BBAMessage) error {
	switch content := msg.Content.(type) {
	case *pb.BBAMessage_BvalRequest:
		bba.handleBvalRequest(msg.SenderId, content.BvalRequest.Value)
	case *pb.BBAMessage_AuxRequest:
		bba.handleAuxRequest(msg.SenderId, content.AuxRequest.Value)
	default:
		return fmt.Errorf("unknown BBA message content")
	}
	return nil
}

func (bba *BBAInstance) handleBvalRequest(senderID int32, val bool) {
	if _, exists := bba.recvBval[senderID]; exists {
		return
	}
	bba.recvBval[senderID] = val
	count := 0
	for _, v := range bba.recvBval {
		if v == val {
			count++
		}
	}
	if count >= bba.F+1 && !bba.hasSentBval(val) {
		bba.sentBvals = append(bba.sentBvals, val)
		bba.broadcastBval(val)
	}
	if count >= 2*bba.F+1 && !bba.isInBinValues(val) {
		bba.binValues = append(bba.binValues, val)
		if len(bba.binValues) == 1 {
			bba.broadcastAux(val)
			bba.handleAuxRequest(bba.nodeID, val)
		}
	}
}

func (bba *BBAInstance) handleAuxRequest(senderID int32, val bool) {
	if _, exists := bba.recvAux[senderID]; exists {
		return
	}
	bba.recvAux[senderID] = val
	bba.tryOutputAgreement()
}

func (bba *BBAInstance) tryOutputAgreement() {
	if len(bba.binValues) == 0 || len(bba.recvAux) < bba.N-bba.F {
		return
	}
	auxValuesSet := make(map[bool]struct{})
	for _, vote := range bba.recvAux {
		auxValuesSet[vote] = struct{}{}
	}
	values := []bool{}
	valueExists := make(map[bool]bool)
	for _, binVal := range bba.binValues {
		if _, ok := auxValuesSet[binVal]; ok {
			if !valueExists[binVal] {
				values = append(values, binVal)
				valueExists[binVal] = true
			}
		}
	}
	coin := bba.epoch%2 == 0
	if bba.decision != nil && bba.decision.(bool) == coin {
		if !bba.done {
			bba.done = true
			logger.Info("✅ Node %d BBA session '%s' is DONE. Final Decision: %v", bba.nodeID, bba.sessionID, bba.decision)
		}
		return
	}

	bba.advanceEpoch()

	if len(values) == 1 {
		bba.estimated = values[0]
		if bba.decision == nil && values[0] == coin {
			bba.decision = values[0]
			logger.Info("🏆 Node %d BBA session '%s' DECIDED on %v at epoch %d", bba.nodeID, bba.sessionID, bba.decision, bba.epoch-1)
		}
	} else {
		bba.estimated = coin
	}
	est := bba.estimated.(bool)
	bba.sentBvals = append(bba.sentBvals, est)
	bba.broadcastBval(est)
	bba.handleBvalRequest(bba.nodeID, est)
}

func (bba *BBAInstance) advanceEpoch() {
	bba.epoch++
	logger.Info("Node %d advancing BBA session '%s' to epoch %d", bba.nodeID, bba.sessionID, bba.epoch)

	bba.recvBval = make(map[int32]bool)
	bba.recvAux = make(map[int32]bool)
	bba.sentBvals = []bool{}
	bba.binValues = []bool{}

	if bufferedMsgs, found := bba.futureMessages[int32(bba.epoch)]; found {
		logger.Info("Node %d BBA session '%s': Xử lý %d tin nhắn đã đệm cho epoch %d", bba.nodeID, bba.sessionID, len(bufferedMsgs), bba.epoch)
		for _, msg := range bufferedMsgs {
			bba.processMessage(msg)
		}
		delete(bba.futureMessages, int32(bba.epoch))
	}
}

func (bba *BBAInstance) broadcastBval(value bool) {
	msg := &pb.BBAMessage{
		SessionId: bba.sessionID,
		Epoch:     int32(bba.epoch),
		SenderId:  bba.nodeID,
		Content:   &pb.BBAMessage_BvalRequest{BvalRequest: &pb.BvalRequest{Value: value}},
	}
	bba.broadcast(msg)
}

func (bba *BBAInstance) broadcastAux(value bool) {
	msg := &pb.BBAMessage{
		SessionId: bba.sessionID,
		Epoch:     int32(bba.epoch),
		SenderId:  bba.nodeID,
		Content:   &pb.BBAMessage_AuxRequest{AuxRequest: &pb.AuxRequest{Value: value}},
	}
	bba.broadcast(msg)
}

func (bba *BBAInstance) hasSentBval(val bool) bool {
	for _, v := range bba.sentBvals {
		if v == val {
			return true
		}
	}
	return false
}

func (bba *BBAInstance) isInBinValues(val bool) bool {
	for _, v := range bba.binValues {
		if v == val {
			return true
		}
	}
	return false
}
