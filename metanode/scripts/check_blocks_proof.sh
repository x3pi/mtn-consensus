#!/bin/bash
echo "Node 0 Block:"
curl -s -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' http://127.0.0.1:8757 | grep -o 'result":"[^"]*"'
echo "Node 1 Block:"
curl -s -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' http://127.0.0.1:10747 | grep -o 'result":"[^"]*"'
