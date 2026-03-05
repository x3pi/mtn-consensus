#!/bin/bash

# Script để phân tích quá trình vote và propagation cho epoch transition
# Đặc biệt hữu ích để debug tại sao một node transition mà các node khác không

set -e

# Get script directory and change to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

PROPOSAL_HASH="${1:-eae7aee9d03632dd}"  # Default to epoch 5->6 proposal
EPOCH_FROM="${2:-5}"
EPOCH_TO="${3:-6}"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔍 Vote Propagation Analysis"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Proposal: epoch $EPOCH_FROM -> $EPOCH_TO"
echo "Proposal Hash: $PROPOSAL_HASH"
echo ""

# Analyze each node
for i in 0 1 2 3; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ ! -f "$LOG_FILE" ]; then
        echo "⚠️  Node $i: Log file not found"
        continue
    fi
    
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Node $i Analysis:"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    # Check if node received proposal
    PROPOSAL_RECEIVED=$(grep -c "Received epoch change proposal.*$PROPOSAL_HASH\|Epoch change proposal created.*$PROPOSAL_HASH" "$LOG_FILE" 2>/dev/null || echo "0")
    if [ "$PROPOSAL_RECEIVED" -gt 0 ]; then
        echo "  ✅ Proposal received: YES"
        echo "    First seen:"
        grep "Received epoch change proposal.*$PROPOSAL_HASH\|Epoch change proposal created.*$PROPOSAL_HASH" "$LOG_FILE" | head -1 | sed 's/^/      /'
    else
        echo "  ❌ Proposal received: NO"
    fi
    
    # Check if node voted
    VOTE_COUNT=$(grep -c "Voted on epoch change proposal.*$PROPOSAL_HASH\|Auto-voted on proposal.*epoch $EPOCH_FROM -> $EPOCH_TO" "$LOG_FILE" 2>/dev/null || echo "0")
    if [ "$VOTE_COUNT" -gt 0 ]; then
        echo "  ✅ Node voted: YES (count: $VOTE_COUNT)"
        echo "    Vote details:"
        grep "Voted on epoch change proposal.*$PROPOSAL_HASH\|Auto-voted on proposal.*epoch $EPOCH_FROM -> $EPOCH_TO" "$LOG_FILE" | head -1 | sed 's/^/      /'
    else
        echo "  ❌ Node voted: NO"
    fi
    
    # Check quorum progress
    QUORUM_LOGS=$(grep "Quorum progress.*$PROPOSAL_HASH\|quorum.*reached.*epoch $EPOCH_FROM -> $EPOCH_TO" "$LOG_FILE" 2>/dev/null | head -5)
    if [ -n "$QUORUM_LOGS" ]; then
        echo "  📊 Quorum progress:"
        echo "$QUORUM_LOGS" | sed 's/^/      /'
    else
        echo "  ⚠️  Quorum progress: NO LOGS"
    fi
    
    # Check transition status
    TRANSITION_READY=$(grep -c "Transition ready.*epoch $EPOCH_FROM -> $EPOCH_TO.*quorum=APPROVED\|EPOCH TRANSITION TRIGGERED" "$LOG_FILE" 2>/dev/null || echo "0")
    TRANSITION_NOT_READY=$(grep -c "Transition NOT ready.*epoch $EPOCH_FROM -> $EPOCH_TO" "$LOG_FILE" 2>/dev/null || echo "0")
    
    if [ "$TRANSITION_READY" -gt 0 ]; then
        echo "  ✅ Transition status: READY/TRIGGERED"
        echo "    Details:"
        grep "Transition ready.*epoch $EPOCH_FROM -> $EPOCH_TO\|EPOCH TRANSITION TRIGGERED" "$LOG_FILE" | head -1 | sed 's/^/      /'
    elif [ "$TRANSITION_NOT_READY" -gt 0 ]; then
        echo "  ⏸️  Transition status: NOT READY"
        echo "    Last reason:"
        grep "Transition NOT ready.*epoch $EPOCH_FROM -> $EPOCH_TO" "$LOG_FILE" | tail -1 | sed 's/^/      /'
    else
        echo "  ❓ Transition status: UNKNOWN"
    fi
    
    # Check for votes received from other nodes
    echo "  📥 Votes received from other nodes:"
    VOTES_RECEIVED=$(grep "Processed epoch change vote.*$PROPOSAL_HASH\|Processed epoch change vote.*epoch $EPOCH_FROM -> $EPOCH_TO" "$LOG_FILE" 2>/dev/null | head -5)
    if [ -n "$VOTES_RECEIVED" ]; then
        echo "$VOTES_RECEIVED" | sed 's/^/      /'
    else
        echo "      (No votes received logs found)"
    fi
    
    # Check current epoch
    COMMITTEE_FILE="config/committee_node_${i}.json"
    if [ -f "$COMMITTEE_FILE" ]; then
        CURRENT_EPOCH=$(jq -r '.epoch // 0' "$COMMITTEE_FILE" 2>/dev/null || echo "0")
        echo "  📋 Current epoch: $CURRENT_EPOCH"
    fi
    
    echo ""
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Summary:"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Expected behavior:"
echo "  1. All nodes should receive the proposal"
echo "  2. All nodes should auto-vote on valid proposal"
echo "  3. Votes should propagate via blocks"
echo "  4. All nodes should see quorum reached (3/4 votes)"
echo "  5. All nodes should transition at similar commit index"
echo ""
echo "If only one node transitions:"
echo "  - Check if votes propagated correctly"
echo "  - Check if proposal hash matches across nodes"
echo "  - Check if quorum check is consistent"
echo ""

