#!/bin/bash
while true; do
  BLOCK=$(curl -s -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' http://localhost:10749 | grep -o '"result":"0x[0-9a-fA-F]*"' | awk -F'"' '{print $4}')
  if [ -n "$BLOCK" ]; then
    DEC_BLOCK=$((16#${BLOCK#0x}))
    echo "Current block: $DEC_BLOCK"
    if [ "$DEC_BLOCK" -ge 150 ]; then
      echo "Reached block 150! Restoring node 2..."
      cd /home/abc/chain-n/mtn-consensus/metanode
      ./scripts/node/restore_node.sh 2
      break
    fi
  fi
  sleep 5
done
