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
// MODIFIED: This function is now non-blocking and uses goroutines for processing.
func (p *Process) handleNetworkRequest(req t_network.Request) error {
	msg := &pb.BBAMessage{}
	if err := proto.Unmarshal(req.Message().Body(), msg); err != nil {
		logger.Error("Failed to unmarshal BBA message: %v", err)
		return err
	}

	go func(msg *pb.BBAMessage) {
		p.mu.RLock()
		instance, ok := p.instances[msg.SessionId]
		p.mu.RUnlock()

		if ok {
			// If the instance exists, let it handle the message.
			instance.handleMessage(msg)
		} else {
			// If the instance doesn't exist, buffer the message.
			p.mu.Lock()
			// Double-check if instance was created while waiting for the lock.
			if instance, ok := p.instances[msg.SessionId]; ok {
				p.mu.Unlock()
				instance.handleMessage(msg)
				return
			}
			p.pendingMessages[msg.SessionId] = append(p.pendingMessages[msg.SessionId], msg)
			p.mu.Unlock()
		}
	}(msg)

	return nil
}

// ADDED: A helper function to parse round number from sessionID.
func parseRoundFromSessionID(sessionID string) (uint64, error) {
	parts := strings.Split(sessionID, "_")
	if len(parts) < 3 || parts[0] != "agreement" || parts[1] != "round" {
		return 0, fmt.Errorf("invalid sessionID format: %s", sessionID)
	}
	round, err := strconv.ParseUint(parts[2], 10, 64)
	if err != nil {
		return 0, fmt.Errorf("failed to parse round number '%s': %w", parts[2], err)
	}
	return round, nil
}

// StartAgreementAndWait starts a BBA session and blocks until a decision is made or timeout occurs.
func (p *Process) StartAgreementAndWait(sessionID string, value bool, timeout time.Duration) (bool, error) {
	p.mu.Lock()
	if _, exists := p.instances[sessionID]; exists {
		p.mu.Unlock()
		return false, fmt.Errorf("BBA instance for session %s already exists", sessionID)
	}

	waiter := &agreementWaiter{
		decisionChan: make(chan bool, 1),
		errChan:      make(chan error, 1),
	}
	p.waiters[sessionID] = waiter
	p.mu.Unlock()

	// StartAgreement is non-blocking
	p.StartAgreement(sessionID, value)

	select {
	case decision := <-waiter.decisionChan:
		return decision, nil
	case err := <-waiter.errChan:
		return false, err
	case <-time.After(timeout):
		p.mu.Lock()
		delete(p.waiters, sessionID)
		p.mu.Unlock()
		return false, fmt.Errorf("BBA agreement for session '%s' timed out after %v", sessionID, timeout)
	}
}

// StartAgreement starts a new consensus session (asynchronously).
// MODIFIED: Extracts round number, updates highest round, and passes it to NewBBAInstance.
// The core logic is now non-blocking.
func (p *Process) StartAgreement(sessionID string, value bool) {
	p.mu.Lock()
	defer p.mu.Unlock()

	if _, exists := p.instances[sessionID]; exists {
		return
	}

	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d/", p.host.ID()) + sessionID + ".log")

	round, err := parseRoundFromSessionID(sessionID)
	if err != nil {
		return
	}
	if round > p.highestRound {
		p.highestRound = round
	}

	instance := p.NewBBAInstance(sessionID, round, fileLogger)
	p.instances[sessionID] = instance

	// Process any buffered messages for this new session in a goroutine
	go func() {
		p.mu.Lock()
		if pending, found := p.pendingMessages[sessionID]; found {
			for _, msg := range pending {
				// The message handler itself is now safe for concurrent calls
				go instance.handleMessage(msg)
			}
			delete(p.pendingMessages, sessionID)
		}
		p.mu.Unlock()

		// Start the BBA process for this instance
		instance.inputValue(value)
	}()
}

// NewBBAInstance creates a new BBAInstance.
// MODIFIED: Accepts round number and file logger as parameters.
func (p *Process) NewBBAInstance(sessionID string, round uint64, fileLogger *loggerfile.FileLogger) *BBAInstance {
	config := p.host.Config()

	return &BBAInstance{
		process:        p,
		sessionID:      sessionID,
		round:          round,
		N:              config.NumValidator,
		F:              (config.NumValidator - 1) / 3,
		nodeID:         p.host.ID(),
		recvBval:       make(map[int32]bool),
		recvAux:        make(map[int32]bool),
		sentBvals:      []bool{},
		binValues:      []bool{},
		futureMessages: make(map[int32][]*pb.BBAMessage),
		fileLogger:     fileLogger,
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

	// Create a copy of the value to avoid race conditions in the goroutine
	bvalMsg := &pb.BBAMessage{
		SessionId: bba.sessionID,
		Epoch:     int32(bba.epoch),
		SenderId:  bba.nodeID,
		Content:   &pb.BBAMessage_BvalRequest{BvalRequest: &pb.BvalRequest{Value: val}},
	}
	bba.broadcast(bvalMsg)
	bba.handleBvalRequest(bba.nodeID, val)
}

func (bba *BBAInstance) handleMessage(msg *pb.BBAMessage) {
	bba.lock.Lock()
	defer bba.lock.Unlock()

	if msg.Epoch > int32(bba.epoch) {
		bba.futureMessages[msg.Epoch] = append(bba.futureMessages[msg.Epoch], msg)
		return
	}

	if msg.Epoch < int32(bba.epoch) {
		return
	}

	bba.processMessage(msg)
}

func (bba *BBAInstance) processMessage(msg *pb.BBAMessage) {
	switch content := msg.Content.(type) {
	case *pb.BBAMessage_BvalRequest:
		bba.handleBvalRequest(msg.SenderId, content.BvalRequest.Value)
	case *pb.BBAMessage_AuxRequest:
		bba.handleAuxRequest(msg.SenderId, content.AuxRequest.Value)
	default:
		logger.Error("unknown BBA message content for session %s", bba.sessionID)
	}
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
	if bba.decision != nil {
		return // Already decided
	}

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

	if len(values) == 0 {
		bba.advanceEpoch()
		return
	}

	// Using modulo on the round number for the coin flip to ensure determinism
	coin := (bba.round+uint64(bba.epoch))%2 == 0

	if len(values) == 1 {
		v := values[0]
		if v == coin {
			bba.decision = v
			logger.Info("🏆 Node %d BBA session '%s' DECIDED on %v at epoch %d", bba.nodeID, bba.sessionID, bba.decision, bba.epoch)

			bba.process.mu.Lock()
			if waiter, ok := bba.process.waiters[bba.sessionID]; ok {
				waiter.decisionChan <- bba.decision.(bool)
				close(waiter.decisionChan)
				close(waiter.errChan)
				delete(bba.process.waiters, bba.sessionID)
			}
			bba.process.mu.Unlock()
			return // Stop processing after a decision
		} else {
			bba.estimated = v
		}
	} else { // len(values) > 1 or (len(values)==1 and values[0]!=coin)
		bba.estimated = coin
	}

	bba.advanceEpoch()
}

func (bba *BBAInstance) advanceEpoch() {
	if bba.decision != nil {
		return
	}

	bba.epoch++

	bba.recvBval = make(map[int32]bool)
	bba.recvAux = make(map[int32]bool)
	bba.sentBvals = []bool{}
	bba.binValues = []bool{}

	// Handle buffered messages for the new epoch
	if bufferedMsgs, found := bba.futureMessages[int32(bba.epoch)]; found {
		for _, msg := range bufferedMsgs {
			bba.processMessage(msg) // Already under lock
		}
		delete(bba.futureMessages, int32(bba.epoch))
	}

	// If an estimate is set, broadcast it for the new epoch
	if bba.estimated != nil {
		est := bba.estimated.(bool)
		bba.sentBvals = append(bba.sentBvals, est)
		bba.broadcastBval(est)
		bba.handleBvalRequest(bba.nodeID, est)
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
