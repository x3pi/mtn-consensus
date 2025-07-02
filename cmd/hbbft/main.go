package main

import (
	"context"
	"encoding/gob"
	"fmt"
	"log"
	"math/rand"
	"sync"
	"time"
)

func main() {
	rand.Seed(time.Now().UnixNano())
	// Đăng ký kiểu Transaction để có thể mã hóa/giải mã qua mạng.
	gob.Register(&Transaction{})

	// Cấu hình mô phỏng
	const (
		numNodes       = 4
		initialTxCount = 1000 // Giao dịch ban đầu cho mỗi node
		batchSize      = 100  // Kích thước batch của HBBFT
		simulationTime = 2 * time.Minute
		networkLatency = 100 * time.Millisecond
		statsInterval  = 15 * time.Second
	)

	// 1. Khởi tạo Network Router
	router := NewNetworkRouter(networkLatency)

	// 2. Tạo Node IDs
	nodeIDs := make([]uint64, numNodes)
	for i := 0; i < numNodes; i++ {
		nodeIDs[i] = uint64(i)
	}

	// 3. Khởi tạo các Server (Node)
	servers := make([]*Server, numNodes)
	for i := 0; i < numNodes; i++ {
		server := newServer(uint64(i), nodeIDs, router, batchSize)
		servers[i] = server
		router.RegisterNode(uint64(i), server)

		// Thêm các giao dịch ban đầu vào mempool của mỗi node
		initialTxs := make([]*Transaction, initialTxCount)
		for j := range initialTxs {
			initialTxs[j] = newTransaction()
		}
		server.addTransactions(initialTxs...)
	}

	// 4. Khởi động các node
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var wg sync.WaitGroup
	wg.Add(numNodes)

	for _, s := range servers {
		go s.start(ctx, &wg)
	}
	log.Println("🚀 Tất cả các node đã được khởi động. Bắt đầu mô phỏng...")

	// 5. Vòng lặp chính để theo dõi và in thống kê tổng hợp
	mainTicker := time.NewTicker(statsInterval)
	defer mainTicker.Stop()
	timeout := time.NewTimer(simulationTime)
	defer timeout.Stop()

	startTime := time.Now()

	for {
		select {
		case <-timeout.C:
			log.Println("⏳ Mô phỏng kết thúc. Đang chờ các node tắt...")
			cancel()  // Gửi tín hiệu dừng đến tất cả các goroutine
			wg.Wait() // Chờ tất cả các node kết thúc
			log.Println("✅ Tất cả các node đã tắt.")
			printFinalStats(servers, time.Since(startTime))
			return

		case <-mainTicker.C:
			printAggregatedStats(servers, time.Since(startTime))
		}
	}
}

// printAggregatedStats in thống kê tổng hợp từ tất cả các node.
func printAggregatedStats(nodes []*Server, runningTime time.Duration) {
	var totalCommits, totalSent, totalReceived, totalMempool int
	for _, node := range nodes {
		commits, sent, received, mempool := node.getStats()
		// Vì các block được commit trên tất cả các node, chỉ cần lấy từ một node là đủ.
		if totalCommits == 0 {
			totalCommits = commits
		}
		totalSent += sent
		totalReceived += received
		totalMempool += mempool
	}

	fmt.Println("\n--- THỐNG KÊ TỔNG QUÁT ---")
	fmt.Printf("Thời gian chạy: %s\n", runningTime.Truncate(time.Second))
	fmt.Printf("Tổng số Giao dịch đã Commit (ước tính): %d\n", totalCommits)
	fmt.Printf("Tổng số Tin nhắn (Gửi/Nhận): %d / %d\n", totalSent, totalReceived)
	fmt.Printf("Tổng số Giao dịch trong Mempools: %d\n", totalMempool)
	fmt.Println("--------------------------\n")
}

// printFinalStats in thống kê cuối cùng khi mô phỏng kết thúc.
func printFinalStats(nodes []*Server, runningTime time.Duration) {
	fmt.Println("\n\n--- KẾT QUẢ MÔ PHỎNG CUỐI CÙNG ---")
	printAggregatedStats(nodes, runningTime)

	var totalCommits int
	if len(nodes) > 0 {
		totalCommits, _, _, _ = nodes[0].getStats()
	}

	if runningTime.Seconds() > 0 {
		tps := float64(totalCommits) / runningTime.Seconds()
		fmt.Printf("Thông lượng (TPS - Giao dịch mỗi giây): %.2f\n", tps)
	}
	fmt.Println("------------------------------------")
}
