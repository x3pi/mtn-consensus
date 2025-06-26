package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"io/ioutil"
	"log"
	"os"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"

	// Assuming this is the correct import path for your protobuf definitions

	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
)

// ValidatorInfo contains public key of a validator.

func LoadConfigFromFile(filename string) (*rbc.NodeConfig, error) {
	data, err := ioutil.ReadFile(filename)
	if err != nil {
		return nil, fmt.Errorf("could not read config file: %w", err)
	}
	var config rbc.NodeConfig
	if err := json.Unmarshal(data, &config); err != nil {
		return nil, fmt.Errorf("could not unmarshal json: %w", err)
	}
	return &config, nil
}

func main() {
	// Cờ lệnh giờ chỉ cần ID của node và đường dẫn tới file config
	configFile := flag.String("config", "config.json", "Configuration file name")
	flag.Parse()

	// Tải cấu hình chứa tất cả các node
	allConfig, err := LoadConfigFromFile(*configFile)
	if err != nil {
		log.Fatalf("Error loading config: %v", err)
	}

	// Khởi tạo process với ID và danh sách peers đã được đọc từ config
	process, err := rbc.NewProcess(allConfig)
	if err != nil {
		log.Fatalf("Failed to create process: %v", err)
	}

	// Start the server and connect to peers
	go func() {
		if err := process.Start(); err != nil {
			log.Fatalf("Process failed to start: %v", err)
		}
	}()

	// Allow user to broadcast messages from stdin (giữ nguyên)
	logger.Info("Enter a message and press Enter to broadcast:")
	scanner := bufio.NewScanner(os.Stdin)
	for {
		fmt.Print("> ")
		if scanner.Scan() {
			line := scanner.Text()
			if line != "" {
				process.StartBroadcast([]byte(line), "string")
			}
		} else {
			if err := scanner.Err(); err != nil {
				logger.Info("Error reading from stdin: %v", err)
			}
			break
		}
	}
}
