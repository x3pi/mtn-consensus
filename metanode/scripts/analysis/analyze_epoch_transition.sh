#!/bin/bash

# ============================================================================
# Script: analyze_epoch_transition.sh
# Mục đích: Phân tích chi tiết tại sao epoch transition không hoạt động
# ============================================================================

set -e

# Get script directory and change to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

print_info() {
    echo -e "${GREEN}ℹ️  $1${NC}"
}

print_warn() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

print_header() {
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

echo ""
print_header "🔍 Phân tích Epoch Transition"
echo ""

# 1. Check epoch status
print_header "1. Epoch Status"
for i in {0..3}; do
    COMMITTEE_FILE="config/committee_node_${i}.json"
    if [ -f "$COMMITTEE_FILE" ]; then
        EPOCH=$(jq -r '.epoch // "N/A"' "$COMMITTEE_FILE" 2>/dev/null || echo "N/A")
        EPOCH_TS=$(jq -r '.epoch_timestamp_ms // "N/A"' "$COMMITTEE_FILE" 2>/dev/null || echo "N/A")
        echo "  Node $i: epoch=$EPOCH, timestamp_ms=$EPOCH_TS"
    fi
done

echo ""
print_header "2. Proposal Status"
for i in {0..3}; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Node $i:"
        # Check for proposals
        PROPOSAL_COUNT=$(grep -i "proposal.*created\|proposal.*received" "$LOG_FILE" | grep "epoch.*->" | wc -l)
        echo "    - Proposals: $PROPOSAL_COUNT"
        
        # Check for votes
        VOTE_COUNT=$(grep -i "auto-voted\|voted on proposal" "$LOG_FILE" | wc -l)
        echo "    - Votes: $VOTE_COUNT"
        
        # Check for quorum
        QUORUM_COUNT=$(grep -i "quorum.*reached" "$LOG_FILE" | wc -l)
        echo "    - Quorum reached: $QUORUM_COUNT"
        
        # Check for transition
        TRANSITION_COUNT=$(grep -i "transition.*triggered\|transition.*complete" "$LOG_FILE" | wc -l)
        echo "    - Transitions: $TRANSITION_COUNT"
    fi
done

echo ""
print_header "3. Recent Proposal Activity (Last 10 minutes)"
for i in {0..3}; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Node $i:"
        grep -i "proposal.*created\|proposal.*received\|auto-voted\|quorum.*reached\|transition.*triggered" "$LOG_FILE" | tail -5 | while read line; do
            echo "    $line"
        done
        echo ""
    fi
done

echo ""
print_header "4. Commit Index vs Transition Barrier"
for i in {0..3}; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Node $i:"
        # Get latest commit index
        LATEST_COMMIT=$(grep "Executing commit #" "$LOG_FILE" | tail -1 | grep -oP 'commit #\K[0-9]+' || echo "N/A")
        echo "    - Latest commit: $LATEST_COMMIT"
        
        # Get proposal commit index
        PROPOSAL_COMMIT=$(grep "proposal.*created\|proposal.*received" "$LOG_FILE" | tail -1 | grep -oP 'commit_index=\K[0-9]+' || echo "N/A")
        if [ "$PROPOSAL_COMMIT" != "N/A" ]; then
            TRANSITION_BARRIER=$((PROPOSAL_COMMIT + 10))
            echo "    - Proposal commit: $PROPOSAL_COMMIT"
            echo "    - Transition barrier: $TRANSITION_BARRIER"
            if [ "$LATEST_COMMIT" != "N/A" ] && [ "$LATEST_COMMIT" -ge "$TRANSITION_BARRIER" ]; then
                echo "    - ✅ Barrier passed (commit $LATEST_COMMIT >= $TRANSITION_BARRIER)"
            elif [ "$LATEST_COMMIT" != "N/A" ]; then
                echo "    - ❌ Barrier NOT passed (commit $LATEST_COMMIT < $TRANSITION_BARRIER)"
            fi
        fi
    fi
done

echo ""
print_header "5. Node Status"
for i in {0..3}; do
    if tmux has-session -t "metanode-$i" 2>/dev/null; then
        echo "  Node $i: ✅ Running (tmux session exists)"
    else
        echo "  Node $i: ❌ Not running (no tmux session)"
    fi
done

echo ""
print_header "6. Recommendations"
echo ""
print_warn "Nếu epoch không transition:"
echo "  1. Kiểm tra tất cả nodes có cùng epoch không"
echo "  2. Kiểm tra quorum đã đạt chưa (cần 3/4 votes)"
echo "  3. Kiểm tra commit index đã vượt barrier chưa (proposal_commit_index + 10)"
echo "  4. Kiểm tra tất cả nodes online và vote"
echo ""

