package main

import (
	"context"
	"log"
	"sync"
	"time"

	"github.com/anthdm/hbbft"
)

// Server đại diện cho một node trong mạng đồng thuận.
type Server struct {
	id        uint64
	hb        *hbbft.HoneyBadger
	router    *NetworkRouter
	mempool   map[string]*Transaction
	lock      sync.RWMutex
	startTime time.Time

	// Metrics
	totalCommit      int
	messagesReceived int
	messagesSent     int
	lastCommitTime   time.Time
}

// newServer tạo một instance Server mới.
func newServer(id uint64, nodes []uint64, router *NetworkRouter, batchSize int) *Server {
	cfg := hbbft.Config{
		N:         len(nodes),
		ID:        id,
		Nodes:     nodes,
		BatchSize: batchSize,
	}
	hb := hbbft.NewHoneyBadger(cfg)

	return &Server{
		id:             id,
		hb:             hb,
		mempool:        make(map[string]*Transaction),
		router:         router,
		startTime:      time.Now(),
		lastCommitTime: time.Now(),
	}
}

// addTransactions thêm các giao dịch vào mempool và đưa vào HoneyBadger.
func (s *Server) addTransactions(txs ...*Transaction) {
	s.lock.Lock()
	defer s.lock.Unlock()

	for _, tx := range txs {
		hashKey := string(tx.Hash())
		if _, ok := s.mempool[hashKey]; !ok {
			// === SỬA ĐỔI TẠI ĐÂY ===
			// Chỉ cần gọi hàm trực tiếp.
			s.hb.AddTransaction(tx)

			// Thêm vào mempool sau khi đã đưa vào HoneyBadger.
			s.mempool[hashKey] = tx
		}
	}
}

// start khởi động các vòng lặp xử lý chính của server.
func (s *Server) start(ctx context.Context, wg *sync.WaitGroup) {
	defer wg.Done()
	log.Printf("Node %d đang khởi động...", s.id)

	if err := s.hb.Start(); err != nil {
		log.Fatalf("Node %d: Không thể khởi động HoneyBadger: %v", s.id, err)
	}

	s.sendMessages() // Gửi các tin nhắn khởi tạo nếu có

	msgChan := s.router.GetMessageChannel(s.id)
	txTicker := time.NewTicker(5 * time.Second)
	defer txTicker.Stop()
	statsTicker := time.NewTicker(10 * time.Second)
	defer statsTicker.Stop()

	log.Printf("Node %d đã sẵn sàng.", s.id)

	for {
		select {
		case <-ctx.Done():
			log.Printf("Node %d đang tắt...", s.id)
			return
		case msg := <-msgChan:
			s.handleMessage(msg)
		case <-txTicker.C:
			// Tạo và thêm các giao dịch mới một cách định kỳ
			newTxs := make([]*Transaction, 100)
			for i := range newTxs {
				newTxs[i] = newTransaction()
			}
			s.addTransactions(newTxs...)
		case <-statsTicker.C:
			s.printStats()
		}
	}
}

// handleMessage xử lý một tin nhắn đến từ mạng.
func (s *Server) handleMessage(msg Message) {
	s.lock.Lock()
	s.messagesReceived++
	s.lock.Unlock()

	hbmsg, ok := msg.Payload.Payload.(hbbft.HBMessage)
	if !ok {
		log.Printf("Node %d: Không thể ép kiểu tin nhắn", s.id)
		return
	}

	if err := s.hb.HandleMessage(msg.From, hbmsg.Epoch, hbmsg.Payload.(*hbbft.ACSMessage)); err != nil {
		log.Printf("Node %d: Lỗi khi xử lý tin nhắn từ %d: %v", s.id, msg.From, err)
	}

	s.sendMessages()
	s.processOutputs()
}

// sendMessages gửi tất cả các tin nhắn đang chờ từ HoneyBadger ra mạng.
func (s *Server) sendMessages() {
	messages := s.hb.Messages()
	if len(messages) == 0 {
		return
	}

	s.lock.Lock()
	s.messagesSent += len(messages)
	s.lock.Unlock()

	for _, msgTuple := range messages {
		msg := Message{
			From:    s.id,
			To:      msgTuple.To,
			Payload: msgTuple,
		}
		s.router.SendMessage(msg)
	}
}

// processOutputs xử lý các batch giao dịch đã được đồng thuận.
func (s *Server) processOutputs() {
	s.lock.Lock()
	defer s.lock.Unlock()

	for epoch, transactions := range s.hb.Outputs() {
		commitCount := len(transactions)
		s.totalCommit += commitCount
		s.lastCommitTime = time.Now()

		log.Printf("✅ Node %d đã commit %d giao dịch tại epoch %d. Tổng số: %d", s.id, commitCount, epoch, s.totalCommit)

		// Xóa các giao dịch đã commit khỏi mempool
		for _, tx := range transactions {
			delete(s.mempool, string(tx.Hash()))
		}
	}
}

// printStats in ra các số liệu thống kê hiện tại của node.
func (s *Server) printStats() {
	s.lock.RLock()
	defer s.lock.RUnlock()

	timeSinceLastCommit := time.Since(s.lastCommitTime).Truncate(time.Second)
	log.Printf(
		"📊 Node %d Stats: Commits=%d | Sent=%d | Received=%d | Mempool=%d | LastCommit=%v ago",
		s.id, s.totalCommit, s.messagesSent, s.messagesReceived, len(s.mempool), timeSinceLastCommit,
	)
}

// getStats trả về các số liệu thống kê chính của node một cách an toàn.
func (s *Server) getStats() (int, int, int, int) {
	s.lock.RLock()
	defer s.lock.RUnlock()
	return s.totalCommit, s.messagesSent, s.messagesReceived, len(s.mempool)
}
