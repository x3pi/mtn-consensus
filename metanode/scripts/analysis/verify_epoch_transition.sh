#!/bin/bash

# Script để verify epoch transition - đảm bảo tất cả nodes có cùng state (không fork)

set -e

# Get script directory and change to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔍 Epoch Transition Verification (Fork-Safety Check)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Extract values from logs
echo "1. Extracting transition values from logs:"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

declare -A NODE_EPOCH
declare -A NODE_LAST_COMMIT_INDEX
declare -A NODE_LAST_GLOBAL_EXEC_INDEX
declare -A NODE_EPOCH_TIMESTAMP

for i in 0 1 2 3; do
    LOG_FILE="logs/latest/node_${i}.log"
    COMMITTEE_FILE="config/committee_node_${i}.json"
    
    if [ ! -f "$LOG_FILE" ]; then
        echo "  ⚠️  Node $i: Log file not found"
        continue
    fi
    
    # Extract from committee.json
    if [ -f "$COMMITTEE_FILE" ]; then
        EPOCH=$(jq -r '.epoch // 0' "$COMMITTEE_FILE" 2>/dev/null || echo "0")
        LAST_GLOBAL_EXEC_INDEX=$(jq -r '.last_global_exec_index // 0' "$COMMITTEE_FILE" 2>/dev/null || echo "0")
        EPOCH_TIMESTAMP=$(jq -r '.epoch_timestamp_ms // 0' "$COMMITTEE_FILE" 2>/dev/null || echo "0")
        
        NODE_EPOCH[$i]=$EPOCH
        NODE_LAST_GLOBAL_EXEC_INDEX[$i]=$LAST_GLOBAL_EXEC_INDEX
        NODE_EPOCH_TIMESTAMP[$i]=$EPOCH_TIMESTAMP
    else
        NODE_EPOCH[$i]="N/A"
        NODE_LAST_GLOBAL_EXEC_INDEX[$i]="N/A"
        NODE_EPOCH_TIMESTAMP[$i]="N/A"
    fi
    
    # Extract last commit index from transition log
    LAST_COMMIT_INDEX=$(grep "Last commit index (barrier):" "$LOG_FILE" 2>/dev/null | tail -1 | sed -n 's/.*Last commit index (barrier): \([0-9]*\).*/\1/p' || echo "N/A")
    if [ "$LAST_COMMIT_INDEX" = "N/A" ]; then
        # Try alternative pattern
        LAST_COMMIT_INDEX=$(grep "last_commit_index=" "$LOG_FILE" 2>/dev/null | tail -1 | sed -n 's/.*last_commit_index=\([0-9]*\).*/\1/p' || echo "N/A")
    fi
    NODE_LAST_COMMIT_INDEX[$i]=$LAST_COMMIT_INDEX
done

echo ""
echo "2. Node Values Comparison:"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
printf "%-8s %-10s %-20s %-25s %-20s\n" "Node" "Epoch" "Last Commit Index" "Last Global Exec Index" "Epoch Timestamp"
echo "────────────────────────────────────────────────────────────────────────────────────────────────────────────"
for i in 0 1 2 3; do
    printf "%-8s %-10s %-20s %-25s %-20s\n" \
        "node-$i" \
        "${NODE_EPOCH[$i]}" \
        "${NODE_LAST_COMMIT_INDEX[$i]}" \
        "${NODE_LAST_GLOBAL_EXEC_INDEX[$i]}" \
        "${NODE_EPOCH_TIMESTAMP[$i]}"
done
echo ""

# Check for consistency
echo "3. Fork-Safety Verification:"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Find nodes with valid values
VALID_NODES=()
for i in 0 1 2 3; do
    if [ "${NODE_EPOCH[$i]}" != "N/A" ] && [ "${NODE_EPOCH[$i]}" != "0" ]; then
        VALID_NODES+=($i)
    fi
done

if [ ${#VALID_NODES[@]} -eq 0 ]; then
    echo "  ⚠️  No valid nodes found"
    exit 1
fi

# Get reference values from first valid node
REF_NODE=${VALID_NODES[0]}
REF_EPOCH=${NODE_EPOCH[$REF_NODE]}
REF_LAST_COMMIT_INDEX=${NODE_LAST_COMMIT_INDEX[$REF_NODE]}
REF_LAST_GLOBAL_EXEC_INDEX=${NODE_LAST_GLOBAL_EXEC_INDEX[$REF_NODE]}
REF_EPOCH_TIMESTAMP=${NODE_EPOCH_TIMESTAMP[$REF_NODE]}

echo "  📋 Reference values (from node $REF_NODE):"
echo "    - Epoch: $REF_EPOCH"
echo "    - Last commit index: $REF_LAST_COMMIT_INDEX"
echo "    - Last global exec index: $REF_LAST_GLOBAL_EXEC_INDEX"
echo "    - Epoch timestamp: $REF_EPOCH_TIMESTAMP"
echo ""

# Check each node
FORK_DETECTED=0
for i in "${VALID_NODES[@]}"; do
    MISMATCHES=()
    
    if [ "${NODE_EPOCH[$i]}" != "$REF_EPOCH" ]; then
        MISMATCHES+=("epoch (${NODE_EPOCH[$i]} vs $REF_EPOCH)")
    fi
    
    if [ "${NODE_LAST_COMMIT_INDEX[$i]}" != "N/A" ] && [ "$REF_LAST_COMMIT_INDEX" != "N/A" ]; then
        if [ "${NODE_LAST_COMMIT_INDEX[$i]}" != "$REF_LAST_COMMIT_INDEX" ]; then
            MISMATCHES+=("last_commit_index (${NODE_LAST_COMMIT_INDEX[$i]} vs $REF_LAST_COMMIT_INDEX)")
        fi
    fi
    
    if [ "${NODE_LAST_GLOBAL_EXEC_INDEX[$i]}" != "$REF_LAST_GLOBAL_EXEC_INDEX" ]; then
        MISMATCHES+=("last_global_exec_index (${NODE_LAST_GLOBAL_EXEC_INDEX[$i]} vs $REF_LAST_GLOBAL_EXEC_INDEX)")
    fi
    
    if [ "${NODE_EPOCH_TIMESTAMP[$i]}" != "$REF_EPOCH_TIMESTAMP" ]; then
        MISMATCHES+=("epoch_timestamp_ms (${NODE_EPOCH_TIMESTAMP[$i]} vs $REF_EPOCH_TIMESTAMP)")
    fi
    
    if [ ${#MISMATCHES[@]} -gt 0 ]; then
        echo "  ❌ Node $i: FORK DETECTED! Mismatches:"
        for mismatch in "${MISMATCHES[@]}"; do
            echo "      - $mismatch"
        done
        FORK_DETECTED=1
    else
        echo "  ✅ Node $i: All values match (no fork)"
    fi
done

echo ""
if [ $FORK_DETECTED -eq 1 ]; then
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "❌ FORK DETECTED! Nodes have different values."
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    exit 1
else
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ NO FORK: All nodes have consistent values."
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    exit 0
fi

