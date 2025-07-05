package node

import (
	"fmt"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/meta-node-blockchain/meta-node/pkg/bls"
	m_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/network"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
)

// Node là trung tâm quản lý mạng, kết nối và các thông điệp.
type Node struct {
	Config      *NodeConfig
	ID          int32
	KeyPair     *bls.KeyPair
	Peers       map[int32]string
	server      t_network.SocketServer
	connections map[int32]t_network.Connection
	connMutex   sync.RWMutex
	MasterConn  t_network.Connection
}

// NewNode khởi tạo một đối tượng Node mới từ cấu hình.
func NewNode(config *NodeConfig, handler t_network.Handler) (*Node, error) {
	keyPair := bls.NewKeyPair(common.FromHex(config.KeyPair))
	if keyPair == nil {
		keyPair = bls.GenerateKeyPair()
	}

	peers := make(map[int32]string)
	for _, nodeConf := range config.Peers {
		peers[int32(nodeConf.Id)] = nodeConf.ConnectionAddress
	}

	node := &Node{
		Config:      config,
		ID:          int32(config.ID),
		KeyPair:     keyPair,
		Peers:       peers,
		connections: make(map[int32]t_network.Connection),
	}

	connectionsManager := network.NewConnectionsManager()
	var err error
	node.server, err = network.NewSocketServer(node.KeyPair, connectionsManager, handler, "validator", "0.0.1")
	if err != nil {
		return nil, fmt.Errorf("failed to create socket server: %w", err)
	}
	node.server.AddOnConnectedCallBack(node.onConnect)
	node.server.AddOnDisconnectedCallBack(node.onDisconnect)

	return node, nil
}

// Start khởi chạy node, lắng nghe kết nối và kết nối đến các peers.
func (n *Node) Start() error {
	addr := n.Config.ConnectionAddress
	go func() {
		logger.Info("Node %d listening on %s", n.ID, addr)
		if err := n.server.Listen(addr); err != nil {
			logger.Error("Server listening error on node %d: %v", n.ID, err)
		}
	}()

	time.Sleep(time.Second * 2) // Chờ các node khác khởi động

	// Kết nối đến Master
	logger.Info("Node %d attempting to connect to Master at %s", n.ID, n.Config.Master.ConnectionAddress)
	masterConn := network.NewConnection(common.HexToAddress("0x0"), m_common.MASTER_CONNECTION_TYPE)
	masterConn.SetRealConnAddr(n.Config.Master.ConnectionAddress)
	if err := masterConn.Connect(); err != nil {
		logger.Error("Node %d failed to connect to Master: %v", n.ID, err)
	} else {
		n.MasterConn = masterConn
		n.AddConnection(-1, masterConn) // -1 là ID cho Master
		go n.server.HandleConnection(masterConn)
		logger.Info("Node %d connected to Master", n.ID)
	}

	// Kết nối đến các peers khác
	for peerID, peerAddr := range n.Peers {
		if peerID == n.ID {
			continue
		}
		conn := network.NewConnection(common.HexToAddress("0x0"), "rbc_message")
		conn.SetRealConnAddr(peerAddr)
		logger.Info("Node %d attempting to connect to Node %d at %s", n.ID, peerID, peerAddr)
		err := conn.Connect()
		if err != nil {
			logger.Warn("Node %d failed to connect to Node %d: %v", n.ID, peerID, err)
			continue
		}
		n.AddConnection(peerID, conn)
		go n.server.HandleConnection(conn)
	}
	return nil
}

// Stop dừng hoạt động của node.
func (n *Node) Stop() {
	n.server.Stop()
}

// onConnect là callback khi có kết nối mới.
func (n *Node) onConnect(conn t_network.Connection) {
	logger.Info("Node %d sees a new connection from %s", n.ID, conn.RemoteAddrSafe())
}

// onDisconnect là callback khi một kết nối bị mất.
func (n *Node) onDisconnect(conn t_network.Connection) {
	n.connMutex.Lock()
	defer n.connMutex.Unlock()
	for id, c := range n.connections {
		if c == conn {
			logger.Warn("Node %d disconnected from Node %d", n.ID, id)
			delete(n.connections, id)
			return
		}
	}
	logger.Warn("Node %d disconnected from an unknown peer at %s", n.ID, conn.RemoteAddrSafe())
}

// AddConnection thêm một kết nối vào map quản lý.
func (n *Node) AddConnection(peerID int32, conn t_network.Connection) {
	n.connMutex.Lock()
	defer n.connMutex.Unlock()
	if existingConn, ok := n.connections[peerID]; ok && existingConn.IsConnect() {
		return // Đã có kết nối
	}
	n.connections[peerID] = conn
}

// GetConnection trả về một kết nối dựa trên peer ID.
func (n *Node) GetConnection(peerID int32) (t_network.Connection, bool) {
	n.connMutex.RLock()
	defer n.connMutex.RUnlock()
	conn, ok := n.connections[peerID]
	return conn, ok
}

// Send gửi một thông điệp đến một peer cụ thể.
func (n *Node) Send(targetID int32, command string, body []byte) error {
	conn, ok := n.GetConnection(targetID)
	if !ok || !conn.IsConnect() {
		return fmt.Errorf("node %d not connected to target %d", n.ID, targetID)
	}

	netMsg := network.NewMessage(&pb.Message{
		Header: &pb.Header{Command: command},
		Body:   body,
	})

	return conn.SendMessage(netMsg)
}

// Broadcast gửi một thông điệp đến tất cả các peers.
func (n *Node) Broadcast(command string, body []byte) {
	for peerID := range n.Peers {
		if peerID == n.ID {
			continue // Bỏ qua việc gửi cho chính mình
		}
		go n.Send(peerID, command, body)
	}
}
