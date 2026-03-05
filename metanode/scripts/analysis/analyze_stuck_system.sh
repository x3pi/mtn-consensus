#!/bin/bash

# ============================================================================
# Script: analyze_stuck_system.sh
# Mục đích: Phân tích tại sao hệ thống bị đứng lại
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
print_header "🔍 Phân tích hệ thống bị đứng lại"
echo ""

# 1. Check epoch divergence
print_header "1. Epoch Divergence Check"
EPOCHS=()
TIMESTAMPS=()
for i in {0..3}; do
    COMMITTEE_FILE="config/committee_node_${i}.json"
    if [ -f "$COMMITTEE_FILE" ]; then
        EPOCH=$(jq -r '.epoch // "N/A"' "$COMMITTEE_FILE" 2>/dev/null || echo "N/A")
        EPOCH_TS=$(jq -r '.epoch_timestamp_ms // "N/A"' "$COMMITTEE_FILE" 2>/dev/null || echo "N/A")
        EPOCHS+=("$EPOCH")
        TIMESTAMPS+=("$EPOCH_TS")
        echo "  Node $i: epoch=$EPOCH, timestamp_ms=$EPOCH_TS"
    else
        echo "  Node $i: ❌ Committee file not found"
        EPOCHS+=("N/A")
        TIMESTAMPS+=("N/A")
    fi
done

# Check if epochs are the same
UNIQUE_EPOCHS=($(printf '%s\n' "${EPOCHS[@]}" | sort -u))
if [ ${#UNIQUE_EPOCHS[@]} -gt 1 ]; then
    print_error "⚠️  EPOCH DIVERGENCE DETECTED!"
    echo "   Nodes are in different epochs: ${UNIQUE_EPOCHS[*]}"
    echo "   This prevents consensus from working!"
fi

# Check if timestamps are the same for same epoch
for epoch in "${UNIQUE_EPOCHS[@]}"; do
    if [ "$epoch" != "N/A" ]; then
        TS_FOR_EPOCH=()
        for i in {0..3}; do
            if [ "${EPOCHS[$i]}" = "$epoch" ]; then
                TS_FOR_EPOCH+=("${TIMESTAMPS[$i]}")
            fi
        done
        UNIQUE_TS=($(printf '%s\n' "${TS_FOR_EPOCH[@]}" | sort -u))
        if [ ${#UNIQUE_TS[@]} -gt 1 ]; then
            print_error "⚠️  TIMESTAMP DIVERGENCE for epoch $epoch!"
            echo "   Nodes in same epoch have different timestamps: ${UNIQUE_TS[*]}"
            echo "   This causes genesis blocks to have different hashes!"
            echo "   Consensus cannot work because nodes cannot validate each other's blocks!"
        fi
    fi
done

echo ""
print_header "2. Commit Index Check"
for i in {0..3}; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ -f "$LOG_FILE" ]; then
        COMMIT_INDEX=$(grep "synced_commit_index=" "$LOG_FILE" | tail -1 | grep -oP 'synced_commit_index=\K[0-9]+' || echo "N/A")
        echo "  Node $i: synced_commit_index=$COMMIT_INDEX"
    fi
done

echo ""
print_header "3. Consensus Activity Check"
for i in {0..3}; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ -f "$LOG_FILE" ]; then
        LAST_BLOCK=$(grep -i "block.*created\|round.*start" "$LOG_FILE" | tail -1 | cut -d' ' -f1-2 || echo "N/A")
        LAST_COMMIT=$(grep "Executing commit" "$LOG_FILE" | tail -1 | cut -d' ' -f1-2 || echo "N/A")
        echo "  Node $i:"
        echo "    Last block: $LAST_BLOCK"
        echo "    Last commit: $LAST_COMMIT"
    fi
done

echo ""
print_header "4. Error/Warning Check"
for i in {0..3}; do
    LOG_FILE="logs/latest/node_${i}.log"
    if [ -f "$LOG_FILE" ]; then
        ERROR_COUNT=$(grep -i "error\|warn\|invalid\|failed" "$LOG_FILE" | tail -20 | wc -l)
        if [ "$ERROR_COUNT" -gt 0 ]; then
            echo "  Node $i: $ERROR_COUNT recent errors/warnings"
            grep -i "error\|warn\|invalid.*block\|ancestor.*not found" "$LOG_FILE" | tail -5 | while read line; do
                echo "    $line"
            done
        fi
    fi
done

echo ""
print_header "5. Recommendations"
echo ""
if [ ${#UNIQUE_EPOCHS[@]} -gt 1 ]; then
    print_error "❌ VẤN ĐỀ: Epoch divergence"
    echo "   - Nodes đang ở các epoch khác nhau"
    echo "   - Cần restart tất cả nodes để sync lại epoch"
    echo ""
    echo "   Giải pháp:"
    echo "   1. Stop tất cả nodes: ./stop_nodes.sh"
    echo "   2. Đồng bộ committee.json: Copy từ node có epoch cao nhất"
    echo "   3. Restart tất cả nodes: ./run_nodes.sh"
    echo ""
fi

if [ ${#UNIQUE_TS[@]} -gt 1 ] 2>/dev/null; then
    print_error "❌ VẤN ĐỀ: Timestamp divergence"
    echo "   - Nodes trong cùng epoch có timestamp khác nhau"
    echo "   - Genesis blocks có hash khác nhau → không thể validate blocks"
    echo "   - Consensus không hoạt động"
    echo ""
    echo "   Giải pháp:"
    echo "   1. Stop tất cả nodes: ./stop_nodes.sh"
    echo "   2. Đồng bộ timestamp trong committee.json: Dùng cùng timestamp cho tất cả nodes"
    echo "   3. Restart tất cả nodes: ./run_nodes.sh"
    echo ""
fi

print_info "💡 Để fix vấn đề:"
echo "   1. Stop tất cả nodes: ./stop_nodes.sh"
echo "   2. Đồng bộ committee.json: Tất cả nodes phải có cùng epoch và timestamp"
echo "   3. Restart: ./run_nodes.sh"
echo ""

