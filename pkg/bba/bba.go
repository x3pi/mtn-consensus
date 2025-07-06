package bba

import (
	"fmt"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/core"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/loggerfile"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
	"google.golang.org/protobuf/proto"
)

const (
	BBA_COMMAND = "bba_message"
)

var _ core.Module = (*Process)(nil)

// agreementWaiter holds channels to return a consensus result synchronously.
type agreementWaiter struct {
	decisionChan chan bool
	errChan      chan error
}

// BBAInstance represents a specific BBA consensus session.
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

// Process manages all BBA sessions and pending messages.
type Process struct {
	host            core.Host
	instances       map[string]*BBAInstance
	pendingMessages map[string][]*pb.BBAMessage // Buffer for uninitialized sessions
	waiters         map[string]*agreementWaiter // Map to store waiters by sessionID
	mu              sync.RWMutex
}

// NewProcess initializes the BBA Process.
func NewProcess(host core.Host) (*Process, error) {
	return &Process{
		host:            host,
		instances:       make(map[string]*BBAInstance),
		pendingMessages: make(map[string][]*pb.BBAMessage), // Initialize buffer
		waiters:         make(map[string]*agreementWaiter), // Initialize waiters map
	}, nil
}

// CommandHandlers implements the core.Module interface.
func (p *Process) CommandHandlers() map[string]func(t_network.Request) error {
	return map[string]func(t_network.Request) error{
		BBA_COMMAND: p.handleNetworkRequest,
	}
}

// Start implements the core.Module interface.
func (p *Process) Start() {
	logger.Info("BBA Process started.")
}

// Stop implements the core.Module interface.
func (p *Process) Stop() {
	logger.Info("BBA Process stopping.")
}

// handleNetworkRequest is the entry point for all BBA messages from the network.
func (p *Process) handleNetworkRequest(req t_network.Request) error {
	msg := &pb.BBAMessage{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		logger.Error("Failed to unmarshal BBA message: %v", err)
		return err
	}

	p.mu.Lock()
	instance, ok := p.instances[msg.SessionId]
	if !ok {
		// If session doesn't exist, buffer the message
		logger.Warn("Received BBA message for unknown session '%s'. Buffering message.", msg.SessionId)
		p.pendingMessages[msg.SessionId] = append(p.pendingMessages[msg.SessionId], msg)
		p.mu.Unlock()
		return nil
	}
	p.mu.Unlock()

	// If session exists, process normally
	return instance.handleMessage(msg)
}

// StartAgreementAndWait starts a BBA session and blocks until a decision is made or timeout occurs.
func (p *Process) StartAgreementAndWait(sessionID string, value bool, timeout time.Duration) (bool, error) {
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + sessionID + ".log")

	p.mu.Lock()
	if _, exists := p.instances[sessionID]; exists {
		p.mu.Unlock()
		fileLogger.Info("BBA instance for session %s already exists", sessionID)
		return false, fmt.Errorf("BBA instance for session %s already exists", sessionID)
	}

	// Create a new waiter for this session
	waiter := &agreementWaiter{
		decisionChan: make(chan bool, 1),
		errChan:      make(chan error, 1),
	}
	p.waiters[sessionID] = waiter
	p.mu.Unlock()

	// Start the consensus process (asynchronously)
	p.StartAgreement(sessionID, value)

	// Wait for the result or timeout
	select {
	case decision := <-waiter.decisionChan:
		return decision, nil
	case err := <-waiter.errChan:
		return false, err
	case <-time.After(timeout):
		return false, fmt.Errorf("BBA agreement for session '%s' timed out after %v", sessionID, timeout)
	}
}

// StartAgreement starts a new consensus session (asynchronously).
func (p *Process) StartAgreement(sessionID string, value bool) {
	p.mu.Lock()
	if _, exists := p.instances[sessionID]; exists {
		p.mu.Unlock()
		logger.Warn("BBA instance for session %s already exists", sessionID)
		return
	}
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + sessionID + ".log")

	// Create new instance
	instance := p.NewBBAInstance(sessionID)
	p.instances[sessionID] = instance

	// Check buffer for any pending messages for this session
	if pending, found := p.pendingMessages[sessionID]; found {
		fileLogger.Info("Processing %d buffered messages for new BBA session '%s'", len(pending), sessionID)
		for _, msg := range pending {
			// Pass pending messages to the new instance for processing
			go instance.handleMessage(msg)
		}
		// Clean up processed messages from the buffer
		delete(p.pendingMessages, sessionID)
	}
	p.mu.Unlock()

	logger.Info("Node %d starting BBA for session '%s' with value %v", p.host.ID(), sessionID, value)
	instance.inputValue(value)
}

// NewBBAInstance creates a new BBAInstance.
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
		logger.Info("Node %d BBA session '%s': Buffering message for epoch %d (current is %d)", bba.nodeID, bba.sessionID, msg.Epoch, bba.epoch)
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
			finalDecision := bba.decision.(bool)
			logger.Info("✅ Node %d BBA session '%s' is DONE. Final Decision: %v", bba.nodeID, bba.sessionID, finalDecision)

			// Find and send result to the waiter
			bba.process.mu.Lock()
			if waiter, ok := bba.process.waiters[bba.sessionID]; ok {
				waiter.decisionChan <- finalDecision
				delete(bba.process.waiters, bba.sessionID) // Clean up waiter
			}
			bba.process.mu.Unlock()
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
		logger.Info("Node %d BBA session '%s': Processing %d buffered messages for epoch %d", bba.nodeID, bba.sessionID, len(bufferedMsgs), bba.epoch)
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
