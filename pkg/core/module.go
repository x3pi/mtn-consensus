package core

import (
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
)

// Module là một interface chung cho các thành phần logic (như RBC, BBA)
// có thể được đăng ký và quản lý bởi một Node.
type Module interface {
	// CommandHandlers trả về một map các lệnh và hàm xử lý tương ứng của module.
	CommandHandlers() map[string]func(t_network.Request) error
	// Start khởi chạy các tiến trình nền của module.
	Start()
	// Stop dừng các tiến trình của module.
	Stop()
}
