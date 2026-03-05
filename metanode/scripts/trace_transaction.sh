#!/bin/bash

# Script để trace một transaction cụ thể qua toàn bộ hệ thống
# Usage: ./scripts/trace_transaction.sh <transaction_hash>

if [ $# -eq 0 ]; then
    echo "Usage: $0 <transaction_hash>"
    echo "Example: $0 559cde2ef6a6ed4dc558e2cf7e7515c1a6907df137584203ddc556cc47433165"
    exit 1
fi

TX_HASH="$1"
TX_HASH_SHORT="${TX_HASH:0:16}"  # First 8 bytes (16 hex chars)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"

echo "🔍 Tracing transaction: $TX_HASH"
echo "   Short hash (first 8 bytes): $TX_HASH_SHORT"
echo ""

# 1. Check Go Sub Node logs
echo "📤 STEP 1: Go Sub Node - Transaction Submission"
echo "   Checking: $GO_PROJECT_ROOT/cmd/simple_chain/sample/simple/data-write/logs/2025/12/22/App.log"
if grep -i "$TX_HASH\|$TX_HASH_SHORT" "$GO_PROJECT_ROOT/cmd/simple_chain/sample/simple/data-write/logs/2025/12/22/App.log" 2>/dev/null | grep -i "TX FLOW\|Sending.*transaction" | head -5; then
    echo "   ✅ Found in Go Sub Node logs"
else
    echo "   ❌ NOT FOUND in Go Sub Node logs"
fi
echo ""

# 2. Check Rust Node 0 logs (RPC/UDS server)
echo "📥 STEP 2: Rust Node 0 - Transaction Reception"
echo "   Checking: $METANODE_ROOT/logs/latest/node_0.log"
if grep -i "$TX_HASH_SHORT\|656 bytes" "$METANODE_ROOT/logs/latest/node_0.log" 2>/dev/null | grep -i "TX FLOW\|Transaction.*submitted\|included in block" | head -5; then
    echo "   ✅ Found in Rust Node 0 logs"
else
    echo "   ❌ NOT FOUND in Rust Node 0 logs"
    echo "   Checking for Transactions message..."
    if grep -i "Transactions message\|Split.*transaction" "$METANODE_ROOT/logs/latest/node_0.log" 2>/dev/null | tail -10; then
        echo "   ⚠️  Found Transactions message logs, but not this specific transaction"
    fi
fi
echo ""

# 3. Check Rust Node 0 logs (Commit processor)
echo "🔷 STEP 3: Rust Node 0 - Transaction Commit"
echo "   Checking: $METANODE_ROOT/logs/latest/node_0.log"
if grep -i "$TX_HASH_SHORT" "$METANODE_ROOT/logs/latest/node_0.log" 2>/dev/null | grep -i "Executing commit\|Sent committed" | head -5; then
    echo "   ✅ Found in commit logs"
else
    echo "   ❌ NOT FOUND in commit logs"
fi
echo ""

# 4. Check Go Master Node logs
echo "⚙️  STEP 4: Go Master Node - Transaction Execution"
echo "   Checking: $GO_PROJECT_ROOT/cmd/simple_chain/sample/simple/data/logs/2025/12/22/App.log"
if grep -i "$TX_HASH\|$TX_HASH_SHORT" "$GO_PROJECT_ROOT/cmd/simple_chain/sample/simple/data/logs/2025/12/22/App.log" 2>/dev/null | grep -i "TX FLOW\|Processing\|Creating block" | head -5; then
    echo "   ✅ Found in Go Master Node logs"
else
    echo "   ❌ NOT FOUND in Go Master Node logs"
fi
echo ""

# Summary
echo "📊 SUMMARY:"
echo "   Transaction hash: $TX_HASH"
echo "   Short hash: $TX_HASH_SHORT"
echo ""
echo "   To see full logs, run:"
echo "   - Go Sub: grep -i '$TX_HASH' $GO_PROJECT_ROOT/cmd/simple_chain/sample/simple/data-write/logs/2025/12/22/App.log"
echo "   - Rust: grep -i '$TX_HASH_SHORT' $METANODE_ROOT/logs/latest/node_0.log"
echo "   - Go Master: grep -i '$TX_HASH' $GO_PROJECT_ROOT/cmd/simple_chain/sample/simple/data/logs/2025/12/22/App.log"

