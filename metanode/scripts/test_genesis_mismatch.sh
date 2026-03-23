#!/bin/bash
cd /home/abc/chain-n/mtn-consensus/metanode/scripts

echo "Starting fresh restart..."
./fresh_restart.sh > /dev/null 2>&1

echo "Waiting for epoch 1 (110 blocks -> ~220 seconds)..."
sleep 230

echo "Creating snapshot..."
./create_snapshot.sh > /dev/null 2>&1

echo "Waiting for more blocks..."
sleep 40

echo "Stopping Node 2..."
./node/stop_node.sh 2

echo "Restoring Node 2..."
./node/restore_node.sh 2

echo "Done! Check logs for InvalidGenesisAncestor"
