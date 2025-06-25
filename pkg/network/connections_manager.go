package network

import (
	"fmt"
	"sync"

	"github.com/ethereum/go-ethereum/common"
	"github.com/holiman/uint256"

	p_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/types/network"
)

const (
	// MaxConnectionTypes ước tính số lượng loại kết nối tối đa.
	MaxConnectionTypes = 20
)

// ConnectionsManager quản lý tập hợp các kết nối mạng, được phân loại theo loại.
// Nó cũng theo dõi một kết nối "cha" đặc biệt.
type ConnectionsManager struct {
	mu                          sync.RWMutex                            // mu bảo vệ các truy cập đồng thời vào các trường của ConnectionsManager.
	parentConnection            network.Connection                      // parentConnection là kết nối đến node cha (nếu có).
	typeToMapAddressConnections []map[common.Address]network.Connection // typeToMapAddressConnections là một slice các map
}

// NewConnectionsManager tạo một instance mới của ConnectionsManager.
func NewConnectionsManager() network.ConnectionsManager {
	cm := &ConnectionsManager{}
	cm.typeToMapAddressConnections = make([]map[common.Address]network.Connection, MaxConnectionTypes)
	for i := range cm.typeToMapAddressConnections {
		cm.typeToMapAddressConnections[i] = make(map[common.Address]network.Connection)
	}
	return cm
}

// ConnectionsByType trả về một map chứa tất cả các kết nối của một loại (cType) cụ thể.
func (cm *ConnectionsManager) ConnectionsByType(cType int) map[common.Address]network.Connection {
	cm.mu.RLock()
	defer cm.mu.RUnlock()

	if cType < 0 || cType >= len(cm.typeToMapAddressConnections) {
		return make(map[common.Address]network.Connection)
	}
	connectionsMap := cm.typeToMapAddressConnections[cType]
	copiedMap := make(map[common.Address]network.Connection, len(connectionsMap))
	for addr, conn := range connectionsMap {
		copiedMap[addr] = conn
	}
	return copiedMap
}

// ConnectionByTypeAndAddress trả về một kết nối cụ thể dựa trên loại và địa chỉ.
func (cm *ConnectionsManager) ConnectionByTypeAndAddress(cType int, address common.Address) network.Connection {
	cm.mu.RLock()
	defer cm.mu.RUnlock()

	if cType < 0 || cType >= len(cm.typeToMapAddressConnections) {
		return nil
	}

	connectionsMapForType := cm.typeToMapAddressConnections[cType]
	if connectionsMapForType == nil {
		return nil
	}

	conn, ok := connectionsMapForType[address]
	if !ok {
		return nil
	}
	return conn
}

// ConnectionsByTypeAndAddresses trả về một map các kết nối cho một loại cụ thể, lọc theo danh sách địa chỉ được cung cấp.
func (cm *ConnectionsManager) ConnectionsByTypeAndAddresses(cType int, addresses []common.Address) map[common.Address]network.Connection {
	cm.mu.RLock()
	defer cm.mu.RUnlock()

	if cType < 0 || cType >= len(cm.typeToMapAddressConnections) {
		return make(map[common.Address]network.Connection)
	}

	connectionsMap := cm.typeToMapAddressConnections[cType]
	if connectionsMap == nil {
		return make(map[common.Address]network.Connection)
	}

	result := make(map[common.Address]network.Connection, len(addresses))
	for _, addr := range addresses {
		if conn, ok := connectionsMap[addr]; ok && conn != nil { // Đảm bảo conn không nil trước khi thêm
			result[addr] = conn
		}
	}
	return result
}

// FilterAddressAvailable lọc một map địa chỉ, chỉ giữ lại những địa chỉ có kết nối hoạt động thuộc một loại cụ thể.
func (cm *ConnectionsManager) FilterAddressAvailable(cType int, addresses map[common.Address]*uint256.Int) map[common.Address]*uint256.Int {
	cm.mu.RLock()
	defer cm.mu.RUnlock()

	if cType < 0 || cType >= len(cm.typeToMapAddressConnections) {
		return make(map[common.Address]*uint256.Int)
	}

	connectionsMap := cm.typeToMapAddressConnections[cType]
	if connectionsMap == nil {
		return make(map[common.Address]*uint256.Int)
	}

	availableAddresses := make(map[common.Address]*uint256.Int)
	for address, value := range addresses {
		if conn, ok := connectionsMap[address]; ok && conn != nil && conn.IsConnect() {
			availableAddresses[address] = value
		}
	}
	return availableAddresses
}

// ParentConnection trả về kết nối cha.
func (cm *ConnectionsManager) ParentConnection() network.Connection {
	cm.mu.RLock()
	defer cm.mu.RUnlock()
	return cm.parentConnection
}

// Stats trả về thống kê về số lượng kết nối theo loại.
func (cm *ConnectionsManager) Stats() *pb.NetworkStats {
	cm.mu.RLock()
	defer cm.mu.RUnlock()

	pbNetworkStats := &pb.NetworkStats{
		TotalConnectionByType: make(map[string]int32, len(cm.typeToMapAddressConnections)),
	}
	for i, connectionsMap := range cm.typeToMapAddressConnections {
		connectionTypeName := p_common.MapIndexToConnectionType(i)
		if connectionTypeName == "" && len(connectionsMap) > 0 {
			connectionTypeName = fmt.Sprintf("UNKNOWN_TYPE_%d", i)
		}
		// Chỉ thêm vào stats nếu có tên loại hợp lệ hoặc có kết nối (để không bỏ sót UNKNOWN_TYPE nếu có)
		if connectionTypeName != "" || len(connectionsMap) > 0 {
			pbNetworkStats.TotalConnectionByType[connectionTypeName] = int32(len(connectionsMap))
		}
	}
	// Thông tin về kết nối cha có thể được thêm vào đây nếu pb.NetworkStats có các trường tương ứng.
	// Ví dụ:
	// if cm.parentConnection != nil {
	//    pbNetworkStats.ParentConnectionAddress = cm.parentConnection.Address().Hex()
	//    pbNetworkStats.ParentConnectionType = cm.parentConnection.Type()
	//    pbNetworkStats.IsParentConnected = cm.parentConnection.IsConnect()
	// }
	return pbNetworkStats
}

// AddParentConnection đặt kết nối cha.
func (cm *ConnectionsManager) AddParentConnection(conn network.Connection) {
	cm.mu.Lock()
	defer cm.mu.Unlock()
	if conn == nil {
		return
	}
	if cm.parentConnection != nil && cm.parentConnection != conn {
		// Cân nhắc ngắt kết nối cha cũ nếu nó đang hoạt động và không phải là kết nối mới
		// if cm.parentConnection.IsConnect() {
		// go cm.parentConnection.Disconnect() // Ngắt kết nối trong goroutine để không block
		// }
	}
	cm.parentConnection = conn
}

// RemoveConnection xóa một kết nối khỏi trình quản lý.
func (cm *ConnectionsManager) RemoveConnection(conn network.Connection) {
	cm.mu.Lock()
	defer cm.mu.Unlock()

	if conn == nil {
		return
	}

	address := conn.Address()
	cTypeStr := conn.Type()
	cType := p_common.MapConnectionTypeToIndex(cTypeStr)

	if cType < 0 || cType >= len(cm.typeToMapAddressConnections) {
		return
	}

	connectionsMap := cm.typeToMapAddressConnections[cType]
	if connectionsMap == nil {
		return
	}

	if existingConn, ok := connectionsMap[address]; ok && existingConn == conn {
		delete(connectionsMap, address)
	} else if ok {
	} else {
	}

	// Kiểm tra và xóa nếu đó là kết nối cha
	if cm.parentConnection == conn {
		cm.parentConnection = nil
	}
}

// AddConnection thêm một kết nối mới vào trình quản lý.
func (cm *ConnectionsManager) AddConnection(conn network.Connection, replace bool) {
	cm.mu.Lock()
	defer cm.mu.Unlock()

	if conn == nil {
		return
	}

	address := conn.Address()
	cTypeStr := conn.Type()
	if cTypeStr == "" {
		// Địa chỉ có thể là zero nếu kết nối chưa được Init, nhưng loại thì nên có.
		return
	}
	cType := p_common.MapConnectionTypeToIndex(cTypeStr)

	if cType < 0 || cType >= len(cm.typeToMapAddressConnections) {
		return
	}

	if cm.typeToMapAddressConnections[cType] == nil {
		cm.typeToMapAddressConnections[cType] = make(map[common.Address]network.Connection)
	}
	connectionsMap := cm.typeToMapAddressConnections[cType]

	// Thông thường, địa chỉ không nên là zero trừ khi đó là một kết nối đang chờ Init.
	// Nếu địa chỉ là zero, có thể nó sẽ được cập nhật sau qua conn.Init().
	if address == (common.Address{}) {
	}

	existingConn, exists := connectionsMap[address]

	if !exists || replace {
		if replace && exists && existingConn != nil && existingConn != conn {
			if existingConn.IsConnect() {
				// Chạy Disconnect trong một goroutine để tránh block AddConnection,
				// đặc biệt nếu Disconnect có thể mất thời gian.
				go func(oldConn network.Connection) {
					if err := oldConn.Disconnect(); err != nil {
					}
				}(existingConn)
			}
		}
		connectionsMap[address] = conn
	} else {

	}
}

// MapAddressConnectionToInterface là một hàm tiện ích để chuyển đổi kiểu của map.
func MapAddressConnectionToInterface(data map[common.Address]network.Connection) map[common.Address]interface{} {
	rs := make(map[common.Address]interface{}, len(data))
	for i, v := range data {
		rs[i] = v
	}
	return rs
}
