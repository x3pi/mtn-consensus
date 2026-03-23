#!/bin/bash
cd /home/abc/chain-n/mtn-consensus/metanode/scripts

echo "Starting fresh restart..."
./fresh_restart.sh > /dev/null 2>&1

echo "Waiting for epoch 1 (110 blocks -> ~220 seconds)..."
sleep 230

echo "Stopping Node 2..."
./node/stop_node.sh 2

echo "Creating snapshot (Epoch 1)..."
./create_snapshot.sh > /dev/null 2>&1

echo "Waiting for epoch 2 (210 blocks -> ~220 seconds)..."
sleep 230

echo "Restoring Node 2 from Epoch 1 snapshot while network is in Epoch 2..."
./node/restore_node.sh 2

echo "Waiting for node to catch up (30 seconds)..."
sleep 30
echo "Done! Check logs for InvalidGenesisAncestor"
