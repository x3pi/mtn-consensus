package main

import (
	"log"
	"math/rand"
	"sync"
	"time"

	"github.com/anthdm/hbbft"
)

// Message đại diện cho một tin nhắn được gửi qua mạng.
type Message struct {
	From    uint64
	To      uint64
	Payload hbbft.MessageTuple
}

// NetworkRouter mô phỏng một mạng phân tán với độ trễ.
type NetworkRouter struct {
	nodes    map[uint64]*Server
	channels map[uint64]chan Message
	latency  time.Duration
	mu       sync.RWMutex
}

// NewNetworkRouter khởi tạo một network router mới.
func NewNetworkRouter(latency time.Duration) *NetworkRouter {
	return &NetworkRouter{
		nodes:    make(map[uint64]*Server),
		channels: make(map[uint64]chan Message),
		latency:  latency,
	}
}

// RegisterNode đăng ký một node mới với router.
func (nr *NetworkRouter) RegisterNode(id uint64, server *Server) {
	nr.mu.Lock()
	defer nr.mu.Unlock()
	nr.nodes[id] = server
	// Tạo một channel có buffer để tránh bị chặn khi gửi tin nhắn.
	nr.channels[id] = make(chan Message, 1024)
}

// SendMessage gửi một tin nhắn đến một node cụ thể với độ trễ mô phỏng.
func (nr *NetworkRouter) SendMessage(msg Message) {
	go func() {
		// Thêm một chút biến thiên ngẫu nhiên vào độ trễ
		time.Sleep(nr.latency + time.Duration(rand.Intn(20))*time.Millisecond)
		nr.mu.RLock()
		defer nr.mu.RUnlock()

		if ch, exists := nr.channels[msg.To]; exists {
			select {
			case ch <- msg:
			// Tin nhắn đã được gửi thành công
			default:
				log.Printf("Cảnh báo: Kênh tin nhắn cho node %d đã đầy. Tin nhắn bị loại bỏ.", msg.To)
			}
		} else {
			log.Printf("Cảnh báo: Không tìm thấy kênh cho node %d.", msg.To)
		}
	}()
}

// GetMessageChannel trả về channel chỉ đọc để một node nhận tin nhắn.
func (nr *NetworkRouter) GetMessageChannel(nodeID uint64) <-chan Message {
	nr.mu.RLock()
	defer nr.mu.RUnlock()
	return nr.channels[nodeID]
}
