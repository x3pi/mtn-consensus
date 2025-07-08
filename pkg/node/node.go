package node

import (
	"fmt"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/meta-node-blockchain/meta-node/pkg/bls"
	m_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/config"
	"github.com/meta-node-blockchain/meta-node/pkg/core"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/network"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
)

// Node là trung tâm quản lý mạng, kết nối và các thông điệp.
// Nó thực hiện interface core.Host để cung cấp dịch vụ cho các module.
type Node struct {
	config       *config.NodeConfig
	id           int32
	keyPair      *bls.KeyPair
	peers        map[int32]string
	server       t_network.SocketServer
	connections  map[int32]t_network.Connection
	connMutex    sync.RWMutex
	masterConn   t_network.Connection
	modules      []core.Module
	rootHandlers map[string]func(t_network.Request) error
}

// NewNode khởi tạo một đối tượng Node mới từ cấu hình.
func NewNode(config *config.NodeConfig) (*Node, error) {
	keyPair := bls.NewKeyPair(common.FromHex(config.KeyPair))
	if keyPair == nil {
		keyPair = bls.GenerateKeyPair()
	}

	peers := make(map[int32]string)
	for _, nodeConf := range config.Peers {
		peers[int32(nodeConf.Id)] = nodeConf.ConnectionAddress
	}

	node := &Node{
		config:       config,
		id:           int32(config.ID),
		keyPair:      keyPair,
		peers:        peers,
		connections:  make(map[int32]t_network.Connection),
		modules:      make([]core.Module, 0),
		rootHandlers: make(map[string]func(t_network.Request) error),
	}
	return node, nil
}

// RegisterModule đăng ký một module logic với node.
func (n *Node) RegisterModule(m core.Module) {
	n.modules = append(n.modules, m)
	for command, handler := range m.CommandHandlers() {
		if _, exists := n.rootHandlers[command]; exists {
			logger.Warn("Command '%s' is already registered and will be overwritten.", command)
		}
		n.rootHandlers[command] = handler
	}
}

// Start khởi chạy node, lắng nghe kết nối và kết nối đến các peers.
func (n *Node) Start() error {
	rootHandler := network.NewHandler(n.rootHandlers, nil)

	connectionsManager := network.NewConnectionsManager()
	var err error
	n.server, err = network.NewSocketServer(n.keyPair, connectionsManager, rootHandler, "validator", "0.0.1")
	if err != nil {
		return fmt.Errorf("failed to create socket server: %w", err)
	}
	n.server.AddOnConnectedCallBack(n.onConnect)
	n.server.AddOnDisconnectedCallBack(n.onDisconnect)

	addr := n.config.ConnectionAddress
	go func() {
		logger.Info("Node %d listening on %s", n.id, addr)
		if err := n.server.Listen(addr); err != nil {
			logger.Error("Server listening error on node %d: %v", n.id, err)
		}
	}()

	time.Sleep(time.Second * 2)

	logger.Info("Node %d attempting to connect to Master at %s", n.id, n.config.Master.ConnectionAddress)
	masterConn := network.NewConnection(common.HexToAddress("0x0"), m_common.MASTER_CONNECTION_TYPE)
	masterConn.SetRealConnAddr(n.config.Master.ConnectionAddress)
	if err := masterConn.Connect(); err != nil {
		logger.Error("Node %d failed to connect to Master: %v", n.id, err)
	} else {
		n.masterConn = masterConn
		n.AddConnection(-1, masterConn)
		go n.server.HandleConnection(masterConn)
		logger.Info("Node %d connected to Master", n.id)
	}

	for peerID, peerAddr := range n.peers {
		if peerID == n.id {
			continue
		}
		conn := network.NewConnection(common.HexToAddress("0x0"), "rbc_message")
		conn.SetRealConnAddr(peerAddr)
		logger.Info("Node %d attempting to connect to Node %d at %s", n.id, peerID, peerAddr)
		err := conn.Connect()
		if err != nil {
			logger.Warn("Node %d failed to connect to Node %d: %v", n.id, peerID, err)
			continue
		}
		n.AddConnection(peerID, conn)
		go n.server.HandleConnection(conn)
	}

	// Đợi cho đến khi tất cả các kết nối đã sẵn sàng hoặc timeout (ví dụ 10 giây)
	maxWait := 10000 * time.Second
	start := time.Now()
	var allConnected bool
	for {
		allConnected = true
		// Kiểm tra kết nối master
		if n.masterConn == nil || !n.masterConn.IsConnect() {
			allConnected = false
		}
		// Kiểm tra kết nối tới các peer
		for peerID := range n.peers {
			if peerID == n.id {
				continue
			}
			conn, ok := n.GetConnection(peerID)
			if !ok || !conn.IsConnect() {
				allConnected = false
				break
			}
		}
		if allConnected || time.Since(start) > maxWait {
			break
		}
		time.Sleep(200 * time.Millisecond)
	}

	if allConnected {
		logger.Info("Node %d đã kết nối thành công tới Master và tất cả các peer.", n.id)
	} else {
		logger.Warn("Node %d KHÔNG kết nối đủ tới Master hoặc các peer (timeout).", n.id)
	}

	for _, m := range n.modules {
		m.Start()
	}

	return nil
}

// Stop dừng hoạt động của node và các module.
func (n *Node) Stop() {
	for _, m := range n.modules {
		m.Stop()
	}
	n.server.Stop()
}

// ID trả về ID của node.
func (n *Node) ID() int32 {
	return n.id
}

// Config trả về cấu hình của node.
func (n *Node) Config() *config.NodeConfig {
	return n.config
}

// MasterConn trả về kết nối đến Master Node.
func (n *Node) MasterConn() t_network.Connection {
	return n.masterConn
}

// Send gửi một thông điệp đến một peer cụ thể.
func (n *Node) Send(targetID int32, command string, body []byte) error {
	// time.Sleep(50 * time.Millisecond)
	conn, ok := n.GetConnection(targetID)
	if !ok || !conn.IsConnect() {
		return fmt.Errorf("node %d not connected to target %d", n.id, targetID)
	}

	netMsg := network.NewMessage(&pb.Message{
		Header: &pb.Header{Command: command},
		Body:   body,
	})

	return conn.SendMessage(netMsg)
}

// Broadcast gửi một thông điệp đến tất cả các peer.
func (n *Node) Broadcast(command string, body []byte) {
	for peerID := range n.peers {
		if peerID == n.id {
			continue
		}
		go n.Send(peerID, command, body)
	}
}

func (n *Node) onConnect(conn t_network.Connection) {
	logger.Info("Node %d sees a new connection from %s", n.id, conn.RemoteAddrSafe())
}

func (n *Node) onDisconnect(conn t_network.Connection) {
	n.connMutex.Lock()
	defer n.connMutex.Unlock()
	for id, c := range n.connections {
		if c == conn {
			logger.Warn("Node %d disconnected from Node %d", n.id, id)
			delete(n.connections, id)
			return
		}
	}
	logger.Warn("Node %d disconnected from an unknown peer at %s", n.id, conn.RemoteAddrSafe())
}

func (n *Node) AddConnection(peerID int32, conn t_network.Connection) {
	n.connMutex.Lock()
	defer n.connMutex.Unlock()
	if existingConn, ok := n.connections[peerID]; ok && existingConn.IsConnect() {
		return
	}
	n.connections[peerID] = conn
}

func (n *Node) GetConnection(peerID int32) (t_network.Connection, bool) {
	n.connMutex.RLock()
	defer n.connMutex.RUnlock()
	conn, ok := n.connections[peerID]
	return conn, ok
}
