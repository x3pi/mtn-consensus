package rbc

import (
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"log"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/aleaqueues"
	"github.com/meta-node-blockchain/meta-node/pkg/binaryagreement"
	m_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/loggerfile"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/network"
	"github.com/meta-node-blockchain/meta-node/pkg/node"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
	"google.golang.org/protobuf/proto"
)

const (
	RBC_COMMAND         = "rbc_message"
	DataTypeBatch       = "batch"
	DataTypeTransaction = "transaction"
	DataTypeVote        = "vote"
)

// ... (Giữ nguyên các struct: ProposalEvent, broadcastState, ProposalNotification) ...
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

// Process quản lý logic của thuật toán RBC.
type Process struct {
	node          *node.Node // Tham chiếu đến Node trung tâm
	N             int
	F             int
	Delivered     chan *ProposalNotification
	logs          map[string]*broadcastState
	logsMu        sync.RWMutex
	queueManager  *aleaqueues.QueueManager
	MessageSender t_network.MessageSender // Để gửi message
	// ... (Các channel và thuộc tính khác giữ nguyên) ...
	PoolTransactions   chan *pb.Batch
	blockNumberChan    chan uint64
	currentBlockNumber uint64
	proposalChannel    chan ProposalEvent

	votesByBlockNumber map[uint64][]*pb.VoteRequest
	votesMutex         sync.RWMutex

	voteSubscribers  map[uint64]map[chan<- *pb.VoteRequest]struct{}
	subscribersMutex sync.RWMutex
}

func NewProcess() (*Process, error) {
	p := &Process{
		// Khởi tạo các channel và map ở đây
		Delivered:          make(chan *ProposalNotification, 1024),
		logs:               make(map[string]*broadcastState),
		MessageSender:      network.NewMessageSender(""),
		PoolTransactions:   make(chan *pb.Batch, 1024),
		blockNumberChan:    make(chan uint64, 1024),
		proposalChannel:    make(chan ProposalEvent, 1024),
		votesByBlockNumber: make(map[uint64][]*pb.VoteRequest),
		voteSubscribers:    make(map[uint64]map[chan<- *pb.VoteRequest]struct{}),
	}
	return p, nil
}

// GetHandler trả về handler cho các message mạng liên quan đến RBC.
func (p *Process) GetHandler() t_network.Handler {
	return network.NewHandler(
		map[string]func(t_network.Request) error{
			RBC_COMMAND: p.handleNetworkRequest,
			// ... (Các handler khác như SendPoolTransactons, BlockNumber)
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
					logger.Error("❌ Dữ liệu phản hồi block number không hợp lệ: độ dài %d < 8", len(responseData))
					return fmt.Errorf("dữ liệu phản hồi block number không hợp lệ")
				}
				validatorBlockNumber := binary.BigEndian.Uint64(responseData)

				p.blockNumberChan <- validatorBlockNumber
				return nil
			},
		},
		nil,
	)
}

// Start khởi chạy các goroutine xử lý logic của RBC.
func (p *Process) Start() {
	// Goroutine xử lý block number, vote, và đề xuất block mới
	go func() {
		fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d", p.node.ID) + ".log")

		for blockNumber := range p.blockNumberChan {
			time.Sleep(20 * time.Millisecond)
			fileLogger.Info("blockNumberChan: ", blockNumber)

			// 5. Nếu đến lượt, yêu cầu transactions cho block tiếp theo
			isMyTurnForNextBlock := ((int(blockNumber+1) + p.node.Config.NumValidator - 1) % p.node.Config.NumValidator) == (int(p.node.Config.ID) - 1)
			if isMyTurnForNextBlock {
				fileLogger.Info("Đến lượt mình đề xuất cho block %d. Đang yêu cầu transactions...", blockNumber+1)
				p.MessageSender.SendBytes(
					p.node.MasterConn,
					m_common.GetTransactionsPool,
					[]byte{},
				)
			}

			// time.Sleep(10 * time.Millisecond)
			fileLogger.Info("--------------------------------------------------")
			fileLogger.Info("⚡ Bắt đầu xử lý cho block: %d", blockNumber)
			p.UpdateBlockNumber(blockNumber)

			// 1. Lấy payload từ queue
			remainder := int(blockNumber)%p.node.Config.NumValidator + 1
			var payload []byte
			var err error
			for {
				payload, err = p.queueManager.GetByPriority(int32(remainder), int64(blockNumber+1))
				if err == nil {
					break // Lấy được payload thì thoát vòng lặp
				}
				// fileLogger.Info("Không có payload cho block %d (proposer %d). Thử lại sau 10ms...", blockNumber, remainder)
				time.Sleep(10 * time.Millisecond) // Đợi một chút rồi thử lại, tránh vòng lặp quá nhanh
			}

			consensusDecision := true
			// 4. Xử lý kết quả đồng thuận
			if true && payload != nil {
				// Chỉ gửi PushFinalizeEvent nếu đồng thuận là CÓ và có payload
				batch := &pb.Batch{}
				if err := proto.Unmarshal(payload, batch); err == nil {
					if err == nil {
						fileLogger.Info("Đã gửi giao dịch của batch")
						fileLogger.Info("PushFinalizeEvent 1 block: %d - %d : %v ", batch.BlockNumber, blockNumber+1, consensusDecision)
						fileLogger.Info("PushFinalizeEvent 1 %v ", batch.Transactions)
						err := p.MessageSender.SendBytes(p.node.MasterConn, m_common.PushFinalizeEvent, payload)
						if err != nil {
							panic(err)
						}
						logger.Info("Đã gửi PushFinalizeEvent cho block %d : %v", blockNumber+1, consensusDecision)

					} else {
						logger.Info("Đã gửi giao dịch batch rỗng")
						batch := &pb.Batch{
							BlockNumber: blockNumber + 1,
						}
						batchBytes, _ := proto.Marshal(batch)

						fileLogger.Info("PushFinalizeEvent 2 block: %d : %v ", blockNumber+1, consensusDecision)
						err := p.MessageSender.SendBytes(p.node.MasterConn, m_common.PushFinalizeEvent, batchBytes)
						if err != nil {
							panic(err)
						}
					}
				}
			} else {
				logger.Info("Đã gửi giao dịch batch rỗng")
				batch := &pb.Batch{
					BlockNumber: blockNumber + 1,
				}
				batchBytes, _ := proto.Marshal(batch)
				fileLogger.Info("PushFinalizeEvent 3 block: %d : %v", blockNumber+1, consensusDecision)
				err := p.MessageSender.SendBytes(p.node.MasterConn, m_common.PushFinalizeEvent, batchBytes)
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

// ... (Giữ nguyên các hàm achieveVoteConsensus, runSimulation, UpdateBlockNumber, GetCurrentBlockNumber, ...)
// Lưu ý: trong các hàm này, thay p.Config bằng p.node.Config, p.ID bằng p.node.ID, v.v.

// handleNetworkRequest xử lý các thông điệp RBC đến từ module node.
func (p *Process) handleNetworkRequest(req t_network.Request) error {
	var msg pb.RBCMessage
	if err := proto.Unmarshal(req.Message().Body(), &msg); err != nil {
		logger.Error("Error unmarshalling RBCMessage: %v", err)
		return err
	}

	senderID := msg.NetworkSenderId
	p.node.AddConnection(senderID, req.Connection()) // Cập nhật kết nối trong node

	go p.handleMessage(&msg)
	return nil
}

// broadcast chuẩn bị và gửi thông điệp RBC.
func (p *Process) broadcast(msg *pb.RBCMessage) {
	msg.NetworkSenderId = p.node.ID
	payload, err := proto.Marshal(msg)
	if err != nil {
		logger.Error("Node %d: Failed to marshal RBC message: %v", p.node.ID, err)
		return
	}
	go p.handleMessage(msg)
	p.node.Broadcast(RBC_COMMAND, payload)
}

// ... (Giữ nguyên các hàm getOrCreateState, GetVotesByBlockNumber, SubscribeToVotes, handleMessage, StartBroadcast, HandleDelivered, HandlePoolTransactions, RequestInitialBlockNumber, CleanupOldMessages)
// Lưu ý: Cần điều chỉnh để sử dụng p.node khi cần truy cập cấu hình hoặc các chức năng mạng.
// Ví dụ: p.StartBroadcast sẽ gọi p.broadcast(initMsg) mà không cần tự gửi.

// StartBroadcast is called by the application to initiate a new broadcast.
func (p *Process) StartBroadcast(payload []byte, dataType string, messageType pb.MessageType) {
	messageID := fmt.Sprintf("%d-%d", p.node.ID, time.Now().UnixNano())
	logger.Info("Node %d starting broadcast for message %s", p.node.ID, messageID)
	initMsg := &pb.RBCMessage{
		Type:             messageType,
		OriginalSenderId: p.node.ID,
		MessageId:        messageID,
		Payload:          payload,
		DataType:         dataType,
	}
	p.broadcast(initMsg)
}

// getOrCreateState remains the same
func (p *Process) getOrCreateState(key string, payload []byte) *broadcastState {
	p.logsMu.Lock()
	defer p.logsMu.Unlock()

	state, exists := p.logs[key]
	if !exists {
		// Trích xuất block number từ payload
		batch := &pb.Batch{}
		var blockNum uint64 = 0
		// Bỏ qua lỗi, nếu payload không phải là batch thì blockNum sẽ là 0
		if proto.Unmarshal(payload, batch) == nil {
			blockNum = batch.GetBlockNumber()
		}

		state = &broadcastState{
			echoRecvd:   make(map[int32]bool),
			readyRecvd:  make(map[int32]bool),
			payload:     payload,
			BlockNumber: blockNum, // <-- Gán block number
		}
		p.logs[key] = state
	}
	return state
}
func (p *Process) GetVotesByBlockNumber(blockNumber uint64) []*pb.VoteRequest {
	p.votesMutex.RLock() // Sử dụng RLock để cho phép nhiều goroutine đọc cùng lúc
	defer p.votesMutex.RUnlock()

	if votes, found := p.votesByBlockNumber[blockNumber]; found {
		// Tạo một bản sao của slice để trả về, tránh việc bên ngoài sửa đổi slice gốc
		votesCopy := make([]*pb.VoteRequest, len(votes))
		copy(votesCopy, votes)
		return votesCopy
	}

	return nil // hoặc trả về một slice rỗng: make([]*pb.VoteRequest, 0)
}
func (p *Process) SubscribeToVotes(blockNumber uint64) (initialVotes []*pb.VoteRequest, updates <-chan *pb.VoteRequest, unsubscribe func()) {
	// Tạo channel để gửi vote mới cho người gọi
	updateChan := make(chan *pb.VoteRequest, 10) // Buffer để không bỏ lỡ vote

	// 1. Lấy danh sách vote hiện có
	p.votesMutex.RLock()
	existingVotes, found := p.votesByBlockNumber[blockNumber]
	if found {
		// Tạo bản sao để tránh race condition
		initialVotes = make([]*pb.VoteRequest, len(existingVotes))
		copy(initialVotes, existingVotes)
	}
	p.votesMutex.RUnlock()

	// 2. Đăng ký channel để nhận vote mới trong tương lai
	p.subscribersMutex.Lock()
	if _, ok := p.voteSubscribers[blockNumber]; !ok {
		p.voteSubscribers[blockNumber] = make(map[chan<- *pb.VoteRequest]struct{})
	}
	p.voteSubscribers[blockNumber][updateChan] = struct{}{}
	p.subscribersMutex.Unlock()

	// 3. Tạo và trả về hàm hủy đăng ký
	unsubscribe = func() {
		p.subscribersMutex.Lock()
		if subscribers, ok := p.voteSubscribers[blockNumber]; ok {
			delete(subscribers, updateChan)
			// Nếu không còn ai đăng ký cho block này, xóa luôn map con
			if len(subscribers) == 0 {
				delete(p.voteSubscribers, blockNumber)
			}
		}
		p.subscribersMutex.Unlock()
		close(updateChan) // Đóng channel sau khi hủy đăng ký
	}

	return initialVotes, updateChan, unsubscribe
}

// handleMessage is the original, unmodified RBC protocol logic.
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

			// --- THÊM LOGIC LƯU VOTE ---
			p.votesMutex.Lock()
			// Thêm vote vào slice tương ứng với block number
			p.votesByBlockNumber[receivedVote.BlockNumber] = append(p.votesByBlockNumber[receivedVote.BlockNumber], receivedVote)
			p.votesMutex.Unlock()
			// --- THÊM LOGIC THÔNG BÁO ---
			// 2. Thông báo cho tất cả subscribersreceivedVote
			p.subscribersMutex.RLock() // Khóa đọc để kiểm tra subscribers
			if receivedVote.BlockNumber > p.currentBlockNumber {

				if subscribers, found := p.voteSubscribers[receivedVote.BlockNumber]; found {
					for subChan := range subscribers {
						// Gửi vote mới đến từng channel đã đăng ký
						// Sử dụng select để tránh bị block nếu channel đầy
						select {
						case subChan <- receivedVote:
						default: // Nếu channel của người nhận bị đầy, bỏ qua để không làm chậm hệ thống
						}
					}
				}
			}
			p.subscribersMutex.RUnlock()
			// --- KẾT THÚC LOGIC THÔNG BÁO ---
		} else {
			logger.Info(string(msg.Payload))
		}
	case pb.MessageType_INIT:
		if !state.sentEcho {
			state.sentEcho = true
			logger.Info("Node %d received INIT, sending ECHO for message %s", p.node.ID, key)
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
			logger.Info("Node %d has enough ECHOs, sending READY for message %s", p.node.ID, key)
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
			logger.Info("Node %d received f+1 READYs, amplifying READY for message %s", p.node.ID, key)
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
			logger.Info("Node %d has DELIVERED message %s", p.node.ID, key)
			proposerID := msg.OriginalSenderId

			var notification *ProposalNotification
			logger.Error(msg.DataType)
			switch msg.DataType {
			case DataTypeBatch:
				batch := &pb.Batch{}
				if err := proto.Unmarshal(state.payload, batch); err == nil {
					priority := int64(batch.BlockNumber)
					logger.Info("-----------------Enqueue: %v - %v", proposerID, priority)
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
					notification = &ProposalNotification{
						SenderID: proposerID,
						Priority: -1,
						Payload:  state.payload,
					}
				}
			case DataTypeTransaction:
				tx := &pb.Transaction{}
				if err := proto.Unmarshal(state.payload, tx); err == nil {
					notification = &ProposalNotification{
						SenderID: proposerID,
						Priority: 0, // hoặc logic khác
						Payload:  state.payload,
					}
				}
				// Thêm các loại dữ liệu khác ở đây
			default:
				notification = &ProposalNotification{
					SenderID: proposerID,
					Priority: -1,
					Payload:  state.payload,
				}
			}

		}
	}
}

func (p *Process) HandleDelivered() {
	pendingPayloads := make(map[uint64]*ProposalNotification)
	var mu sync.Mutex

	processBatch := func(payload *ProposalNotification) {
		batch := &pb.Batch{}
		err := proto.Unmarshal(payload.Payload, batch)

		if err != nil {
			logger.Error("Failed to Unmarshal Payload: %v", err)
		}

		logger.Info("\n[APPLICATION] Node %d Delivered Batch for Block %d from Proposer %x\n> ", p.node.ID, payload.Priority, payload.SenderID)

		if err != nil {
			logger.Error("Failed to send PushFinalizeEvent: %v", err)
		}
	}

	go func() {
		for {
			payload := <-p.Delivered
			batch := &pb.Batch{}
			err := proto.Unmarshal(payload.Payload, batch)

			if err != nil {
				logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", p.node.ID, string(payload.Payload))
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
					logger.Info("\n[APPLICATION] Node %d received future block %d, pending.\n> ", p.node.ID, payload.Priority)
					pendingPayloads[priorityKey] = payload
				}
			}
			mu.Unlock()
		}
	}()
}

func (p *Process) HandlePoolTransactions() {
	fileLogger, _ := loggerfile.NewFileLogger("Note_" + fmt.Sprintf("%d", p.node.ID) + ".log")

	go func() {
		for txs := range p.PoolTransactions {
			proposerId := p.node.KeyPair.PublicKey().Bytes()
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

			fileLogger.Info("\n[APPLICATION] Node %d broadcasting a batch for block %d...\n> ", p.node.ID, p.GetCurrentBlockNumber()+1)
			p.StartBroadcast(payload, "batch", pb.MessageType_INIT)
		}
	}()
}

func (p *Process) RequestInitialBlockNumber() {
	go func() {
		p.MessageSender.SendBytes(
			p.node.MasterConn,
			m_common.ValidatorGetBlockNumber,
			[]byte{},
		)
	}()
}

func (p *Process) CleanupOldMessages() {
	p.logsMu.Lock()
	defer p.logsMu.Unlock()

	currentBlock := p.GetCurrentBlockNumber()
	// Nếu chưa đủ block để dọn dẹp thì bỏ qua
	if currentBlock <= 1000 {
		return
	}

	cleanupThreshold := currentBlock - 1000
	cleanedCount := 0

	for key, state := range p.logs {
		// Chỉ dọn dẹp những message đã được delivered và đủ cũ
		if state.delivered && state.BlockNumber > 0 && state.BlockNumber < cleanupThreshold {
			delete(p.logs, key)
			cleanedCount++
		}
	}

	if cleanedCount > 0 {
		logger.Info("Node %d CLEANED UP %d old message states for blocks older than %d", p.node.ID, cleanedCount, cleanupThreshold)
	}
}

// =================================================================
// == START: Thêm phương thức để đẩy sự kiện từ bên ngoài
// =================================================================

// PushProposalEvent là phương thức công khai để đẩy sự kiện vào kênh proposal chung
func (p *Process) PushProposalEvent(event ProposalEvent) {
	logger.Info("Received external proposal event via PushProposalEvent: NodeID=%s, Value=%v", event.NodeID, event.Value)
	p.proposalChannel <- event
}

// =================================================================
// == END: Thêm phương thức để đẩy sự kiện từ bên ngoài
// =================================================================

// MessageInTransit mô phỏng một thông điệp đang được gửi qua mạng.
type MessageInTransit[N binaryagreement.NodeIdT] struct {
	Sender  N
	Message binaryagreement.Message
}

func (p *Process) UpdateBlockNumber(blockNumber uint64) {
	p.currentBlockNumber = blockNumber
}

func (p *Process) GetCurrentBlockNumber() uint64 {
	return p.currentBlockNumber
}

// SetNode gán đối tượng Node cho Process và hoàn tất việc khởi tạo.
func (p *Process) SetNode(node *node.Node) {
	p.node = node
	p.N = len(node.Config.Peers)
	p.F = (p.N - 1) / 3

	peerIDs := make([]int32, 0, len(node.Config.Peers))
	for _, peerConf := range node.Config.Peers {
		peerIDs = append(peerIDs, int32(peerConf.Id))
	}
	p.queueManager = aleaqueues.NewQueueManager(peerIDs)
}

func (p *Process) GetCommandHandlers() map[string]func(t_network.Request) error {
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
