package main

import (
	"bufio"
	"flag"
	"fmt"
	"log"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/meta-node-blockchain/meta-node/pkg/rbc"
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
			logger.Info("\n[APPLICATION] Node %d Delivered: %s\n> ", process.ID, string(payload))
		}
	}()

	// Wait for the network to initialize
	logger.Info("Waiting for network to initialize...")
	time.Sleep(5 * time.Second)

	// Allow user to broadcast messages from stdin
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
