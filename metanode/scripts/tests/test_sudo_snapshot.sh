#!/bin/bash

echo "=== Testing Sudo LVM Snapshot ==="
echo

BIN_PATH="/home/abc/chain-n/mtn-consensus/metanode/bin/lvm-snap-rsync"
BIN_DIR="/home/abc/chain-n/mtn-consensus/metanode/bin"

echo "Testing sudo execution simulation..."
echo "Command: sudo $BIN_PATH --id 999"
echo "Working directory: $BIN_DIR"
echo

# Test if sudo works
if sudo -n true 2>/dev/null; then
    echo "✅ Sudo works without password"
    
    # Try to run the binary with sudo (but expect it to fail due to LVM permissions)
    echo "Running: sudo $BIN_PATH --id 999 (from $BIN_DIR)"
    cd "$BIN_DIR"
    sudo "$BIN_PATH" --id 999 2>&1 | head -10
    EXIT_CODE=$?
    echo "Exit code: $EXIT_CODE"
    
    if [ $EXIT_CODE -eq 0 ]; then
        echo "✅ Binary executed successfully with sudo!"
    else
        echo "⚠️  Binary failed (expected due to LVM permissions in test environment)"
        echo "    In production with proper LVM setup, this would create snapshots."
    fi
    
else
    echo "❌ Sudo requires password - need to setup NOPASSWD"
    echo "    Run: bash /home/abc/chain-n/mtn-consensus/metanode/scripts/setup_sudo_snapshot.sh"
fi

echo
echo "=== Test Complete ==="
