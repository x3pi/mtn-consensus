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
	"github.com/meta-node-blockchain/meta-node/pkg/config"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/node"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
)

// Application đóng gói toàn bộ logic của ứng dụng.
type Application struct {
	node       *node.Node
	rbcProcess *rbc.Process
	bbaProcess *bba.Process
}

// NewApplication khởi tạo toàn bộ ứng dụng.
func NewApplication(config *config.NodeConfig) (*Application, error) {
	app := &Application{}

	appNode, err := node.NewNode(config)
	if err != nil {
		return nil, fmt.Errorf("failed to create node: %w", err)
	}
	app.node = appNode

	rbcProcess, err := rbc.NewProcess(appNode)
	if err != nil {
		return nil, fmt.Errorf("failed to create rbc process: %w", err)
	}
	app.rbcProcess = rbcProcess

	bbaProcess, err := bba.NewProcess(appNode)
	if err != nil {
		return nil, fmt.Errorf("failed to create bba process: %w", err)
	}
	app.bbaProcess = bbaProcess

	app.node.RegisterModule(rbcProcess)
	app.node.RegisterModule(bbaProcess)

	return app, nil
}

// Start khởi chạy ứng dụng.
func (app *Application) Start() error {
	return app.node.Start()
}

// LoadConfigFromFile đọc và phân tích cú pháp tệp cấu hình.
func LoadConfigFromFile(filename string) (*config.NodeConfig, error) {
	data, err := os.ReadFile(filename)
	if err != nil {
		return nil, fmt.Errorf("could not read config file: %w", err)
	}
	var config config.NodeConfig
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

	app, err := NewApplication(config)
	if err != nil {
		log.Fatalf("Failed to initialize application: %v", err)
	}

	go func() {
		if err := app.Start(); err != nil {
			log.Fatalf("Application failed to start: %v", err)
		}
	}()

	runCLI(app)
}

func runCLI(app *Application) {
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
					app.rbcProcess.StartBroadcast([]byte(message), "string", mtn_proto.MessageType_SEND)
				}
			case "bba":
				if len(parts) == 3 {
					sessionID := parts[1]
					value := parts[2] == "true"
					app.bbaProcess.StartAgreement(sessionID, value)
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
