package rbc

import (
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/meta-node-blockchain/meta-node/pkg/aleaqueues"
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
	RBC_COMMAND = "rbc_message"
)

// broadcastState remains the same
type broadcastState struct {
	mu         sync.Mutex
	echoRecvd  map[int32]bool
	readyRecvd map[int32]bool
	sentEcho   bool
	sentReady  bool
	delivered  bool
	payload    []byte
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
			isMyTurn := (int(blockNumber) % p.Config.NumValidator) == (int(p.Config.ID) - 1)
			logger.Info("blockNumber: %v", blockNumber)
			logger.Info("remainder: %v", int(blockNumber)%p.Config.NumValidator)
			logger.Info("isMyTurn: %v, %v, %v", isMyTurn, p.Config.NumValidator, p.Config.ID)
			time.Sleep(50 * time.Millisecond)
			if isMyTurn {
				logger.Info("It's my turn (Node %d) to propose for block %d. Requesting transactions...", p.Config.ID, blockNumber)
				p.MessageSender.SendBytes(
					p.MasterConn,
					m_common.GetTransactionsPool,
					[]byte{},
				)
			}
		}
	}()

	p.HandleDelivered()
	p.HandlePoolTransactions()

	time.Sleep(10 * time.Second)
	p.RequestInitialBlockNumber()
	return nil
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
		state = &broadcastState{
			echoRecvd:  make(map[int32]bool),
			readyRecvd: make(map[int32]bool),
			payload:    payload,
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
	case pb.MessageType_INIT:
		if !state.sentEcho {
			state.sentEcho = true
			logger.Info("Node %d received INIT, sending ECHO for message %s", p.ID, key)
			echoMsg := &pb.RBCMessage{
				Type:             pb.MessageType_ECHO,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
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
			}
			p.broadcast(readyMsg)
		}

		if len(state.readyRecvd) > 2*p.F && !state.delivered {
			state.delivered = true
			logger.Info("Node %d has DELIVERED message %s", p.ID, key)
			proposerID := msg.OriginalSenderId

			batch := &pb.Batch{}
			if err := proto.Unmarshal(state.payload, batch); err != nil {
				notification := &ProposalNotification{
					SenderID: proposerID,
					Priority: -1,
					Payload:  state.payload,
				}
				p.Delivered <- notification
				return
			}

			priority := int64(batch.BlockNumber)

			// Sử dụng QueueManager để thêm vào hàng đợi một cách an toàn
			p.queueManager.Enqueue(proposerID, priority, state.payload)
			logger.Info("Node %d delegated to QueueManager to ENQUEUE proposal from node %d with priority %d", p.ID, proposerID, priority)

			// Gửi thông báo lên tầng ứng dụng (không đổi)
			notification := &ProposalNotification{
				SenderID: proposerID,
				Priority: priority,
				Payload:  state.payload,
			}
			p.Delivered <- notification
		}
	}
}

// StartBroadcast is called by the application to initiate a new broadcast.
func (p *Process) StartBroadcast(payload []byte) {
	messageID := fmt.Sprintf("%d-%d", p.ID, time.Now().UnixNano())
	logger.Info("Node %d starting broadcast for message %s", p.ID, messageID)
	initMsg := &pb.RBCMessage{
		Type:             pb.MessageType_INIT,
		OriginalSenderId: p.ID,
		MessageId:        messageID,
		Payload:          payload,
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
		transactionsPb := &pb.Transactions{
			Transactions: batch.Transactions,
		}
		txBytes, err := proto.Marshal(transactionsPb)
		if err != nil {
			logger.Error("Failed to marshal transactions: %v", err)
			return
		}
		err = p.MessageSender.SendBytes(p.MasterConn, m_common.PushFinalizeEvent, txBytes)
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
			p.StartBroadcast(payload)
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
