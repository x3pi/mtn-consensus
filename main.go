package main

import (
	"bufio"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"io/ioutil"
	"log"
	"os"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"

	// Assuming this is the correct import path for your protobuf definitions
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
	"google.golang.org/protobuf/proto"
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
	process, err := rbc.NewProcess(allConfig)
	if err != nil {
		log.Fatalf("Failed to create process: %v", err)
	}

	// Create a map to hold payloads that arrive out of order.
	// We only store ONE payload per block number (first-come, first-served) to prevent duplicates.
	pendingPayloads := make(map[uint64]*rbc.ProposalNotification)
	var mu sync.Mutex // Mutex to protect access to pendingPayloads

	// Function to process a batch
	processBatch := func(payload *rbc.ProposalNotification) {
		logger.Info("\n[APPLICATION] Node %d Delivered Batch for Block %d from Proposer %x\n> ", process.ID, payload.Priority, payload.SenderID)
		err := process.MessageSender.SendBytes(process.MasterConn, common.PushFinalizeEvent, []byte{})
		if err != nil {
			logger.Error("Failed to send PushFinalizeEvent: %v", err)
		}
		// This assumes that after sending PushFinalizeEvent, GetCurrentBlockNumber() will be incremented.
	}

	// Start the server and connect to peers
	go func() {
		if err := process.Start(); err != nil {
			log.Fatalf("Process failed to start: %v", err)
		}
	}()

	// Goroutine to handle delivered messages with ordering logic
	go func() {
		for {
			payload := <-process.Delivered
			batch := &mtn_proto.Batch{}
			err := proto.Unmarshal(payload.Payload, batch)

			if err != nil {
				// Not a batch, process as a simple message
				logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", process.ID, string(payload.Payload))
				continue
			}

			// It's a batch, handle it with ordering logic
			mu.Lock()
			currentExpectedBlock := process.GetCurrentBlockNumber() + 1
			if payload.Priority == int64(currentExpectedBlock) {
				processBatch(payload)

				// After processing a block, check for subsequent blocks in the pending map
				nextBlock := currentExpectedBlock + 1
				for {
					if pendingPayload, found := pendingPayloads[nextBlock]; found {
						processBatch(pendingPayload) // No loop needed, process the single payload
						delete(pendingPayloads, nextBlock)
						nextBlock++
					} else {
						break // No more sequential blocks in pending
					}
				}
			} else if payload.Priority > int64(currentExpectedBlock) {
				// Arrived early, store it ONLY IF we haven't stored one for this block yet.
				priorityKey := uint64(payload.Priority)
				if _, found := pendingPayloads[priorityKey]; !found {
					logger.Info("\n[APPLICATION] Node %d received future block %d, pending.\n> ", process.ID, payload.Priority)
					pendingPayloads[priorityKey] = payload
				}
			}
			// Ignore old or duplicate blocks (payload.Priority < currentExpectedBlock)
			mu.Unlock()
		}
	}()

	// Goroutine to periodically check for and process pending blocks
	go func() {
		ticker := time.NewTicker(200 * time.Millisecond) // Check every 200ms
		defer ticker.Stop()

		for range ticker.C {
			mu.Lock()
			// Check if we can process any pending blocks
			currentExpectedBlock := process.GetCurrentBlockNumber() + 1
			for {
				if pendingPayload, found := pendingPayloads[currentExpectedBlock]; found {
					logger.Info("[PENDING CHECKER] Processing pending block %d", currentExpectedBlock)
					processBatch(pendingPayload) // No loop needed, process the single payload
					delete(pendingPayloads, currentExpectedBlock)
					currentExpectedBlock++ // Move to check the next block
				} else {
					break // No more sequential blocks to process from pending
				}
			}
			mu.Unlock()
		}
	}()

	// Wait for the network to initialize
	logger.Info("Waiting for network to initialize...")
	// time.Sleep(10 * time.Second)

	go func() {
		for txs := range process.PoolTransactions {
			proposerId := process.KeyPair.PublicKey().Bytes()
			headerData := fmt.Sprintf("%d:%x", process.GetCurrentBlockNumber()+1, proposerId)
			batchHash := sha256.Sum256([]byte(headerData))
			batch := &mtn_proto.Batch{
				Hash:         batchHash[:],
				Transactions: txs,
				BlockNumber:  process.GetCurrentBlockNumber() + 1,
				ProposerId:   proposerId,
			}
			payload, err := proto.Marshal(batch)
			if err != nil {
				logger.Error("Failed to marshal batch:", err)
				continue
			}
			logger.Info("\n[APPLICATION] Node %d broadcasting a batch for block %d...\n> ", process.ID, process.GetCurrentBlockNumber()+1)
			process.StartBroadcast(payload)
		}
	}()
	time.Sleep(time.Second * 20)

	go func() {

		process.MessageSender.SendBytes(
			process.MasterConn,
			common.ValidatorGetBlockNumber,
			[]byte{},
		)
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
