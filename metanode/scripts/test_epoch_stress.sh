#!/bin/bash
set -e

echo "╔═══════════════════════════════════════════════════════╗"
echo "║  🔄 EPOCH TRANSITION STRESS TEST                     ║"
echo "║  📦 epoch_length=50, 200K TXs → cross 2-4 epochs    ║"
echo "╚═══════════════════════════════════════════════════════╝"

# Step 1: Start cluster with short epochs
EPOCH_LENGTH=50
echo "🚀 Step 1: Restarting cluster with epoch length = $EPOCH_LENGTH..."
./mtn-orchestrator.sh restart --fresh --build-all --epoch-length $EPOCH_LENGTH
sleep 10

# Step 2: Run load test
echo "🔥 Step 2: Blasting 200,000 transactions..."
cd ../../mtn-simple-2025/cmd/tool/tps_blast
./run_multinode_load.sh 10 20000  # 10 accounts * 20000 = 200K TXs
sleep 10

# Step 3: Check fragment offset persistence
echo "🔍 Step 3: Verifying persistence files..."
for i in 0 1 2 3 4; do
  OFFSET_FILE="../../config/storage/node_${i}/fragment_offset.dat"
  if [ -f "$OFFSET_FILE" ]; then
    echo "✅ Node $i: fragment_offset exists ($(cat $OFFSET_FILE))"
  else
    echo "⚠️ Node $i: No fragment_offset file (OK if no fragmentation occurred)"
  fi
done

# Step 4: Kill + restart node 0, blast more
echo "💀 Step 4: Crashing Node 0 to test recovery..."
kill -9 $(pgrep -f "metanode.*node-0") || true
sleep 5
echo "⚡ Restarting Node 0..."
cd ../../../../mtn-consensus/metanode/scripts
./node/node.sh 0 start
sleep 10

echo "🔥 Blasting 100,000 more transactions after recovery..."
cd ../../mtn-simple-2025/cmd/tool/tps_blast
./run_multinode_load.sh 10 10000
sleep 10

# Step 5: Fork check
echo "🔍 Step 5: Running fork check..."
cd ../block_hash_checker
go run . --config multinode_config.json
if [ $? -eq 0 ]; then
    echo "✅ EPOCH STRESS TEST COMPLETE: 0 FORKS DETECTED"
else
    echo "❌ EPOCH STRESS TEST FAILED: FORK DETECTED"
    exit 1
fi
