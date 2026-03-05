#!/bin/bash

echo "=== Testing LVM Config Reading ==="
echo

# Test 1: Direct binary execution
echo "Test 1: Running binary directly from bin directory"
cd metanode/bin
echo "Current directory: $(pwd)"
echo "Files in directory:"
ls -la
echo
echo "Running: ./lvm-snap-rsync --id 123"
echo "Output:"
./lvm-snap-rsync --id 123 2>&1 | head -5
echo
cd ../..

# Test 2: Simulate what node.rs does
echo "Test 2: Simulating node.rs logic"
BIN_PATH="/home/abc/chain-n/mtn-consensus/metanode/bin/lvm-snap-rsync"
BIN_DIR="/home/abc/chain-n/mtn-consensus/metanode/bin"

echo "Binary path: $BIN_PATH"
echo "Binary directory: $BIN_DIR"
echo "Running: cd $BIN_DIR && ./lvm-snap-rsync --id 456"
echo "Output:"
(cd "$BIN_DIR" && ./lvm-snap-rsync --id 456) 2>&1 | head -5
echo

# Test 3: Check config content
echo "Test 3: Config content verification"
echo "Config file: $BIN_DIR/config.toml"
echo "Content:"
cat "$BIN_DIR/config.toml"
echo

echo "=== Test Complete ==="
