package rbc

import (
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"log"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/aleaqueues"
	m_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/core"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/loggerfile"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/network"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
	"google.golang.org/protobuf/proto"
)

const (
	RBC_COMMAND         = "rbc_message"
	DataTypeBatch       = "batch"
	DataTypeTransaction = "transaction"
	DataTypeVote        = "vote"
)

type ProposalEvent struct {
	NodeID string
	Value  bool
}

type broadcastState struct {
	mu          sync.Mutex
	echoRecvd   map[int32]bool
	readyRecvd  map[int32]bool
	sentEcho    bool
	sentReady   bool
	delivered   bool
	payload     []byte
	BlockNumber uint64
}

type ProposalNotification struct {
	SenderID int32
	Priority int64
	Payload  []byte
}

type Process struct {
	host               core.Host
	N                  int
	F                  int
	Delivered          chan *ProposalNotification
	logs               map[string]*broadcastState
	logsMu             sync.RWMutex
	queueManager       *aleaqueues.QueueManager
	MessageSender      t_network.MessageSender
	PoolTransactions   chan *pb.Batch
	blockNumberChan    chan uint64
	currentBlockNumber uint64
	proposalChannel    chan ProposalEvent
	votesByBlockNumber map[uint64][]*pb.VoteRequest
	votesMutex         sync.RWMutex
	voteSubscribers    map[uint64]map[chan<- *pb.VoteRequest]struct{}
	subscribersMutex   sync.RWMutex
}

// NewProcess khởi tạo RBC Process, nhận Host làm dependency.
func NewProcess(host core.Host) (*Process, error) {
	p := &Process{
		host:               host,
		N:                  len(host.Config().Peers),
		F:                  (len(host.Config().Peers) - 1) / 3,
		Delivered:          make(chan *ProposalNotification, 1024),
		logs:               make(map[string]*broadcastState),
		MessageSender:      network.NewMessageSender(""),
		PoolTransactions:   make(chan *pb.Batch, 1024),
		blockNumberChan:    make(chan uint64, 1024),
		proposalChannel:    make(chan ProposalEvent, 1024),
		votesByBlockNumber: make(map[uint64][]*pb.VoteRequest),
		voteSubscribers:    make(map[uint64]map[chan<- *pb.VoteRequest]struct{}),
	}
	peerIDs := make([]int32, 0, len(host.Config().Peers))
	for _, peerConf := range host.Config().Peers {
		peerIDs = append(peerIDs, int32(peerConf.Id))
	}
	p.queueManager = aleaqueues.NewQueueManager(peerIDs)
	return p, nil
}

// CommandHandlers implement interface core.Module
func (p *Process) CommandHandlers() map[string]func(t_network.Request) error {
	return map[string]func(t_network.Request) error{
		RBC_COMMAND: p.handleNetworkRequest,
		m_common.SendPoolTransactons: func(req t_network.Request) error {
			batch := &pb.Batch{}
			if err := proto.Unmarshal(req.Message().Body(), batch); err == nil {
				p.PoolTransactions <- batch
				return nil
			} else {
				panic("Error Unmarshal batch in SendPoolTransactons")
			}
		},
		m_common.BlockNumber: func(req t_network.Request) error {
			responseData := req.Message().Body()
			if len(responseData) < 8 {
				return fmt.Errorf("invalid block number response data length")
			}
			validatorBlockNumber := binary.BigEndian.Uint64(responseData)
			p.blockNumberChan <- validatorBlockNumber
			return nil
		},
	}
}

// Start implement interface core.Module
func (p *Process) Start() {
	go func() {
		fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d", p.host.ID()) + ".log")
		for blockNumber := range p.blockNumberChan {
			time.Sleep(20 * time.Millisecond)
			fileLogger.Info("blockNumberChan: %v", blockNumber)

			isMyTurnForNextBlock := ((int(blockNumber+1) + p.host.Config().NumValidator - 1) % p.host.Config().NumValidator) == (int(p.host.Config().ID) - 1)
			if isMyTurnForNextBlock {
				fileLogger.Info("Đến lượt mình đề xuất cho block %d. Đang yêu cầu transactions...", blockNumber+1)
				p.MessageSender.SendBytes(
					p.host.MasterConn(),
					m_common.GetTransactionsPool,
					[]byte{},
				)
			}

			fileLogger.Info("--------------------------------------------------")
			fileLogger.Info("⚡ Bắt đầu xử lý cho block: %d", blockNumber)
			p.UpdateBlockNumber(blockNumber)

			remainder := int(blockNumber)%p.host.Config().NumValidator + 1
			var payload []byte
			var err error
			for {
				payload, err = p.queueManager.GetByPriority(int32(remainder), int64(blockNumber+1))
				if err == nil {
					break
				}
				time.Sleep(10 * time.Millisecond)
			}

			consensusDecision := true
			if consensusDecision && payload != nil {
				batch := &pb.Batch{}
				if proto.Unmarshal(payload, batch) == nil {
					fileLogger.Info("Đã gửi giao dịch của batch")
					fileLogger.Info("PushFinalizeEvent 1 block: %d - %d : %v ", batch.BlockNumber, blockNumber+1, consensusDecision)
					fileLogger.Info("PushFinalizeEvent 1 %v ", batch.Transactions)
					err := p.MessageSender.SendBytes(p.host.MasterConn(), m_common.PushFinalizeEvent, payload)
					if err != nil {
						panic(err)
					}
					logger.Info("Đã gửi PushFinalizeEvent cho block %d : %v", blockNumber+1, consensusDecision)

				} else {
					logger.Info("Đã gửi giao dịch batch rỗng")
					batch := &pb.Batch{BlockNumber: blockNumber + 1}
					batchBytes, _ := proto.Marshal(batch)
					fileLogger.Info("PushFinalizeEvent 2 block: %d : %v ", blockNumber+1, consensusDecision)
					err := p.MessageSender.SendBytes(p.host.MasterConn(), m_common.PushFinalizeEvent, batchBytes)
					if err != nil {
						panic(err)
					}
				}
			} else {
				logger.Info("Đã gửi giao dịch batch rỗng")
				batch := &pb.Batch{BlockNumber: blockNumber + 1}
				batchBytes, _ := proto.Marshal(batch)
				fileLogger.Info("PushFinalizeEvent 3 block: %d : %v", blockNumber+1, consensusDecision)
				err := p.MessageSender.SendBytes(p.host.MasterConn(), m_common.PushFinalizeEvent, batchBytes)
				if err != nil {
					panic(err)
				}
			}

			p.CleanupOldMessages()
		}
	}()

	p.HandleDelivered()
	p.HandlePoolTransactions()
	time.Sleep(10 * time.Second)
	p.RequestInitialBlockNumber()
}

// Stop implement interface core.Module
func (p *Process) Stop() {
	logger.Info("RBC Process stopping...")
}

func (p *Process) handleNetworkRequest(req t_network.Request) error {
	var msg pb.RBCMessage
	if err := proto.Unmarshal(req.Message().Body(), &msg); err != nil {
		logger.Error("Error unmarshalling RBCMessage: %v", err)
		return err
	}
	go p.handleMessage(&msg)
	return nil
}

func (p *Process) broadcast(msg *pb.RBCMessage) {
	msg.NetworkSenderId = p.host.ID()
	payload, err := proto.Marshal(msg)
	if err != nil {
		logger.Error("Node %d: Failed to marshal RBC message: %v", p.host.ID(), err)
		return
	}
	go p.handleMessage(msg)
	p.host.Broadcast(RBC_COMMAND, payload)
}

func (p *Process) StartBroadcast(payload []byte, dataType string, messageType pb.MessageType) {
	messageID := fmt.Sprintf("%d-%d", p.host.ID(), time.Now().UnixNano())
	logger.Info("Node %d starting broadcast for message %s", p.host.ID(), messageID)
	initMsg := &pb.RBCMessage{
		Type:             messageType,
		OriginalSenderId: p.host.ID(),
		MessageId:        messageID,
		Payload:          payload,
		DataType:         dataType,
	}
	p.broadcast(initMsg)
}

func (p *Process) handleMessage(msg *pb.RBCMessage) {
	key := fmt.Sprintf("%d-%s", msg.OriginalSenderId, msg.MessageId)
	state := p.getOrCreateState(key, msg.Payload)

	state.mu.Lock()
	defer state.mu.Unlock()

	switch msg.Type {
	case pb.MessageType_SEND:
		if msg.DataType == DataTypeVote {
			receivedVote := &pb.VoteRequest{}
			if err := proto.Unmarshal(msg.Payload, receivedVote); err != nil {
				log.Fatalf("Lỗi khi unmarshal (deserialize): %v", err)
			}
			logger.Error("receivedVote: %v", receivedVote)

			p.votesMutex.Lock()
			p.votesByBlockNumber[receivedVote.BlockNumber] = append(p.votesByBlockNumber[receivedVote.BlockNumber], receivedVote)
			p.votesMutex.Unlock()

			p.subscribersMutex.RLock()
			if receivedVote.BlockNumber > p.currentBlockNumber {
				if subscribers, found := p.voteSubscribers[receivedVote.BlockNumber]; found {
					for subChan := range subscribers {
						select {
						case subChan <- receivedVote:
						default:
						}
					}
				}
			}
			p.subscribersMutex.RUnlock()
		} else {
			logger.Info(string(msg.Payload))
		}
	case pb.MessageType_INIT:
		if !state.sentEcho {
			state.sentEcho = true
			logger.Info("Node %d received INIT, sending ECHO for message %s", p.host.ID(), key)
			echoMsg := &pb.RBCMessage{
				Type:             pb.MessageType_ECHO,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
				DataType:         msg.DataType,
			}
			p.broadcast(echoMsg)
		}
	case pb.MessageType_ECHO:
		state.echoRecvd[msg.NetworkSenderId] = true
		if len(state.echoRecvd) > (p.N+p.F)/2 && !state.sentReady {
			state.sentReady = true
			logger.Info("Node %d has enough ECHOs, sending READY for message %s", p.host.ID(), key)
			readyMsg := &pb.RBCMessage{
				Type:             pb.MessageType_READY,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
				DataType:         msg.DataType,
			}
			p.broadcast(readyMsg)
		}
	case pb.MessageType_READY:
		state.readyRecvd[msg.NetworkSenderId] = true
		if len(state.readyRecvd) > p.F && !state.sentReady {
			state.sentReady = true
			logger.Info("Node %d received f+1 READYs, amplifying READY for message %s", p.host.ID(), key)
			readyMsg := &pb.RBCMessage{
				Type:             pb.MessageType_READY,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
				DataType:         msg.DataType,
			}
			p.broadcast(readyMsg)
		}
		if len(state.readyRecvd) > 2*p.F && !state.delivered {
			state.delivered = true
			logger.Info("Node %d has DELIVERED message %s", p.host.ID(), key)
			proposerID := msg.OriginalSenderId
			var notification *ProposalNotification
			switch msg.DataType {
			case DataTypeBatch:
				batch := &pb.Batch{}
				if err := proto.Unmarshal(state.payload, batch); err == nil {
					priority := int64(batch.BlockNumber)
					p.queueManager.Enqueue(proposerID, priority, state.payload)
					notification = &ProposalNotification{
						SenderID: proposerID,
						Priority: priority,
						Payload:  state.payload,
					}
					if notification != nil {
						p.Delivered <- notification
					}
				} else {
					notification = &ProposalNotification{SenderID: proposerID, Priority: -1, Payload: state.payload}
				}
			default:
				notification = &ProposalNotification{SenderID: proposerID, Priority: -1, Payload: state.payload}
			}
		}
	}
}

// ... Các hàm helper còn lại (getOrCreateState, HandleDelivered, etc.) giữ nguyên ...
// (Lưu ý: đã được điều chỉnh trong các ví dụ trước để dùng p.host thay vì p.node)

func (p *Process) getOrCreateState(key string, payload []byte) *broadcastState {
	p.logsMu.Lock()
	defer p.logsMu.Unlock()
	state, exists := p.logs[key]
	if !exists {
		batch := &pb.Batch{}
		var blockNum uint64 = 0
		if proto.Unmarshal(payload, batch) == nil {
			blockNum = batch.GetBlockNumber()
		}
		state = &broadcastState{
			echoRecvd:   make(map[int32]bool),
			readyRecvd:  make(map[int32]bool),
			payload:     payload,
			BlockNumber: blockNum,
		}
		p.logs[key] = state
	}
	return state
}
func (p *Process) HandleDelivered() {
	pendingPayloads := make(map[uint64]*ProposalNotification)
	var mu sync.Mutex
	processBatch := func(payload *ProposalNotification) {
		logger.Info("\n[APPLICATION] Node %d Delivered Batch for Block %d from Proposer %x\n> ", p.host.ID(), payload.Priority, payload.SenderID)
	}

	go func() {
		for payload := range p.Delivered {
			batch := &pb.Batch{}
			if err := proto.Unmarshal(payload.Payload, batch); err != nil {
				logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", p.host.ID(), string(payload.Payload))
				continue
			}

			mu.Lock()
			currentExpectedBlock := p.GetCurrentBlockNumber() + 1
			if payload.Priority == int64(currentExpectedBlock) {
				processBatch(payload)
				nextBlock := currentExpectedBlock + 1
				for {
					if pendingPayload, found := pendingPayloads[nextBlock]; found {
						processBatch(pendingPayload)
						delete(pendingPayloads, nextBlock)
						nextBlock++
					} else {
						break
					}
				}
			} else if payload.Priority > int64(currentExpectedBlock) {
				priorityKey := uint64(payload.Priority)
				if _, found := pendingPayloads[priorityKey]; !found {
					logger.Info("\n[APPLICATION] Node %d received future block %d, pending.\n> ", p.host.ID(), payload.Priority)
					pendingPayloads[priorityKey] = payload
				}
			}
			mu.Unlock()
		}
	}()
}
func (p *Process) HandlePoolTransactions() {
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d", p.host.ID()) + ".log")
	go func() {
		for txs := range p.PoolTransactions {
			// FIXME: Replace with actual public key from host/keypair management
			proposerId := []byte("temp-proposer-key")
			headerData := fmt.Sprintf("%d:%x", p.GetCurrentBlockNumber()+1, proposerId)
			batchHash := sha256.Sum256([]byte(headerData))
			batch := &pb.Batch{
				Hash:         batchHash[:],
				Transactions: txs.Transactions,
				BlockNumber:  txs.BlockNumber,
				ProposerId:   proposerId,
			}
			payload, err := proto.Marshal(batch)
			if err != nil {
				fileLogger.Info("Failed to marshal batch:", err)
				continue
			}
			fileLogger.Info("\n[APPLICATION] Node %d broadcasting a batch for block %d...\n> ", p.host.ID(), p.GetCurrentBlockNumber()+1)
			p.StartBroadcast(payload, "batch", pb.MessageType_INIT)
		}
	}()
}
func (p *Process) RequestInitialBlockNumber() {
	go p.MessageSender.SendBytes(p.host.MasterConn(), m_common.ValidatorGetBlockNumber, []byte{})
}
func (p *Process) CleanupOldMessages() {
	p.logsMu.Lock()
	defer p.logsMu.Unlock()
	currentBlock := p.GetCurrentBlockNumber()
	if currentBlock <= 1000 {
		return
	}
	cleanupThreshold := currentBlock - 1000
	cleanedCount := 0
	for key, state := range p.logs {
		if state.delivered && state.BlockNumber > 0 && state.BlockNumber < cleanupThreshold {
			delete(p.logs, key)
			cleanedCount++
		}
	}
	if cleanedCount > 0 {
		logger.Info("Node %d CLEANED UP %d old message states for blocks older than %d", p.host.ID(), cleanedCount, cleanupThreshold)
	}
}
func (p *Process) PushProposalEvent(event ProposalEvent) {
	p.proposalChannel <- event
}
func (p *Process) UpdateBlockNumber(blockNumber uint64) {
	p.currentBlockNumber = blockNumber
}
func (p *Process) GetCurrentBlockNumber() uint64 {
	return p.currentBlockNumber
}
