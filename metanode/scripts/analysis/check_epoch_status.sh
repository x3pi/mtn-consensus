#!/bin/bash

# ============================================================================
# Script: check_epoch_status.sh
# Mục đích: Kiểm tra trạng thái epoch của tất cả nodes và debug vấn đề epoch transition
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

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_header() {
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

# Check if node_id is provided (optional)
NODE_ID="${1:-}"

echo ""
print_header "📊 Epoch Status Check"
echo ""

# Check committee.json files
print_header "1. Committee.json Status"
for i in {0..3}; do
    COMMITTEE_FILE="config/committee_node_${i}.json"
    if [ -f "$COMMITTEE_FILE" ]; then
        EPOCH=$(jq -r '.epoch // "N/A"' "$COMMITTEE_FILE" 2>/dev/null || echo "N/A")
        EPOCH_TS=$(jq -r '.epoch_timestamp_ms // "N/A"' "$COMMITTEE_FILE" 2>/dev/null || echo "N/A")
        if [ "$EPOCH_TS" != "N/A" ] && [ "$EPOCH_TS" != "null" ]; then
            # Convert to human readable
            EPOCH_TS_READABLE=$(date -d "@$((EPOCH_TS / 1000))" "+%Y-%m-%d %H:%M:%S" 2>/dev/null || echo "N/A")
        else
            EPOCH_TS_READABLE="N/A"
        fi
        echo "  Node $i: epoch=$EPOCH, timestamp_ms=$EPOCH_TS ($EPOCH_TS_READABLE)"
    else
        echo "  Node $i: ❌ Committee file not found"
    fi
done

echo ""
print_header "2. Epoch Transition Logs (Last 20 lines)"
if [ -n "$NODE_ID" ]; then
    LOG_FILE="logs/latest/node_${NODE_ID}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Checking node $NODE_ID logs..."
        echo ""
        grep -i "epoch.*proposal\|epoch.*transition\|epoch.*change\|quorum.*reached" "$LOG_FILE" | tail -20 || echo "  No epoch-related logs found"
    else
        echo "  ❌ Log file not found: $LOG_FILE"
    fi
else
    for i in {0..3}; do
        LOG_FILE="logs/latest/node_${i}.log"
        if [ -f "$LOG_FILE" ]; then
            echo "  Node $i:"
            grep -i "epoch.*proposal\|epoch.*transition\|epoch.*change\|quorum.*reached" "$LOG_FILE" | tail -5 || echo "    No epoch-related logs found"
            echo ""
        fi
    done
fi

echo ""
print_header "3. Recent Epoch Proposals"
if [ -n "$NODE_ID" ]; then
    LOG_FILE="logs/latest/node_${NODE_ID}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Checking node $NODE_ID for proposals..."
        echo ""
        grep -i "proposal.*created\|proposal.*received\|auto-voted" "$LOG_FILE" | tail -10 || echo "  No proposal logs found"
    fi
else
    for i in {0..3}; do
        LOG_FILE="logs/latest/node_${i}.log"
        if [ -f "$LOG_FILE" ]; then
            PROPOSAL_COUNT=$(grep -i "proposal.*created\|proposal.*received" "$LOG_FILE" | wc -l)
            echo "  Node $i: $PROPOSAL_COUNT proposal-related logs"
        fi
    done
fi

echo ""
print_header "4. Quorum Status"
if [ -n "$NODE_ID" ]; then
    LOG_FILE="logs/latest/node_${NODE_ID}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Checking node $NODE_ID for quorum..."
        echo ""
        grep -i "quorum.*reached\|quorum.*progress" "$LOG_FILE" | tail -10 || echo "  No quorum logs found"
    fi
else
    for i in {0..3}; do
        LOG_FILE="logs/latest/node_${i}.log"
        if [ -f "$LOG_FILE" ]; then
            QUORUM_COUNT=$(grep -i "quorum.*reached" "$LOG_FILE" | wc -l)
            echo "  Node $i: $QUORUM_COUNT quorum reached logs"
        fi
    done
fi

echo ""
print_header "5. Time-based Epoch Change Check"
if [ -n "$NODE_ID" ]; then
    LOG_FILE="logs/latest/node_${NODE_ID}.log"
    if [ -f "$LOG_FILE" ]; then
        echo "  Checking node $NODE_ID for time-based triggers..."
        echo ""
        grep -i "time-based.*epoch\|epoch.*duration\|epoch.*trigger" "$LOG_FILE" | tail -10 || echo "  No time-based logs found"
    fi
else
    for i in {0..3}; do
        LOG_FILE="logs/latest/node_${i}.log"
        if [ -f "$LOG_FILE" ]; then
            TIME_BASED_COUNT=$(grep -i "time-based.*epoch\|epoch.*duration" "$LOG_FILE" | wc -l)
            echo "  Node $i: $TIME_BASED_COUNT time-based epoch logs"
        fi
    done
fi

echo ""
print_header "6. Recommendations"
echo ""
echo "  Nếu epoch không chuyển đổi, kiểm tra:"
echo "  1. ✅ Tất cả nodes có cùng epoch trong committee.json?"
echo "  2. ✅ Có proposal nào được tạo không? (check logs)"
echo "  3. ✅ Có đủ quorum (2f+1) không? (cần ít nhất 3/4 nodes online)"
echo "  4. ✅ Time-based epoch change có enabled không?"
echo "  5. ✅ Epoch duration đã đủ chưa?"
echo ""
echo "  Để xem chi tiết logs của một node:"
echo "    ./check_epoch_status.sh <node_id>"
echo "    tail -f logs/latest/node_X.log | grep -i epoch"
echo ""

