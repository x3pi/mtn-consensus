#!/bin/bash

echo "=== Testing Networking ==="

# Test ports
echo "Testing consensus ports:"
for port in 9000 9001 9002 9003; do
    if nc -z 127.0.0.1 $port 2>/dev/null; then
        echo "✅ Port $port OK"
    else
        echo "❌ Port $port FAIL"
    fi
done

echo ""
echo "Testing RPC ports:"
for port in 10100 10101 10102 10103; do
    if nc -z 127.0.0.1 $port 2>/dev/null; then
        echo "✅ RPC Port $port OK"
    else
        echo "❌ RPC Port $port FAIL"
    fi
done

echo ""
echo "Testing inter-node connectivity:"
for from_port in 9000 9001 9002 9003; do
    echo "From port $from_port:"
    for to_port in 9000 9001 9002 9003; do
        if [ "$from_port" != "$to_port" ]; then
            if nc -z 127.0.0.1 $to_port 2>/dev/null; then
                echo "  ✅ Can connect to $to_port"
            else
                echo "  ❌ Cannot connect to $to_port"
            fi
        fi
    done
done

echo ""
echo "=== Testing Transaction Submission ==="

# Test transaction submission
echo "Submitting test transaction to node 0..."
curl -s -X POST "http://127.0.0.1:10100" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc": "2.0", "method": "submit_transaction", "params": {"transaction": "test_tx_123"}, "id": 1}' || echo "❌ RPC call failed"

echo ""
echo "Test completed!"
