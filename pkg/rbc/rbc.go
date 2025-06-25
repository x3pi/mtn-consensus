package rbc

import (
	"fmt"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"

	// Imports for the network module
	"github.com/meta-node-blockchain/meta-node/pkg/bls"
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

// Process is updated to use the network module
type Process struct {
	ID        int32
	Peers     map[int32]string // Still used for initial addresses
	N         int
	F         int
	Delivered chan []byte

	server      t_network.SocketServer
	connections map[int32]t_network.Connection // Manages active connections by peer ID
	connMutex   sync.RWMutex

	logs   map[string]*broadcastState
	logsMu sync.Mutex
}

// NewProcess is updated to initialize network module components.
// It now accepts a bls.KeyPair, which is required by the network.SocketServer.
// For this standalone example, we allow it to be nil and create a dummy one.
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
	}

	// Create a handler for incoming RBC messages
	handler := network.NewHandler(
		map[string]func(t_network.Request) error{
			RBC_COMMAND: p.handleNetworkRequest,
		},
		nil, // No rate limits
	)

	// The network module's ConnectionsManager is not used here to avoid forcing
	// the use of Ethereum addresses as keys. A simple map is used instead.
	// In a full integration, the ConnectionsManager would be appropriate.
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
			// The network module is based on Ethereum addresses, but we don't use them here.
			// The real address string is what matters for connection.
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
		// The onConnect callback will handle adding the connection to the map
		// but we can add it here preemptively
		p.addConnection(peerID, conn)
		go p.server.HandleConnection(conn) // Start handling the connection
	}

	return nil
}

// onConnect is a callback for the SocketServer when a new connection is accepted.
// This is primarily for INCOMING connections.
func (p *Process) onConnect(conn t_network.Connection) {
	logger.Info("Node %d sees a new connection from %s", p.ID, conn.RemoteAddrSafe())
	// We don't know the peer's ID yet. It will be mapped when the first message arrives.
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
	// Unmarshal the outer message body to get the inner RBCMessage
	var msg mtn_proto.RBCMessage
	if err := proto.Unmarshal(req.Message().Body(), &msg); err != nil {
		logger.Info("Error unmarshalling RBCMessage: %v", err)
		return err
	}

	// Associate the connection with the sender's ID if not already done
	senderID := msg.NetworkSenderId
	p.addConnection(senderID, req.Connection())

	// Process the message using the original RBC logic
	go p.handleMessage(&msg)
	return nil
}

// send uses the network module to send a message to a specific peer.
func (p *Process) send(targetID int32, msg *mtn_proto.RBCMessage) {
	p.connMutex.RLock()
	conn, ok := p.connections[targetID]
	p.connMutex.RUnlock()

	if !ok || !conn.IsConnect() {
		// logger.Warn("Node %d: No active connection to peer %d. Message not sent.", p.ID, targetID)
		return
	}

	// Set the network sender ID before sending
	msg.NetworkSenderId = p.ID

	// Marshal the RBCMessage to be the body of the network message
	body, err := proto.Marshal(msg)
	if err != nil {
		logger.Info("Node %d: Failed to marshal message for peer %d: %v", p.ID, targetID, err)
		return
	}

	// Create the network module's message wrapper
	netMsg := network.NewMessage(&pb.Message{
		Header: &pb.Header{
			Command: RBC_COMMAND,
		},
		Body: body,
	})

	if err := conn.SendMessage(netMsg); err != nil {
		// logger.Warn("Node %d: Failed to send message to peer %d: %v", p.ID, targetID, err)
	}
}

// broadcast sends a message to all peers, including itself.
func (p *Process) broadcast(msg *mtn_proto.RBCMessage) {
	p.connMutex.RLock()
	// Create a snapshot of the connections to avoid holding the lock during send operations
	connsSnapshot := make(map[int32]t_network.Connection, len(p.connections))
	for id, conn := range p.connections {
		connsSnapshot[id] = conn
	}
	p.connMutex.RUnlock()

	// Set the network sender ID, which is the current process's ID
	msg.NetworkSenderId = p.ID

	for id := range p.Peers {
		if id == p.ID {
			// Handle message for self locally
			go p.handleMessage(msg)
		} else {
			// Send to remote peers
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
func (p *Process) handleMessage(msg *mtn_proto.RBCMessage) {
	key := fmt.Sprintf("%d-%s", msg.OriginalSenderId, msg.MessageId)
	state := p.getOrCreateState(key, msg.Payload)

	state.mu.Lock()
	defer state.mu.Unlock()

	switch msg.Type {
	case mtn_proto.MessageType_INIT:
		if !state.sentEcho {
			state.sentEcho = true
			logger.Info("Node %d received INIT, sending ECHO for message %s", p.ID, key)
			echoMsg := &mtn_proto.RBCMessage{
				Type:             mtn_proto.MessageType_ECHO,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
			}
			p.broadcast(echoMsg)
		}

	case mtn_proto.MessageType_ECHO:
		state.echoRecvd[msg.NetworkSenderId] = true
		if len(state.echoRecvd) > (p.N+p.F)/2 && !state.sentReady {
			state.sentReady = true
			logger.Info("Node %d has enough ECHOs, sending READY for message %s", p.ID, key)
			readyMsg := &mtn_proto.RBCMessage{
				Type:             mtn_proto.MessageType_READY,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
			}
			p.broadcast(readyMsg)
		}

	case mtn_proto.MessageType_READY:
		state.readyRecvd[msg.NetworkSenderId] = true

		if len(state.readyRecvd) > p.F && !state.sentReady {
			state.sentReady = true
			logger.Info("Node %d received f+1 READYs, amplifying READY for message %s", p.ID, key)
			readyMsg := &mtn_proto.RBCMessage{
				Type:             mtn_proto.MessageType_READY,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
			}
			p.broadcast(readyMsg)
		}

		if len(state.readyRecvd) > 2*p.F && !state.delivered {
			state.delivered = true
			logger.Info("Node %d has DELIVERED message %s: %s", p.ID, key, string(state.payload))
			p.Delivered <- state.payload
		}
	}
}

// StartBroadcast is called by the application to initiate a new broadcast.
func (p *Process) StartBroadcast(payload []byte) {
	messageID := fmt.Sprintf("%d-%d", p.ID, time.Now().UnixNano())
	logger.Info("Node %d starting broadcast for message %s", p.ID, messageID)
	initMsg := &mtn_proto.RBCMessage{
		Type:             mtn_proto.MessageType_INIT,
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
