package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/agreement"
	"github.com/meta-node-blockchain/meta-node/pkg/bba"
	"github.com/meta-node-blockchain/meta-node/pkg/config"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/node"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
)

// Application encapsulates the entire application logic.
type Application struct {
	node             *node.Node
	rbcProcess       *rbc.Process
	bbaProcess       *bba.Process
	agreementProcess *agreement.Process // Thêm agreement process
}

func NewApplication(config *config.NodeConfig) (*Application, error) {
	app := &Application{}

	appNode, err := node.NewNode(config)
	if err != nil {
		return nil, fmt.Errorf("failed to create node: %w", err)
	}
	app.node = appNode

	// Khởi tạo RBC và BBA như trước
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

	// Khởi tạo Agreement Process, truyền các dependency cần thiết
	agreementProcess := agreement.NewProcess(appNode, rbcProcess, bbaProcess)
	app.agreementProcess = agreementProcess
	// Đăng ký tất cả các module với node
	time.Sleep(300 * time.Second)
	app.node.RegisterModule(rbcProcess)
	app.node.RegisterModule(bbaProcess)
	app.node.RegisterModule(agreementProcess)

	return app, nil
}

// Start launches the application.
func (app *Application) Start() error {
	return app.node.Start()
}

// LoadConfigFromFile reads and parses the configuration file.
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
	logger.Info("  'rbc <message>'              - to start an RBC broadcast")
	logger.Info("  'bba <session_id> <t/f>'     - to start a BBA (async)")
	logger.Info("  'bba-wait <sid> <t/f> <sec>' - to start a BBA and wait for result") // New command
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
					value := parts[2] == "true" || parts[2] == "t"
					// Call asynchronous function
					app.bbaProcess.StartAgreement(sessionID, value)
				}
			case "bba-wait": // Handle new command
				if len(parts) == 4 {
					sessionID := parts[1]
					value := parts[2] == "true" || parts[2] == "t"
					timeoutSec, err := strconv.Atoi(parts[3])
					if err != nil {
						logger.Error("Invalid timeout value: %v", err)
						continue
					}

					logger.Info("Starting BBA for session '%s' and waiting for decision (timeout %d s)...", sessionID, timeoutSec)

					// Call synchronous function and wait for result
					decision, err := app.bbaProcess.StartAgreementAndWait(sessionID, value, time.Duration(timeoutSec)*time.Second)
					if err != nil {
						logger.Error("BBA-WAIT FAILED: %v", err)
					} else {
						logger.Info("BBA-WAIT SUCCESS: Final decision for session '%s' is %v", sessionID, decision)
					}
				} else {
					logger.Warn("Usage: bba-wait <session_id> <true|false> <timeout_seconds>")
				}
			default:
				logger.Warn("Unknown command.")
			}
		} else {
			if err := scanner.Err(); err != nil {
				logger.Error("Error reading from stdin: %v", err)
			}
			break
		}
	}
}
