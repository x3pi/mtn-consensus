package rbc

import (
	"fmt"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/meta-node-blockchain/meta-node/pkg/bls"
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

// Process is updated to use the network module and include the KeyPair
type Process struct {
	ID        int32
	Peers     map[int32]string // Still used for initial addresses
	N         int
	F         int
	Delivered chan []byte
	KeyPair   *bls.KeyPair // The missing KeyPair field

	server      t_network.SocketServer
	connections map[int32]t_network.Connection // Manages active connections by peer ID
	connMutex   sync.RWMutex

	logs   map[string]*broadcastState
	logsMu sync.RWMutex
}

// NewProcess is updated to initialize network module components and store the KeyPair.
func NewProcess(id int32, peers map[int32]string, keyPair *bls.KeyPair) (*Process, error) {
	n := len(peers)
	f := (n - 1) / 3
	if n <= 3*f {
		return nil, fmt.Errorf("system cannot tolerate failures with n=%d, f=%d. Requires n > 3f", n, f)
	}

	// If no keypair is provided, create a dummy one. The network module requires it.
	if keyPair == nil {
		keyPair = bls.GenerateKeyPair()
	}

	p := &Process{
		ID:          id,
		Peers:       peers,
		N:           n,
		F:           f,
		Delivered:   make(chan []byte, 1024),
		logs:        make(map[string]*broadcastState),
		connections: make(map[int32]t_network.Connection),
		KeyPair:     keyPair, // Store the keypair in the process struct
	}

	// Create a handler for incoming RBC messages
	handler := network.NewHandler(
		map[string]func(t_network.Request) error{
			RBC_COMMAND: p.handleNetworkRequest,
		},
		nil, // No rate limits
	)

	connectionsManager := network.NewConnectionsManager()

	var err error
	p.server, err = network.NewSocketServer(keyPair, connectionsManager, handler, "rbc-node", "0.0.1")
	if err != nil {
		return nil, fmt.Errorf("failed to create socket server: %v", err)
	}

	// Add callbacks for when connections are established or dropped
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

	return nil
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
			p.Delivered <- state.payload
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
