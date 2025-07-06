package core

import (
	"github.com/meta-node-blockchain/meta-node/pkg/config"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
)

// Host định nghĩa các chức năng mà một node mạng cung cấp cho các module logic.
// Các module (RBC, BBA) sẽ phụ thuộc vào interface này thay vì một implementation cụ thể.
type Host interface {
	// ID trả về ID của node.
	ID() int32
	// Config trả về cấu hình của node.
	Config() *config.NodeConfig
	// Broadcast gửi một thông điệp đến tất cả các peer.
	Broadcast(command string, body []byte)
	// Send gửi một thông điệp đến một peer cụ thể.
	Send(targetID int32, command string, body []byte) error
	// MasterConn trả về kết nối đến Master Node.
	MasterConn() t_network.Connection
}
