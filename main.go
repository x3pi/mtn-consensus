package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"strings"

	"github.com/meta-node-blockchain/meta-node/pkg/bba"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/network"
	"github.com/meta-node-blockchain/meta-node/pkg/node"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
	t_network "github.com/meta-node-blockchain/meta-node/types/network"
)

// LoadConfigFromFile đọc và phân tích cú pháp tệp cấu hình.
func LoadConfigFromFile(filename string) (*node.NodeConfig, error) {
	data, err := os.ReadFile(filename)
	if err != nil {
		return nil, fmt.Errorf("could not read config file: %w", err)
	}
	var config node.NodeConfig
	if err := json.Unmarshal(data, &config); err != nil {
		return nil, fmt.Errorf("could not unmarshal json: %w", err)
	}
	return &config, nil
}

func main() {
	configFile := flag.String("config", "config.json", "Configuration file name")
	flag.Parse()

	config, err := LoadConfigFromFile(*configFile)
	if err != nil {
		log.Fatalf("Error loading config: %v", err)
	}

	// 1. Khởi tạo tất cả các process
	rbcProcess, err := rbc.NewProcess()
	if err != nil {
		log.Fatalf("Failed to create rbc process: %v", err)
	}
	bbaProcess, err := bba.NewProcess()
	if err != nil {
		log.Fatalf("Failed to create bba process: %v", err)
	}

	// 2. Lấy tất cả các command handlers và gộp chúng vào một map
	rootHandlers := make(map[string]func(t_network.Request) error)
	for command, handler := range rbcProcess.GetCommandHandlers() {
		rootHandlers[command] = handler
	}
	for command, handler := range bbaProcess.GetCommandHandlers() {
		rootHandlers[command] = handler
	}

	// 3. Tạo một root handler duy nhất từ map đã gộp
	rootHandler := network.NewHandler(rootHandlers, nil)

	// 4. Khởi tạo Node với root handler
	appNode, err := node.NewNode(config, rootHandler)
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}

	// 5. Gán node cho tất cả các process
	rbcProcess.SetNode(appNode)
	bbaProcess.SetNode(appNode)

	// 6. Bắt đầu Node (mạng) và các process
	go func() {
		if err := appNode.Start(); err != nil {
			log.Fatalf("Node failed to start: %v", err)
		}
	}()
	rbcProcess.Start()

	// Giao diện dòng lệnh để tương tác
	logger.Info("Enter command to start process:")
	logger.Info("  'rbc <message>' - to start an RBC broadcast")
	logger.Info("  'bba <session_id> <true|false>' - to start a BBA")
	scanner := bufio.NewScanner(os.Stdin)
	for {
		fmt.Print("> ")
		if scanner.Scan() {
			line := scanner.Text()
			parts := strings.Fields(line)
			if len(parts) == 0 {
				continue
			}
			switch parts[0] {
			case "rbc":
				if len(parts) > 1 {
					message := strings.Join(parts[1:], " ")
					rbcProcess.StartBroadcast([]byte(message), "string", mtn_proto.MessageType_SEND)
				}
			case "bba":
				if len(parts) == 3 {
					sessionID := parts[1]
					value := parts[2] == "true"
					bbaProcess.StartAgreement(sessionID, value)
				}
			default:
				logger.Warn("Unknown command. Use 'rbc' or 'bba'.")
			}
		} else {
			if err := scanner.Err(); err != nil {
				logger.Error("Error reading from stdin: %v", err)
			}
			break
		}
	}
}
