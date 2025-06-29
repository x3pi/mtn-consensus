package rbc

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"log"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/meta-node-blockchain/meta-node/pkg/aleaqueues"
	"github.com/meta-node-blockchain/meta-node/pkg/binaryagreement"
	"github.com/meta-node-blockchain/meta-node/pkg/bls"
	m_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/network"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"

	"google.golang.org/protobuf/proto"
)

const (
	// Command for RBC messages within the network module's protocol
	RBC_COMMAND         = "rbc_message"
	DataTypeBatch       = "batch"
	DataTypeTransaction = "transaction"
	DataTypeVote        = "vote"
)

// Thêm struct ProposalEvent
type ProposalEvent struct {
	NodeID string
	Value  bool
}

// broadcastState remains the same
type broadcastState struct {
	mu          sync.Mutex
	echoRecvd   map[int32]bool
	readyRecvd  map[int32]bool
	sentEcho    bool
	sentReady   bool
	delivered   bool
	payload     []byte
	BlockNumber uint64 // <-- Thêm dòng này

}

// PeerConfig represents the configuration for a peer node.
type PeerConfig struct {
	Id                int    `json:"id"`
	ConnectionAddress string `json:"connection_address"`
	PublicKey         string `json:"public_key"`
}

// ProposalNotification được giữ nguyên
type ProposalNotification struct {
	SenderID int32
	Priority int64
	Payload  []byte
}

type ValidatorInfo struct {
	PublicKey string `json:"public_key"`
}
type NodeConfig struct {
	ID                int             `json:"id"`
	KeyPair           string          `json:"key_pair"`
	Master            PeerConfig      `json:"master"`
	NodeType          string          `json:"node_type"`
	Version           string          `json:"version"`
	ConnectionAddress string          `json:"connection_address"`
	Peers             []PeerConfig    `json:"peers"`
	NumValidator      int             `json:"num_validator"`
	Validator         []ValidatorInfo `json:"validator"`
}

// Process is updated to use the network module and include the KeyPair
type Process struct {
	Config *NodeConfig // Lưu cấu hình được truyền vào

	ID        int32
	Peers     map[int32]string
	N         int
	F         int
	Delivered chan *ProposalNotification
	KeyPair   *bls.KeyPair

	server      t_network.SocketServer
	connections map[int32]t_network.Connection
	connMutex   sync.RWMutex

	logs   map[string]*broadcastState
	logsMu sync.RWMutex

	// Thay thế map và mutex bằng con trỏ đến QueueManager
	queueManager *aleaqueues.QueueManager

	MasterConn    t_network.Connection    // Kết nối đến Master
	MessageSender t_network.MessageSender // Để gửi message

	// Channels for the new handlers
	PoolTransactions   chan []*pb.Transaction
	blockNumberChan    chan uint64
	currentBlockNumber uint64
	proposalChannel    chan ProposalEvent
}

// NewProcess được cập nhật để nhận RBC Config
func NewProcess(config *NodeConfig) (*Process, error) {
	n := len(config.Peers)
	f := (n - 1) / 3
	if n <= 3*f {
		return nil, fmt.Errorf("system cannot tolerate failures with n=%d, f=%d. Requires n > 3f", n, f)
	}
	keyPair := bls.NewKeyPair(common.FromHex(config.KeyPair))
	if keyPair == nil {
		keyPair = bls.GenerateKeyPair()
	}

	peers := make(map[int32]string)
	for _, nodeConf := range config.Peers {
		peers[int32(nodeConf.Id)] = nodeConf.ConnectionAddress
	}
	peerIDs := make([]int32, 0, len(config.Peers))
	for _, nodeConf := range config.Peers {
		peers[int32(nodeConf.Id)] = nodeConf.ConnectionAddress
		peerIDs = append(peerIDs, int32(nodeConf.Id))
	}

	p := &Process{
		Config:           config,
		ID:               int32(config.ID),
		Peers:            peers,
		N:                n,
		F:                f,
		Delivered:        make(chan *ProposalNotification, 1024),
		logs:             make(map[string]*broadcastState),
		connections:      make(map[int32]t_network.Connection),
		KeyPair:          keyPair,
		MessageSender:    network.NewMessageSender(""), // Khởi tạo MessageSender
		PoolTransactions: make(chan []*pb.Transaction, 1024),
		blockNumberChan:  make(chan uint64, 1024),
		proposalChannel:  make(chan ProposalEvent, 1024),
	}
	p.queueManager = aleaqueues.NewQueueManager(peerIDs)

	handler := network.NewHandler(
		map[string]func(t_network.Request) error{
			RBC_COMMAND: p.handleNetworkRequest,
			m_common.SendPoolTransactons: func(req t_network.Request) error {
				var transactionsPb pb.Transactions
				if err := proto.Unmarshal(req.Message().Body(), &transactionsPb); err != nil {
					logger.Error("❌ Failed to unmarshal pool transactions: %v", err)
					return err
				}
				p.PoolTransactions <- transactionsPb.GetTransactions()
				return nil
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
	connectionsManager := network.NewConnectionsManager()
	var err error
	p.server, err = network.NewSocketServer(bls.GenerateKeyPair(), connectionsManager, handler, "validator", "0.0.1")
	if err != nil {
		return nil, fmt.Errorf("failed to create socket server: %v", err)
	}
	p.server.AddOnConnectedCallBack(p.onConnect)
	p.server.AddOnDisconnectedCallBack(p.onDisconnect)

	return p, nil
}

// Start now launches the SocketServer and connects to peers.
func (p *Process) Start() error {
	addr := p.Peers[p.ID]
	// Start listening for incoming connections in a separate goroutine
	go func() {
		logger.Info("Node %d listening on %s", p.ID, addr)
		if err := p.server.Listen(addr); err != nil {
			logger.Error("Server listening error on node %d: %v", p.ID, err)
		}
	}()

	// Allow some time for other nodes to start their listeners
	time.Sleep(time.Second * 2)

	// Kết nối tới Master
	logger.Info("Node %d attempting to connect to Master at %s", p.Config.ID, p.Config.Master.ConnectionAddress)
	masterConn := network.NewConnection(common.HexToAddress("0x0"), m_common.MASTER_CONNECTION_TYPE)
	masterConn.SetRealConnAddr(p.Config.Master.ConnectionAddress)
	if err := masterConn.Connect(); err != nil {
		logger.Error("Node %d failed to connect to Master: %v", p.Config.ID, err)
		// Có thể quyết định dừng chương trình hoặc thử lại ở đây
	} else {
		p.MasterConn = masterConn
		p.addConnection(-1, masterConn)
		go p.server.HandleConnection(masterConn)
		logger.Info("Node %d connected to Master", p.Config.ID)
	}

	// Connect to all other peers
	for peerID, peerAddr := range p.Peers {
		if peerID == p.ID {
			continue
		}

		// Create a new connection object
		conn := network.NewConnection(
			common.HexToAddress("0x0"),
			RBC_COMMAND, // Set a type for clarity
		)
		conn.SetRealConnAddr(peerAddr)

		logger.Info("Node %d attempting to connect to Node %d at %s", p.ID, peerID, peerAddr)
		err := conn.Connect()
		if err != nil {
			logger.Warn("Node %d failed to connect to Node %d: %v", p.ID, peerID, err)
			continue
		}
		p.addConnection(peerID, conn)
		go p.server.HandleConnection(conn) // Start handling the connection
	}

	// Khởi chạy goroutine xử lý block number như yêu cầu
	go func() {
		for blockNumber := range p.blockNumberChan {

			logger.Info("New block number received: %d", blockNumber)
			p.UpdateBlockNumber(blockNumber)
			isMyTurn := ((int(blockNumber) + p.Config.NumValidator - 1) % p.Config.NumValidator) == (int(p.Config.ID) - 1)
			logger.Info("blockNumber: %v", blockNumber)
			logger.Info("isMyTurn: %v, %v, %v", isMyTurn, p.Config.NumValidator, p.Config.ID)
			remainder := int(blockNumber)%p.Config.NumValidator + 1
			logger.Info("remainder: %v", remainder)

			logger.Info(remainder)
			playload, err := p.queueManager.Dequeue(int32(remainder))
			logger.Error("playload: ", playload)
			logger.Error("err: ", err)

			vote := &pb.VoteRequest{
				BlockNumber: blockNumber + 1,
				NodeId:      int32(p.Config.ID),
				Vote:        err != nil,
			}
			voteBytes, err := proto.Marshal(vote)
			if err != nil {
				log.Fatalf("Lỗi khi marshal (serialize): %v", err)
			}
			p.StartBroadcast(voteBytes, DataTypeVote, pb.MessageType_SEND)

			nodeIDs := []string{"1", "2", "3", "4", "5"}
			numFaulty := 1
			ctx, cancel := context.WithCancel(context.Background())
			// <<< SỬA LỖI: Tăng buffer để tránh deadlock nếu gửi nhanh hơn nhận
			proposalChannel1 := make(chan ProposalEvent, len(nodeIDs)*2)

			var proposalSenderWg sync.WaitGroup

			proposalSenderWg.Add(1)
			go func() {
				defer proposalSenderWg.Done()
				defer func() {
					if r := recover(); r != nil {
						logger.Error("Goroutine gửi proposal panic: %v", r)
					}
				}()
				// <<< SỬA LỖI: Logic đề xuất phức tạp hơn để kiểm tra cả true và false
				// Node 1 và 5 đề xuất true (giả sử có block)
				// Node 2, 3, 4 đề xuất false (giả sử không có block)
				// Điều này mô phỏng một kịch bản thực tế hơn.
				proposals := []ProposalEvent{
					{NodeID: "1", Value: err == nil},
					{NodeID: "2", Value: err != nil},
					{NodeID: "3", Value: err != nil},
					{NodeID: "4", Value: err != nil},
					{NodeID: "5", Value: err == nil},
				}
				for _, p := range proposals {
					select {
					case <-ctx.Done():
						logger.Info("Goroutine gửi proposal dừng lại do context bị huỷ")
						return
					case proposalChannel1 <- p:
						time.Sleep(10 * time.Millisecond) // Giảm thời gian chờ để mô phỏng nhanh hơn
					}
				}
				logger.Info("Đã gửi xong tất cả proposals.")
			}()

			// <<< SỬA LỖI: Nhận lại quyết định từ `runSimulation`
			simulationDecision := runSimulation(
				ctx, cancel,
				"Tất cả các nút đều trung thực",
				nodeIDs, numFaulty,
				proposalChannel1,
				&proposalSenderWg,
				fmt.Sprintf("%d", p.ID), // <<< SỬA LỖI: Truyền ID của node hiện tại vào mô phỏng
			)
			batch := &pb.Batch{}
			err = proto.Unmarshal(playload, batch)

			if err != nil {
				logger.Error("Failed to Unmarshal Payload: %v", err)
			}
			// <<< SỬA LỖI: Sử dụng kết quả từ mô phỏng để quyết định giá trị vote
			value := simulationDecision
			logger.Info("QUYẾT ĐỊNH CUỐI CÙNG CỦA NODE %d cho Block: %d LÀ: %v", p.ID, blockNumber+1, value)
			transactionsPb := &pb.Transactions{
				Transactions: batch.Transactions,
			}
			txBytes, err := proto.Marshal(transactionsPb)
			if err != nil {
				logger.Error("Failed to marshal transactions: %v", err)
				return
			}
			err = p.MessageSender.SendBytes(p.MasterConn, m_common.PushFinalizeEvent, txBytes)
			if value {
				logger.Info("batch: ")
				batch := &pb.Batch{}
				err = proto.Unmarshal(playload, batch)
				logger.Info(batch)
				logger.Info(err)
			}
			if isMyTurn {
				logger.Info("It's my turn (Node %d) to propose for block %d. Requesting transactions...", p.Config.ID, blockNumber)
				p.MessageSender.SendBytes(
					p.MasterConn,
					m_common.GetTransactionsPool,
					[]byte{},
				)
			}
			p.CleanupOldMessages()

		}
	}()

	// Di chuyển các câu lệnh này vào bên trong hàm Start()
	p.HandleDelivered()
	p.HandlePoolTransactions()

	time.Sleep(10 * time.Second)
	p.RequestInitialBlockNumber()
	return nil
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

// <<< SỬA LỖI: Thay đổi chữ ký hàm để trả về `bool`
func runSimulation(
	ctx context.Context,
	cancel context.CancelFunc,
	scenarioTitle string,
	nodeIDs []string,
	numFaulty int,
	proposalChannel chan ProposalEvent,
	proposalSenderWg *sync.WaitGroup,
	ourID string, // <<< SỬA LỖI: Thêm tham số để biết ID của node hiện tại
) bool { // <<< SỬA LỖI: Trả về quyết định cuối cùng
	defer cancel()

	logger.Info("\n\n==============================================================")
	logger.Info("🚀 KỊCH BẢN: %s (Mô phỏng bất đồng bộ)\n", scenarioTitle)
	logger.Info("==============================================================")

	nodes := make(map[string]*binaryagreement.BinaryAgreement[string, string])
	nodeChannels := make(map[string]chan MessageInTransit[string])
	var nodeWg sync.WaitGroup
	networkOutgoing := make(chan MessageInTransit[string], len(nodeIDs)*10)
	sessionID := "session-1"
	var closeOnce sync.Once

	// <<< SỬA LỖI: Channel để nhận quyết định cuối cùng từ các node
	decisionChannel := make(chan bool, len(nodeIDs))

	for _, id := range nodeIDs {
		netinfo := binaryagreement.NewNetworkInfo(id, nodeIDs, numFaulty, true)
		nodes[id] = binaryagreement.NewBinaryAgreement[string, string](netinfo, sessionID)
		nodeChannels[id] = make(chan MessageInTransit[string], 100)
	}

	cleanupAndShutdown := func() {
		closeOnce.Do(func() {
			logger.Info("🎉 Đạt được đồng thuận! Bắt đầu quá trình kết thúc mô phỏng.")
			cancel()
			logger.Info("Đang chờ goroutine gửi proposal kết thúc...")
			proposalSenderWg.Wait()
			logger.Info("Goroutine gửi proposal đã kết thúc.")
			close(proposalChannel)
			close(networkOutgoing)
		})
	}

	for _, id := range nodeIDs {
		nodeWg.Add(1)
		go func(nodeID string) {
			defer nodeWg.Done()
			nodeInstance := nodes[nodeID]

			for {
				select {
				case <-ctx.Done():
					return
				case transitMsg, ok := <-nodeChannels[nodeID]:
					if !ok {
						return
					}
					step, err := nodeInstance.HandleMessage(transitMsg.Sender, transitMsg.Message)
					if err != nil {
						continue
					}
					// <<< SỬA LỖI: Kiểm tra output của step
					if step.Output != nil {
						if decision, ok := step.Output.(bool); ok {
							// Gửi quyết định vào channel chung
							decisionChannel <- decision
						}
					}
					for _, msgToSend := range step.MessagesToSend {
						select {
						case networkOutgoing <- MessageInTransit[string]{Sender: nodeID, Message: msgToSend.Message}:
						case <-ctx.Done():
							logger.Info("Handler for node %s stopping send because context is done.", nodeID)
							return
						}
					}
				}
			}
		}(id)
	}

	var networkWg sync.WaitGroup
	networkWg.Add(1)
	go func() {
		defer networkWg.Done()

		var senderWg sync.WaitGroup
		for transitMsg := range networkOutgoing {
			for _, recipientID := range nodeIDs {
				msgCopy := transitMsg
				senderWg.Add(1)
				go func(recID string, msg MessageInTransit[string]) {
					defer senderWg.Done()
					defer func() {
						if r := recover(); r != nil {
							logger.Error("Gửi vào nodeChannels[%s] bị panic: %v", recID, r)
						}
					}()

					select {
					case <-ctx.Done():
						return
					default:
					}

					if nodes[recID] == nil || nodes[recID].Terminated() {
						return
					}

					select {
					case nodeChannels[recID] <- msg:
					case <-ctx.Done():
						return
					}
				}(recipientID, msgCopy)
			}
		}
		senderWg.Wait()
		for _, ch := range nodeChannels {
			close(ch)
		}
	}()

	logger.Info("--- Đang lắng nghe proposals từ channel. Mô phỏng đang chạy... ---")
	var proposalWg sync.WaitGroup
	proposalWg.Add(1)
	go func() {
		defer proposalWg.Done()
		for proposalEvent := range proposalChannel {
			id := proposalEvent.NodeID
			value := proposalEvent.Value
			logger.Info("Nhận proposal từ channel - Nút %s đề xuất giá trị: %v\n", id, value)

			if nodes[id] == nil || nodes[id].Terminated() {
				continue
			}

			step, err := nodes[id].Propose(value)
			if err != nil {
				logger.Error("Nút %s không thể đề xuất: %v\n", id, err)
				continue
			}
			// <<< SỬA LỖI: Kiểm tra output ngay sau khi propose
			if step.Output != nil {
				if decision, ok := step.Output.(bool); ok {
					decisionChannel <- decision
				}
			}
			for _, msgToSend := range step.MessagesToSend {
				networkOutgoing <- MessageInTransit[string]{Sender: id, Message: msgToSend.Message}
			}
		}
	}()

	var monitorWg sync.WaitGroup
	monitorWg.Add(1)
	go func() {
		defer monitorWg.Done()
		requiredDecisions := len(nodeIDs) - numFaulty
		ticker := time.NewTicker(100 * time.Millisecond)
		defer ticker.Stop()

		for {
			select {
			case <-ticker.C:
				decidedCount := 0
				for _, node := range nodes {
					if node.Terminated() {
						decidedCount++
					}
				}
				if decidedCount >= requiredDecisions {
					cleanupAndShutdown()
					return
				}
			case <-ctx.Done():
				return
			}
		}
	}()

	proposalWg.Wait()
	nodeWg.Wait()
	networkWg.Wait()
	monitorWg.Wait()

	// <<< SỬA LỖI: Đóng decisionChannel sau khi tất cả các goroutine có thể ghi đã dừng
	close(decisionChannel)

	logger.Info("\n\n--- KẾT QUẢ CUỐI CÙNG ---")
	// <<< SỬA LỖI: Lấy quyết định cuối cùng từ channel
	finalDecision := false // Mặc định là false
	// Đọc quyết định đầu tiên từ channel, vì tất cả các node trung thực sẽ có cùng quyết định
	if decision, ok := <-decisionChannel; ok {
		finalDecision = decision
	}

	for id, node := range nodes {
		if decision, ok := node.GetDecision(); ok {
			logger.Info("✅ Nút %s đã kết thúc và quyết định: %v\n", id, decision)
		} else {
			logger.Warn("❌ Nút %s KHÔNG kết thúc hoặc không có quyết định.\n", id)
		}
	}

	return finalDecision // <<< SỬA LỖI: Trả về kết quả
}

// UpdateBlockNumber cập nhật số block hiện tại cho process
func (p *Process) UpdateBlockNumber(blockNumber uint64) {
	p.currentBlockNumber = blockNumber
}

func (p *Process) GetCurrentBlockNumber() uint64 {
	return p.currentBlockNumber
}

// onConnect is a callback for the SocketServer when a new connection is accepted.
func (p *Process) onConnect(conn t_network.Connection) {
	logger.Info("Node %d sees a new connection from %s", p.ID, conn.RemoteAddrSafe())
}

// onDisconnect is a callback for when a connection is lost.
func (p *Process) onDisconnect(conn t_network.Connection) {
	p.connMutex.Lock()
	defer p.connMutex.Unlock()
	for id, c := range p.connections {
		if c == conn {
			logger.Warn("Node %d disconnected from Node %d", p.ID, id)
			delete(p.connections, id)
			return
		}
	}
	logger.Warn("Node %d disconnected from an unknown peer at %s", p.ID, conn.RemoteAddrSafe())
}

// addConnection safely adds a connection to the map.
func (p *Process) addConnection(peerID int32, conn t_network.Connection) {
	p.connMutex.Lock()
	defer p.connMutex.Unlock()

	if existingConn, ok := p.connections[peerID]; ok && existingConn.IsConnect() {
		logger.Info("Node %d already has a connection for peer %d", p.ID, peerID)
		return
	}
	p.connections[peerID] = conn
	logger.Info("Node %d stored connection for peer %d", p.ID, peerID)
}

// handleNetworkRequest is the entry point for messages from the network module.
func (p *Process) handleNetworkRequest(req t_network.Request) error {
	var msg pb.RBCMessage
	if err := proto.Unmarshal(req.Message().Body(), &msg); err != nil {
		logger.Info("Error unmarshalling RBCMessage: %v", err)
		return err
	}

	senderID := msg.NetworkSenderId
	p.addConnection(senderID, req.Connection())

	go p.handleMessage(&msg)
	return nil
}

// send uses the network module to send a message to a specific peer.
func (p *Process) send(targetID int32, msg *pb.RBCMessage) {
	p.connMutex.RLock()
	conn, ok := p.connections[targetID]
	p.connMutex.RUnlock()

	if !ok || !conn.IsConnect() {
		return
	}

	msg.NetworkSenderId = p.ID

	body, err := proto.Marshal(msg)
	if err != nil {
		logger.Info("Node %d: Failed to marshal message for peer %d: %v", p.ID, targetID, err)
		return
	}

	netMsg := network.NewMessage(&pb.Message{
		Header: &pb.Header{
			Command: RBC_COMMAND,
		},
		Body: body,
	})

	if err := conn.SendMessage(netMsg); err != nil {
	}
}

// broadcast sends a message to all peers, including itself.
func (p *Process) broadcast(msg *pb.RBCMessage) {
	p.connMutex.RLock()
	connsSnapshot := make(map[int32]t_network.Connection, len(p.connections))
	for id, conn := range p.connections {
		connsSnapshot[id] = conn
	}
	p.connMutex.RUnlock()

	msg.NetworkSenderId = p.ID

	for id := range p.Peers {
		if id == p.ID {
			go p.handleMessage(msg)
		} else {
			go p.send(id, msg)
		}
	}
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
		}
	case pb.MessageType_INIT:
		if !state.sentEcho {
			state.sentEcho = true
			logger.Info("Node %d received INIT, sending ECHO for message %s", p.ID, key)
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
			logger.Info("Node %d has enough ECHOs, sending READY for message %s", p.ID, key)
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
			logger.Info("Node %d received f+1 READYs, amplifying READY for message %s", p.ID, key)
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
			logger.Info("Node %d has DELIVERED message %s", p.ID, key)
			proposerID := msg.OriginalSenderId

			var notification *ProposalNotification
			logger.Error(msg.DataType)
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

// StartBroadcast is called by the application to initiate a new broadcast.
func (p *Process) StartBroadcast(payload []byte, dataType string, messageType pb.MessageType) {
	messageID := fmt.Sprintf("%d-%d", p.ID, time.Now().UnixNano())
	logger.Info("Node %d starting broadcast for message %s", p.ID, messageID)
	initMsg := &pb.RBCMessage{
		Type:             messageType,
		OriginalSenderId: p.ID,
		MessageId:        messageID,
		Payload:          payload,
		DataType:         dataType,
	}
	p.broadcast(initMsg)
}

// Stop gracefully shuts down the server.
func (p *Process) Stop() {
	p.server.Stop()
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

		logger.Info("\n[APPLICATION] Node %d Delivered Batch for Block %d from Proposer %x\n> ", p.ID, payload.Priority, payload.SenderID)

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
				logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", p.ID, string(payload.Payload))
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
					logger.Info("\n[APPLICATION] Node %d received future block %d, pending.\n> ", p.ID, payload.Priority)
					pendingPayloads[priorityKey] = payload
				}
			}
			mu.Unlock()
		}
	}()
}

func (p *Process) HandlePoolTransactions() {
	go func() {
		for txs := range p.PoolTransactions {
			proposerId := p.KeyPair.PublicKey().Bytes()
			headerData := fmt.Sprintf("%d:%x", p.GetCurrentBlockNumber()+1, proposerId)
			batchHash := sha256.Sum256([]byte(headerData))

			batch := &pb.Batch{
				Hash:         batchHash[:],
				Transactions: txs,
				BlockNumber:  p.GetCurrentBlockNumber() + 1,
				ProposerId:   proposerId,
			}

			payload, err := proto.Marshal(batch)
			if err != nil {
				logger.Error("Failed to marshal batch:", err)
				continue
			}

			logger.Info("\n[APPLICATION] Node %d broadcasting a batch for block %d...\n> ", p.ID, p.GetCurrentBlockNumber()+1)
			p.StartBroadcast(payload, "batch", pb.MessageType_INIT)
		}
	}()
}

func (p *Process) RequestInitialBlockNumber() {
	go func() {
		p.MessageSender.SendBytes(
			p.MasterConn,
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
	if currentBlock <= 50 {
		return
	}

	cleanupThreshold := currentBlock - 50
	cleanedCount := 0

	for key, state := range p.logs {
		// Chỉ dọn dẹp những message đã được delivered và đủ cũ
		if state.delivered && state.BlockNumber > 0 && state.BlockNumber < cleanupThreshold {
			delete(p.logs, key)
			cleanedCount++
		}
	}

	if cleanedCount > 0 {
		logger.Info("Node %d CLEANED UP %d old message states for blocks older than %d", p.ID, cleanedCount, cleanupThreshold)
	}
}
