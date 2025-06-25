package main

import (
	"bufio"
	"flag"
	"fmt"
	"log"
	"os"
	"reliable-broadcast/rbc" // Đảm bảo đường dẫn import đúng
	"strconv"
	"strings"
	"time" // Thêm import time cho StartBroadcast
)

func main() {
	id := flag.Int("id", 0, "ID của node này")
	peersStr := flag.String("peers", "0:localhost:8000", "Danh sách các peer dạng id:addr,id:addr")
	flag.Parse()

	peers := make(map[int32]string)
	for _, pStr := range strings.Split(*peersStr, ",") {
		// SỬA LỖI Ở ĐÂY: Dùng SplitN để chỉ tách tại dấu hai chấm đầu tiên
		parts := strings.SplitN(pStr, ":", 2)
		if len(parts) != 2 {
			log.Fatalf("Định dạng peer không hợp lệ: %s. Phải là 'id:host:port'", pStr)
		}
		pID, err := strconv.Atoi(parts[0])
		if err != nil {
			log.Fatalf("ID peer không hợp lệ: %v", err)
		}
		peers[int32(pID)] = parts[1]
	}

	process := rbc.NewProcess(int32(*id), peers)

	// Bắt đầu lắng nghe mạng trong một goroutine
	go process.Start()

	// Goroutine để in các thông điệp đã được giao
	go func() {
		for {
			payload := <-process.Delivered
			fmt.Printf("\n[ỨNG DỤNG] Node %d đã nhận: %s\n> ", process.ID, string(payload))
		}
	}()

	// Đợi một chút để các node khác khởi động và lắng nghe
	time.Sleep(2 * time.Second)

	// Cho phép người dùng nhập từ stdin để bắt đầu broadcast
	fmt.Println("Nhập tin nhắn và nhấn Enter để broadcast:")
	scanner := bufio.NewScanner(os.Stdin)
	for {
		fmt.Print("> ")
		if scanner.Scan() {
			line := scanner.Text()
			if line != "" {
				process.StartBroadcast([]byte(line))
			}
		} else {
			if err := scanner.Err(); err != nil {
				log.Printf("Lỗi đọc stdin: %v", err)
			}
			break
		}
	}
}
