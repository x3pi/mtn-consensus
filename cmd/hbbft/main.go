package main

import (
	"encoding/binary"
	"fmt"
	"log"
	"math/rand"
	"sync"
	"time"

	"encoding/gob"

	"github.com/anthdm/hbbft"
)

// Transaction là một triển khai đơn giản của giao diện hbbft.Transaction
type Transaction struct {
	Nonce uint64
}

// Hash trả về một hash duy nhất cho giao dịch.
func (t *Transaction) Hash() []byte {
	buf := make([]byte, 8)
	binary.LittleEndian.PutUint64(buf, t.Nonce)
	return buf
}

func newTransaction() *Transaction {
	return &Transaction{rand.Uint64()}
}

// message là một cấu trúc để giữ các thông điệp được trao đổi giữa các node.
type message struct {
	from    uint64
	payload hbbft.MessageTuple
}

// Server đại diện cho một node trong mạng.
type Server struct {
	id          uint64
	hb          *hbbft.HoneyBadger
	mempool     map[string]*Transaction
	lock        sync.RWMutex
	totalCommit int
	start       time.Time
}

func newServer(id uint64, nodes []uint64) *Server {
	cfg := hbbft.Config{
		N:         len(nodes),
		ID:        id,
		Nodes:     nodes,
		BatchSize: 100, // Kích thước batch cho mỗi epoch
	}
	hb := hbbft.NewHoneyBadger(cfg)
	return &Server{
		id:      id,
		hb:      hb,
		mempool: make(map[string]*Transaction),
		start:   time.Now(),
	}
}

// addTransactions thêm các giao dịch vào mempool và hbbft.
func (s *Server) addTransactions(txs ...*Transaction) {
	for _, tx := range txs {
		s.lock.Lock()
		if _, ok := s.mempool[string(tx.Hash())]; !ok {
			s.mempool[string(tx.Hash())] = tx
			s.hb.AddTransaction(tx)
		}
		s.lock.Unlock()
	}
}

func main() {
	rand.Seed(time.Now().UnixNano())
	gob.Register(&Transaction{})

	const numNodes = 4
	var nodes []*Server
	nodeIDs := make([]uint64, numNodes)
	for i := 0; i < numNodes; i++ {
		nodeIDs[i] = uint64(i)
	}

	for i := 0; i < numNodes; i++ {
		server := newServer(uint64(i), nodeIDs)
		nodes = append(nodes, server)
	}

	messages := make(chan message, 1024*1024)

	// Bắt đầu các node và gửi các thông điệp khởi tạo.
	for _, node := range nodes {
		// Thêm một số giao dịch ban đầu
		for i := 0; i < 10000; i++ {
			node.addTransactions(newTransaction())
		}

		if err := node.hb.Start(); err != nil {
			log.Fatalf("Lỗi khi khởi động node %d: %v", node.id, err)
		}
		// Gửi các thông điệp ban đầu đến người nhận được chỉ định
		for _, msg := range node.hb.Messages() {
			messages <- message{from: node.id, payload: msg}
		}
	}

	// Vòng lặp xử lý thông điệp chính.
	go func() {
		for msg := range messages {
			// Định tuyến thông điệp đến đúng node nhận
			if int(msg.payload.To) >= len(nodes) {
				log.Printf("Lỗi: Người nhận không hợp lệ %d", msg.payload.To)
				continue
			}
			node := nodes[msg.payload.To]

			// Xử lý thông điệp
			hbmsg := msg.payload.Payload.(hbbft.HBMessage)
			if err := node.hb.HandleMessage(msg.from, hbmsg.Epoch, hbmsg.Payload.(*hbbft.ACSMessage)); err != nil {
				// Lỗi này giờ đây không nên xảy ra thường xuyên
				log.Printf("Lỗi xử lý thông điệp tại node %d từ node %d: %v", node.id, msg.from, err)
			}

			// Gửi các thông điệp phản hồi đến đúng người nhận
			for _, responseMsg := range node.hb.Messages() {
				messages <- message{from: node.id, payload: responseMsg}
			}
		}
	}()

	// Vòng lặp commit và in kết quả.
	ticker := time.NewTicker(2 * time.Second)
	defer ticker.Stop()
	lastCommitCount := 0
	for range ticker.C {
		totalCommits := 0
		for _, node := range nodes {
			outputs := node.hb.Outputs()
			for epoch, txx := range outputs {
				node.totalCommit += len(txx)
				fmt.Printf("Node %d đã commit %d giao dịch trong epoch %d. Tổng số đã commit: %d\n", node.id, len(txx), epoch, node.totalCommit)
			}
			// Thêm giao dịch mới một cách định kỳ để duy trì hoạt động
			for i := 0; i < 10000; i++ {
				node.addTransactions(newTransaction())
			}
			totalCommits += node.totalCommit
		}
		if totalCommits > lastCommitCount {
			fmt.Println("--- Epoch mới đã được commit! ---")
			lastCommitCount = totalCommits
		} else {
			fmt.Println("--- Đang chờ epoch tiếp theo... ---")
		}
	}
}
