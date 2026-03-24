#!/bin/bash
while true; do
  RES=$(curl -s "http://localhost:10749" -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}')
  B=$(echo "$RES" | grep -o '0x[0-9a-fA-F]*')
  if [ -n "$B" ] && [ "$B" != "null" ]; then
    D=$(printf "%d" "$B")
    echo "Block $D"
    if [ "$D" -ge 140 ]; then
      echo "Reached block 140. Restoring node 2..."
      cd /home/abc/chain-n/mtn-consensus/metanode
      ./scripts/node/restore_node.sh 2
      break
    fi
  fi
  sleep 5
done
