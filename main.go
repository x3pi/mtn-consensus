package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os" // <-- 1. Import 'os' instead of 'io/ioutil'

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/node"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
)

// LoadConfigFromFile now uses os.ReadFile.
func LoadConfigFromFile(filename string) (*node.NodeConfig, error) {
	// 2. Use os.ReadFile which is the current standard
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

	// 1. Khởi tạo RBC Process (chưa có node)
	rbcProcess, err := rbc.NewProcess()
	if err != nil {
		log.Fatalf("Failed to create rbc process: %v", err)
	}

	// 2. Khởi tạo Node, truyền handler từ RBC vào
	appNode, err := node.NewNode(config, rbcProcess.GetHandler())
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}

	// 3. Gán node đã khởi tạo vào rbcProcess bằng phương thức SetNode
	rbcProcess.SetNode(appNode)

	// 4. Bắt đầu Node (mạng)
	go func() {
		if err := appNode.Start(); err != nil {
			log.Fatalf("Node failed to start: %v", err)
		}
	}()

	// 5. Bắt đầu logic của RBC
	rbcProcess.Start()

	logger.Info("Enter a message and press Enter to broadcast:")
	scanner := bufio.NewScanner(os.Stdin)
	for {
		fmt.Print("> ")
		if scanner.Scan() {
			line := scanner.Text()
			if line != "" {
				rbcProcess.StartBroadcast([]byte(line), "string", mtn_proto.MessageType_INIT)
			}
		} else {
			if err := scanner.Err(); err != nil {
				logger.Error("Error reading from stdin: %v", err)
			}
			break
		}
	}
}
