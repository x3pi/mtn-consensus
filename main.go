package main

import (
	"bufio"
	"crypto/sha256"
	"flag"
	"fmt"
	"log"
	"math/rand"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	// Assuming this is the correct import path for your protobuf definitions
	"github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
	"google.golang.org/protobuf/proto"
)

func main() {
	id := flag.Int("id", 0, "ID of this node")
	peersStr := flag.String("peers", "0:localhost:8000", "List of peers as id:addr,id:addr")
	flag.Parse()

	peers := make(map[int32]string)
	for _, pStr := range strings.Split(*peersStr, ",") {
		parts := strings.SplitN(pStr, ":", 2)
		if len(parts) != 2 {
			log.Fatalf("Invalid peer format: %s. Must be 'id:host:port'", pStr)
		}
		pID, err := strconv.Atoi(parts[0])
		if err != nil {
			log.Fatalf("Invalid peer ID: %v", err)
		}
		peers[int32(pID)] = parts[1]
	}

	// In a real integration with the meta-node-blockchain, a bls.KeyPair would be
	// created or loaded here and passed to NewProcess. For this example,
	// we pass nil and the Process will handle it.
	process, err := rbc.NewProcess(int32(*id), peers, nil)
	if err != nil {
		log.Fatalf("Failed to create process: %v", err)
	}

	// Start the server and connect to peers
	go func() {
		if err := process.Start(); err != nil {
			log.Fatalf("Process failed to start: %v", err)
		}
	}()

	// Goroutine to print delivered messages
	go func() {
		for {
			payload := <-process.Delivered

			// Try to unmarshal as a Batch message to pretty-print
			batch := &mtn_proto.Batch{}
			err := proto.Unmarshal(payload, batch)
			if err == nil {
				logger.Info("\n[APPLICATION] Node %d Delivered Batch for Block %d from Proposer %x\n> ", process.ID, batch.BlockNumber, batch.ProposerId)
			} else {
				// If it's not a batch, print as a plain string
				logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", process.ID, batch)
			}
		}
	}()

	// Wait for the network to initialize
	logger.Info("Waiting for network to initialize...")
	time.Sleep(5 * time.Second)

	// Goroutine to randomly broadcast a Batch message every second
	go func() {
		// Seed the random number generator
		rand.Seed(time.Now().UnixNano())

		// Ticker for every second
		ticker := time.NewTicker(100 * time.Millisecond)
		defer ticker.Stop()

		blockCounter := uint64(0)

		for range ticker.C {
			// 20% chance to send a message
			if rand.Intn(100) < 20 {
				blockCounter++

				// 1. Create dummy transactions
				tx1 := &mtn_proto.Transaction{
					FromAddress: []byte("sender_address_1"),
					ToAddress:   []byte("recipient_address_1"),
					Data:        []byte("dummy transaction data 1"),
				}
				tx2 := &mtn_proto.Transaction{
					FromAddress: []byte("sender_address_2"),
					ToAddress:   []byte("recipient_address_2"),
					Data:        []byte("dummy transaction data 2"),
				}

				proposerId := process.KeyPair.PublicKey().Bytes()

				// Create a temporary header to hash.
				// In a real scenario, this would be a Merkle root of the transactions.
				headerData := fmt.Sprintf("%d:%x", blockCounter, proposerId)
				batchHash := sha256.Sum256([]byte(headerData))

				// 2. Create the Batch message
				batch := &mtn_proto.Batch{
					Hash:         batchHash[:],
					Transactions: []*mtn_proto.Transaction{tx1, tx2},
					BlockNumber:  blockCounter,
					ProposerId:   proposerId,
				}

				// 3. Marshal the batch into bytes
				payload, err := proto.Marshal(batch)
				if err != nil {
					logger.Error("Failed to marshal batch:", err)
					continue
				}

				// 4. Broadcast the payload
				logger.Info("\n[APPLICATION] Node %d broadcasting a batch for block %d...\n> ", process.ID, blockCounter)
				process.StartBroadcast(payload)
			}
		}
	}()

	// Allow user to broadcast messages from stdin
	logger.Info("Enter a message and press Enter to broadcast (or leave empty for random batch broadcast):")
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
