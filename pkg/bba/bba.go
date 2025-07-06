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

type BBAInstance struct {
	process   *Process
	sessionID string
	N, F      int
	nodeID    int32
	epoch     uint32
	binValues []bool
	sentBvals []bool
	recvBval  map[int32]bool
	recvAux   map[int32]bool
	done      bool
	decision  interface{}
	estimated interface{}
	lock      sync.RWMutex
}

type Process struct {
	host      core.Host
	instances map[string]*BBAInstance
	mu        sync.RWMutex
}

// NewProcess khởi tạo BBA Process, nhận Host làm dependency.
func NewProcess(host core.Host) (*Process, error) {
	return &Process{
		host:      host,
		instances: make(map[string]*BBAInstance),
	}, nil
}

// CommandHandlers implement interface core.Module
func (p *Process) CommandHandlers() map[string]func(t_network.Request) error {
	return map[string]func(t_network.Request) error{
		BBA_COMMAND: p.handleNetworkRequest,
	}
}

// Start implement interface core.Module
func (p *Process) Start() {
	logger.Info("BBA Process started.")
}

// Stop implement interface core.Module
func (p *Process) Stop() {
	logger.Info("BBA Process stopping.")
}

func (p *Process) handleNetworkRequest(req t_network.Request) error {
	msg := &pb.BBAMessage{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		logger.Error("Failed to unmarshal BBA message: %v", err)
		return err
	}

	p.mu.RLock()
	instance, ok := p.instances[msg.SessionId]
	p.mu.RUnlock()

	if !ok {
		logger.Warn("Received BBA message for unknown session: %s", msg.SessionId)
		return nil
	}
	return instance.handleMessage(msg)
}

func (p *Process) StartAgreement(sessionID string, value bool) {
	p.mu.Lock()
	if _, exists := p.instances[sessionID]; exists {
		p.mu.Unlock()
		logger.Warn("BBA instance for session %s already exists", sessionID)
		return
	}
	instance := p.NewBBAInstance(sessionID)
	p.instances[sessionID] = instance
	p.mu.Unlock()

	logger.Info("Node %d starting BBA for session '%s' with value %v", p.host.ID(), sessionID, value)
	instance.inputValue(value)
}

func (p *Process) NewBBAInstance(sessionID string) *BBAInstance {
	config := p.host.Config()
	return &BBAInstance{
		process:   p,
		sessionID: sessionID,
		N:         config.NumValidator,
		F:         (config.NumValidator - 1) / 3,
		nodeID:    p.host.ID(),
		recvBval:  make(map[int32]bool),
		recvAux:   make(map[int32]bool),
		sentBvals: []bool{},
		binValues: []bool{},
	}
}

func (bba *BBAInstance) broadcast(msg *pb.BBAMessage) {
	payload, err := proto.Marshal(msg)
	if err != nil {
		logger.Error("Failed to marshal BBA message: %v", err)
		return
	}
	bba.process.host.Broadcast(BBA_COMMAND, payload)
}

// ... Các hàm helper còn lại của BBAInstance giữ nguyên ...
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
	if bba.done || msg.Epoch < int32(bba.epoch) {
		return nil
	}
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
	logger.Info("Node %d advancing BBA session '%s' to epoch %d", bba.nodeID, bba.sessionID, bba.epoch+1)
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
	bba.recvBval = make(map[int32]bool)
	bba.recvAux = make(map[int32]bool)
	bba.sentBvals = []bool{}
	bba.binValues = []bool{}
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
