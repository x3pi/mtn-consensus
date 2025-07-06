package bba

import (
	"fmt"
	"strconv"
	"strings"
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
	// ADDED: Constants for cleanup mechanism
	CLEANUP_THRESHOLD = 500             // Keep instances for this many rounds before they are eligible for cleanup.
	CLEANUP_INTERVAL  = 5 * time.Minute // How often to run the cleanup process.
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
	decision       interface{}
	estimated      interface{}
	futureMessages map[int32][]*pb.BBAMessage
	round          uint64                 // ADDED: Store the round number for this instance
	fileLogger     *loggerfile.FileLogger // ADDED: File logger for this session
	lock           sync.RWMutex
}

// Process manages all BBA sessions and pending messages.
type Process struct {
	host            core.Host
	instances       map[string]*BBAInstance
	pendingMessages map[string][]*pb.BBAMessage
	waiters         map[string]*agreementWaiter
	mu              sync.RWMutex
	highestRound    uint64        // ADDED: Track the highest round number seen
	doneChan        chan struct{} // ADDED: Channel to stop the cleanup goroutine
}

// NewProcess initializes the BBA Process.
func NewProcess(host core.Host) (*Process, error) {
	return &Process{
		host:            host,
		instances:       make(map[string]*BBAInstance),
		pendingMessages: make(map[string][]*pb.BBAMessage),
		waiters:         make(map[string]*agreementWaiter),
		doneChan:        make(chan struct{}), // ADDED: Initialize the done channel
	}, nil
}

// CommandHandlers implements the core.Module interface.
func (p *Process) CommandHandlers() map[string]func(t_network.Request) error {
	return map[string]func(t_network.Request) error{
		BBA_COMMAND: p.handleNetworkRequest,
	}
}

// Start implements the core.Module interface.
// MODIFIED: Starts the background cleanup process.
func (p *Process) Start() {
	logger.Info("BBA Process started.")
	go p.startCleanupTicker()
}

// Stop implements the core.Module interface.
// MODIFIED: Stops the background cleanup process.
func (p *Process) Stop() {
	logger.Info("BBA Process stopping.")
	close(p.doneChan)
}

// ADDED: Goroutine that periodically cleans up old BBA instances.
func (p *Process) startCleanupTicker() {
	ticker := time.NewTicker(CLEANUP_INTERVAL)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			p.cleanupOldInstances()
		case <-p.doneChan:
			return
		}
	}
}

// ADDED: The core logic for finding and deleting old BBA instances.
func (p *Process) cleanupOldInstances() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.highestRound <= CLEANUP_THRESHOLD {
		return // Not enough rounds have passed to start cleaning up
	}

	// Calculate the round number below which instances are considered old
	cleanupLimit := p.highestRound - CLEANUP_THRESHOLD
	deletedCount := 0

	for sessionID, instance := range p.instances {
		if instance.round < cleanupLimit {
			delete(p.instances, sessionID)
			deletedCount++
		}
	}

	if deletedCount > 0 {
		logger.Info("BBA Cleanup: Removed %d old instances for rounds older than %d", deletedCount, cleanupLimit)
	}
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
		logger.Warn("Received BBA message for unknown session '%s'. Buffering message.", msg.SessionId)
		p.pendingMessages[msg.SessionId] = append(p.pendingMessages[msg.SessionId], msg)
		p.mu.Unlock()
		return nil
	}
	p.mu.Unlock()

	return instance.handleMessage(msg)
}

// ADDED: A helper function to parse round number from sessionID.
func parseRoundFromSessionID(sessionID string) (uint64, error) {
	// Tạo file logger với format tên file tương tự như các hàm khác
	fileLogger, _ := loggerfile.NewFileLogger("Note_parseRound_" + sessionID + ".log")

	fileLogger.Info(" Parsing round from sessionID: '%s'", sessionID)

	parts := strings.Split(sessionID, "_")
	fileLogger.Info("📝 Split sessionID into %d parts: %v", len(parts), parts)

	if len(parts) < 3 || parts[0] != "agreement" || parts[1] != "round" {
		fileLogger.Info("❌ Invalid sessionID format: expected 'agreement_round_<number>', got '%s'", sessionID)
		return 0, fmt.Errorf("invalid sessionID format: %s", sessionID)
	}

	roundStr := parts[2]
	fileLogger.Info("🔢 Extracted round string: '%s'", roundStr)

	round, err := strconv.ParseUint(roundStr, 10, 64)
	if err != nil {
		fileLogger.Info("❌ Failed to parse round number '%s': %v", roundStr, err)
		return 0, err
	}

	fileLogger.Info("✅ Successfully parsed round number: %d", round)
	return round, nil
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

	waiter := &agreementWaiter{
		decisionChan: make(chan bool, 1),
		errChan:      make(chan error, 1),
	}
	p.waiters[sessionID] = waiter
	p.mu.Unlock()

	p.StartAgreement(sessionID, value)

	select {
	case decision := <-waiter.decisionChan:
		return decision, nil
	case err := <-waiter.errChan:
		return false, err
	case <-time.After(timeout):
		// Clean up the waiter on timeout to prevent memory leak
		p.mu.Lock()
		delete(p.waiters, sessionID)
		p.mu.Unlock()
		return false, fmt.Errorf("BBA agreement for session '%s' timed out after %v", sessionID, timeout)
	}
}

// StartAgreement starts a new consensus session (asynchronously).
// MODIFIED: Extracts round number, updates highest round, and passes it to NewBBAInstance.
func (p *Process) StartAgreement(sessionID string, value bool) {
	p.mu.Lock()
	if _, exists := p.instances[sessionID]; exists {
		p.mu.Unlock()
		logger.Warn("BBA instance for session %s already exists", sessionID)
		return
	}
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + sessionID + ".log")

	// ADDED: Parse round, update highest round
	round, err := parseRoundFromSessionID(sessionID)
	if err != nil {
		p.mu.Unlock()
		fileLogger.Info("Could not start BBA: %v", err)
		return
	}
	if round > p.highestRound {
		p.highestRound = round
	}

	// Create new instance, passing the round number
	instance := p.NewBBAInstance(sessionID, round)
	p.instances[sessionID] = instance

	if pending, found := p.pendingMessages[sessionID]; found {
		fileLogger.Info("Processing %d buffered messages for new BBA session '%s'", len(pending), sessionID)
		for _, msg := range pending {
			go instance.handleMessage(msg)
		}
		delete(p.pendingMessages, sessionID)
	}
	p.mu.Unlock()

	logger.Info("Node %d starting BBA for session '%s' with value %v", p.host.ID(), sessionID, value)
	instance.inputValue(value)
}

// NewBBAInstance creates a new BBAInstance.
// MODIFIED: Accepts round number as a parameter and initializes fileLogger.
func (p *Process) NewBBAInstance(sessionID string, round uint64) *BBAInstance {
	config := p.host.Config()
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + sessionID + ".log")

	return &BBAInstance{
		process:        p,
		sessionID:      sessionID,
		round:          round, // ADDED: Set the round number
		N:              config.NumValidator,
		F:              (config.NumValidator - 1) / 3,
		nodeID:         p.host.ID(),
		recvBval:       make(map[int32]bool),
		recvAux:        make(map[int32]bool),
		sentBvals:      []bool{},
		binValues:      []bool{},
		futureMessages: make(map[int32][]*pb.BBAMessage),
		fileLogger:     fileLogger, // ADDED: Set the file logger
	}
}

// ... (Rest of the file remains the same) ...
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

	// ADDED: Log the initial input value
	bba.fileLogger.Info("Node %d BBA session '%s': Input value set to %v", bba.nodeID, bba.sessionID, val)

	bba.handleBvalRequest(bba.nodeID, val)
}

func (bba *BBAInstance) handleMessage(msg *pb.BBAMessage) error {
	bba.lock.Lock()
	defer bba.lock.Unlock()

	if msg.Epoch > int32(bba.epoch) {
		// ADDED: Log buffering of future message
		bba.fileLogger.Info("Node %d BBA session '%s': Buffering message for epoch %d (current is %d)", bba.nodeID, bba.sessionID, msg.Epoch, bba.epoch)
		bba.futureMessages[msg.Epoch] = append(bba.futureMessages[msg.Epoch], msg)
		return nil
	}

	if msg.Epoch < int32(bba.epoch) {
		// ADDED: Log dropping of old message
		bba.fileLogger.Info("Node %d BBA session '%s': Dropping old message from epoch %d (current is %d)", bba.nodeID, bba.sessionID, msg.Epoch, bba.epoch)
		return nil
	}

	return bba.processMessage(msg)
}

func (bba *BBAInstance) processMessage(msg *pb.BBAMessage) error {
	switch content := msg.Content.(type) {
	case *pb.BBAMessage_BvalRequest:
		// ADDED: Log received BVAL message
		bba.fileLogger.Info("Node %d BBA session '%s': Received BVAL from node %d with value %v", bba.nodeID, bba.sessionID, msg.SenderId, content.BvalRequest.Value)
		bba.handleBvalRequest(msg.SenderId, content.BvalRequest.Value)
	case *pb.BBAMessage_AuxRequest:
		// ADDED: Log received AUX message
		bba.fileLogger.Info("Node %d BBA session '%s': Received AUX from node %d with value %v", bba.nodeID, bba.sessionID, msg.SenderId, content.AuxRequest.Value)
		bba.handleAuxRequest(msg.SenderId, content.AuxRequest.Value)
	default:
		return fmt.Errorf("unknown BBA message content")
	}
	return nil
}

func (bba *BBAInstance) handleBvalRequest(senderID int32, val bool) {
	if _, exists := bba.recvBval[senderID]; exists {
		// ADDED: Log duplicate BVAL message
		bba.fileLogger.Info("Node %d BBA session '%s': Ignoring duplicate BVAL from node %d", bba.nodeID, bba.sessionID, senderID)
		return
	}
	bba.recvBval[senderID] = val
	count := 0
	for _, v := range bba.recvBval {
		if v == val {
			count++
		}
	}

	// ADDED: Log BVAL count
	bba.fileLogger.Info("Node %d BBA session '%s': BVAL count for value %v is %d/%d", bba.nodeID, bba.sessionID, val, count, bba.N)

	if count >= bba.F+1 && !bba.hasSentBval(val) {
		bba.sentBvals = append(bba.sentBvals, val)
		bba.broadcastBval(val)
		// ADDED: Log BVAL broadcast
		bba.fileLogger.Info("Node %d BBA session '%s': Broadcasting BVAL %v (threshold F+1=%d reached)", bba.nodeID, bba.sessionID, val, bba.F+1)
	}
	if count >= 2*bba.F+1 && !bba.isInBinValues(val) {
		bba.binValues = append(bba.binValues, val)
		// ADDED: Log bin value addition
		bba.fileLogger.Info("Node %d BBA session '%s': Added %v to bin values (threshold 2F+1=%d reached)", bba.nodeID, bba.sessionID, val, 2*bba.F+1)
		if len(bba.binValues) == 1 {
			bba.broadcastAux(val)
			bba.handleAuxRequest(bba.nodeID, val)
			// ADDED: Log AUX broadcast
			bba.fileLogger.Info("Node %d BBA session '%s': Broadcasting AUX %v (first bin value)", bba.nodeID, bba.sessionID, val)
		}
	}
}

func (bba *BBAInstance) handleAuxRequest(senderID int32, val bool) {
	if _, exists := bba.recvAux[senderID]; exists {
		// ADDED: Log duplicate AUX message
		bba.fileLogger.Info("Node %d BBA session '%s': Ignoring duplicate AUX from node %d", bba.nodeID, bba.sessionID, senderID)
		return
	}
	bba.recvAux[senderID] = val
	// ADDED: Log AUX count
	bba.fileLogger.Info("Node %d BBA session '%s': AUX count for value %v is %d/%d", bba.nodeID, bba.sessionID, val, len(bba.recvAux), bba.N)
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
	// ADDED: Log coin flip and values
	bba.fileLogger.Info("Node %d BBA session '%s': Coin flip is %v, values: %v", bba.nodeID, bba.sessionID, coin, values)

	if bba.decision == nil {
		if len(values) == 1 {
			if values[0] == coin {
				bba.decision = values[0]
				// ADDED: Log decision
				bba.fileLogger.Info("🏆 Node %d BBA session '%s' DECIDED on %v at epoch %d", bba.nodeID, bba.sessionID, bba.decision, bba.epoch-1)
				logger.Info("🏆 Node %d BBA session '%s' DECIDED on %v at epoch %d", bba.nodeID, bba.sessionID, bba.decision, bba.epoch-1)

				bba.process.mu.Lock()
				if waiter, ok := bba.process.waiters[bba.sessionID]; ok {
					waiter.decisionChan <- bba.decision.(bool)
					delete(bba.process.waiters, bba.sessionID)
				}
				bba.process.mu.Unlock()
			}
		}
	}

	bba.advanceEpoch()

	if len(values) == 1 {
		bba.estimated = values[0]
		// ADDED: Log estimated value update
		bba.fileLogger.Info("Node %d BBA session '%s': Estimated value set to %v (single value)", bba.nodeID, bba.sessionID, values[0])
	} else {
		if bba.decision != nil {
			bba.estimated = bba.decision
			// ADDED: Log estimated value update
			bba.fileLogger.Info("Node %d BBA session '%s': Estimated value set to decision %v", bba.nodeID, bba.sessionID, bba.decision)
		} else {
			bba.estimated = coin
			// ADDED: Log estimated value update
			bba.fileLogger.Info("Node %d BBA session '%s': Estimated value set to coin %v", bba.nodeID, bba.sessionID, coin)
		}
	}

	est := bba.estimated.(bool)
	bba.sentBvals = append(bba.sentBvals, est)
	bba.broadcastBval(est)
	// ADDED: Log next epoch BVAL broadcast
	bba.fileLogger.Info("Node %d BBA session '%s': Broadcasting BVAL %v for next epoch", bba.nodeID, bba.sessionID, est)
	bba.handleBvalRequest(bba.nodeID, est)
}

func (bba *BBAInstance) advanceEpoch() {
	bba.epoch++
	// ADDED: Log epoch advancement
	bba.fileLogger.Info("Node %d advancing BBA session '%s' to epoch %d", bba.nodeID, bba.sessionID, bba.epoch)
	logger.Info("Node %d advancing BBA session '%s' to epoch %d", bba.nodeID, bba.sessionID, bba.epoch)

	bba.recvBval = make(map[int32]bool)
	bba.recvAux = make(map[int32]bool)
	bba.sentBvals = []bool{}
	bba.binValues = []bool{}

	if bufferedMsgs, found := bba.futureMessages[int32(bba.epoch)]; found {
		// ADDED: Log processing of buffered messages
		bba.fileLogger.Info("Node %d BBA session '%s': Processing %d buffered messages for epoch %d", bba.nodeID, bba.sessionID, len(bufferedMsgs), bba.epoch)
		logger.Info("Node %d BBA session '%s': Processing %d buffered messages for epoch %d", bba.nodeID, bba.sessionID, len(bufferedMsgs), bba.epoch)
		for _, msg := range bufferedMsgs {
			// MODIFIED: Call handleMessage which acquires the lock, instead of processMessage.
			go bba.handleMessage(msg)
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
