package main

import (
	"bufio"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"io/ioutil"
	"log"
	"math/rand"
	"os"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	// Assuming this is the correct import path for your protobuf definitions
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
	"google.golang.org/protobuf/proto"
)

// PeerConfig represents the configuration for a peer node.
type PeerConfig struct {
	Id                int    `json:"id"`
	ConnectionAddress string `json:"connection_address"`
	PublicKey         string `json:"public_key"`
}

// ValidatorInfo contains public key of a validator.
type ValidatorInfo struct {
	PublicKey string `json:"public_key"`
}
type NodeConfig struct {
	ID                int             `json:"id"`
	KeyPair           string          `json:"key_pair"`
	Master            PeerConfig      `json:"master"`
	NodeType          string          `json:"node_type"`
	Version           string          `json:"version"`
	ConnectionAddress string          `json:"connection_address"`
	Peers             []PeerConfig    `json:"peers"`
	NumValidator      int             `json:"num_validator"`
	Validator         []ValidatorInfo `json:"validator"`
}

func LoadConfigFromFile(filename string) (*NodeConfig, error) {
	data, err := ioutil.ReadFile(filename)
	if err != nil {
		return nil, fmt.Errorf("could not read config file: %w", err)
	}
	var config NodeConfig
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

	// Xây dựng danh sách peers từ tệp cấu hình
	peers := make(map[int32]string)
	logger.Info(allConfig.Peers)
	for _, nodeConf := range allConfig.Peers {
		logger.Info(nodeConf)
		peers[int32(nodeConf.Id)] = nodeConf.ConnectionAddress
	}
	logger.Info(peers)

	// In ra danh sách peers để kiểm tra
	logger.Info("Node %d initialized with peers: %v", int32(*&allConfig.ID), peers)

	// Khởi tạo process với ID và danh sách peers đã được đọc từ config
	process, err := rbc.NewProcess(int32(*&allConfig.ID), peers, nil)
	if err != nil {
		log.Fatalf("Failed to create process: %v", err)
	}

	// Start the server and connect to peers
	go func() {
		if err := process.Start(); err != nil {
			log.Fatalf("Process failed to start: %v", err)
		}
	}()

	// Goroutine to print delivered messages (giữ nguyên)
	go func() {
		for {
			payload := <-process.Delivered
			batch := &mtn_proto.Batch{}
			err := proto.Unmarshal(payload, batch)
			if err == nil {
				logger.Info("\n[APPLICATION] Node %d Delivered Batch for Block %d from Proposer %x\n> ", process.ID, batch.BlockNumber, batch.ProposerId)
			} else {
				logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", process.ID, string(payload))
			}
		}
	}()

	// Wait for the network to initialize
	logger.Info("Waiting for network to initialize...")
	time.Sleep(5 * time.Second)

	// Goroutine to randomly broadcast a Batch message (giữ nguyên)
	go func() {
		rand.Seed(time.Now().UnixNano())
		ticker := time.NewTicker(10 * time.Second) // Tăng thời gian để dễ quan sát
		defer ticker.Stop()
		blockCounter := uint64(0)
		for range ticker.C {
			if rand.Intn(100) < 20 {
				blockCounter++
				tx1 := &mtn_proto.Transaction{FromAddress: []byte("sender_1"), ToAddress: []byte("recipient_1"), Data: []byte("tx_data_1")}
				tx2 := &mtn_proto.Transaction{FromAddress: []byte("sender_2"), ToAddress: []byte("recipient_2"), Data: []byte("tx_data_2")}
				proposerId := process.KeyPair.PublicKey().Bytes()
				headerData := fmt.Sprintf("%d:%x", blockCounter, proposerId)
				batchHash := sha256.Sum256([]byte(headerData))
				batch := &mtn_proto.Batch{
					Hash:         batchHash[:],
					Transactions: []*mtn_proto.Transaction{tx1, tx2},
					BlockNumber:  blockCounter,
					ProposerId:   proposerId,
				}
				payload, err := proto.Marshal(batch)
				if err != nil {
					logger.Error("Failed to marshal batch:", err)
					continue
				}
				logger.Info("\n[APPLICATION] Node %d broadcasting a batch for block %d...\n> ", process.ID, blockCounter)
				process.StartBroadcast(payload)
			}
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
				process.StartBroadcast([]byte(line))
			}
		} else {
			if err := scanner.Err(); err != nil {
				logger.Info("Error reading from stdin: %v", err)
			}
			break
		}
	}
}
