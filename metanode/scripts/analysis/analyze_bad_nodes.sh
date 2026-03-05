#!/bin/bash
# Script phân tích bad nodes - tìm nguyên nhân tại sao có 1 bad node
# Đặc biệt cho trường hợp tất cả nodes chạy trên cùng máy localhost
#
# Usage:
#   ./analyze_bad_nodes.sh          # Chạy một lần
#   ./analyze_bad_nodes.sh --watch # Refresh liên tục mỗi 5 giây
#   ./analyze_bad_nodes.sh -w 10   # Refresh mỗi 10 giây

set -euo pipefail

# Get script directory and change to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

NODES=4
METRICS_BASE_PORT=9100
WATCH_MODE=false
REFRESH_INTERVAL=5

# Parse arguments
if [ "$#" -gt 0 ]; then
    if [ "$1" == "--watch" ] || [ "$1" == "-w" ]; then
        WATCH_MODE=true
        if [ "$#" -gt 1 ] && [[ "$2" =~ ^[0-9]+$ ]]; then
            REFRESH_INTERVAL=$2
        fi
    elif [[ "$1" =~ ^[0-9]+$ ]]; then
        WATCH_MODE=true
        REFRESH_INTERVAL=$1
    fi
fi

TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S')

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔍 Phân Tích Bad Nodes (Localhost Environment)"
echo "📅 Thời gian: $TIMESTAMP"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Lưu trữ dữ liệu
declare -A reputation_scores
declare -A missing_blocks_total
declare -A missing_blocks_current
declare -A block_receive_delay
declare -A accepted_blocks_own
declare -A accepted_blocks_others
declare -A committed_leaders
declare -A leader_wait_count
declare -A leader_wait_ms
declare -A block_proposal_interval_sum
declare -A block_proposal_interval_count
declare -A block_commit_latency_sum
declare -A block_commit_latency_count
declare -A bad_nodes_count
declare -A last_commit_index
declare -A last_global_exec_index
declare -A current_epoch
declare -A highest_accepted_round
declare -A commit_sync_quorum_index
declare -A commit_sync_local_index
declare -A round_tracker_last_propagation_delay
declare -A subscribed_blocks
declare -A verified_blocks
declare -A commit_round_advancement_interval_sum
declare -A commit_round_advancement_interval_count

# Lưu trữ dữ liệu lần trước để so sánh (watch mode)
declare -A prev_reputation_scores
declare -A prev_committed_leaders
declare -A prev_missing_blocks_total
declare -A prev_accepted_blocks_own
declare -A prev_accepted_blocks_others

# Function để thu thập metrics
collect_metrics() {
    local iteration=${1:-1}
    local clear_screen=${2:-false}
    
    if [ "$clear_screen" = true ] && [ "$iteration" -gt 1 ]; then
        clear
    fi
    
    TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S')
    
    if [ "$WATCH_MODE" = true ]; then
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "🔍 Phân Tích Bad Nodes (Localhost Environment) - WATCH MODE"
        echo "📅 Thời gian: $TIMESTAMP | Lần refresh: $iteration | Interval: ${REFRESH_INTERVAL}s"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "💡 Nhấn Ctrl+C để dừng"
        echo ""
    else
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "🔍 Phân Tích Bad Nodes (Localhost Environment)"
        echo "📅 Thời gian: $TIMESTAMP"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
    fi

# Thu thập metrics từ tất cả nodes
echo "📊 Đang thu thập metrics từ tất cả nodes..."
echo ""

for i in $(seq 0 $((NODES-1))); do
    metrics_port=$((METRICS_BASE_PORT + i))
    node_name="node-$i"
    
    if [ "$WATCH_MODE" = false ] || [ "$iteration" -eq 1 ]; then
        echo "  Đang lấy metrics từ node $i (port $metrics_port)..."
    fi
    
    # Force refresh: thêm timestamp vào URL để tránh cache
    metrics=$(curl -s "http://127.0.0.1:$metrics_port/metrics?t=$(date +%s)" 2>/dev/null || echo "")
    
    if [ -z "$metrics" ]; then
        if [ "$WATCH_MODE" = false ] || [ "$iteration" -eq 1 ]; then
            echo "    ⚠️  Không thể kết nối đến node $i"
        fi
        continue
    fi
    
    # Reputation score - chỉ lấy giá trị đầu tiên và loại bỏ newline/whitespace
    rep_score=$(echo "$metrics" | grep "reputation_scores{authority=\"$node_name\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    # Đảm bảo giá trị là số nguyên hợp lệ - sử dụng echo -n để tránh thêm newline
    rep_score=$(echo -n "$rep_score" | grep -oE '^[0-9]+' | head -1 || echo "0")
    
    # Lưu giá trị cũ để so sánh (watch mode) - phải lưu TRƯỚC KHI cập nhật
    if [ "$WATCH_MODE" = true ] && [ "$iteration" -gt 1 ]; then
        prev_reputation_scores[$i]=${reputation_scores[$i]:-0}
    fi
    reputation_scores[$i]=$rep_score
    
    # Missing blocks - sanitize giá trị
    missing_total=$(echo "$metrics" | grep "^missing_blocks_total " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    missing_total=$(echo -n "$missing_total" | grep -oE '^[0-9]+' | head -1 || echo "0")
    missing_current=$(echo "$metrics" | grep "^block_manager_missing_blocks " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    missing_current=$(echo -n "$missing_current" | grep -oE '^[0-9]+' | head -1 || echo "0")
    
    # Lưu giá trị cũ để so sánh (watch mode)
    if [ "$WATCH_MODE" = true ] && [ "$iteration" -gt 1 ]; then
        prev_missing_blocks_total[$i]=${missing_blocks_total[$i]:-0}
    fi
    missing_blocks_total[$i]=$missing_total
    missing_blocks_current[$i]=$missing_current
    
    # Block receive delay - đây là counter (tổng delay), lấy giá trị cao nhất từ tất cả registry
    receive_delay=$(echo "$metrics" | grep "block_receive_delay{authority=\"$node_name\"}" | awk '{print $2}' | while read val; do echo "$val"; done | sort -n | tail -1 || echo "0")
    receive_delay=$(echo -n "$receive_delay" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    block_receive_delay[$i]=$receive_delay
    
    # Accepted blocks - sanitize giá trị
    accepted_own=$(echo "$metrics" | grep "accepted_blocks{source=\"own\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    accepted_own=$(echo -n "$accepted_own" | grep -oE '^[0-9]+' | head -1 || echo "0")
    accepted_others=$(echo "$metrics" | grep "accepted_blocks{source=\"others\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    accepted_others=$(echo -n "$accepted_others" | grep -oE '^[0-9]+' | head -1 || echo "0")
    
    # Lưu giá trị cũ để so sánh (watch mode)
    if [ "$WATCH_MODE" = true ] && [ "$iteration" -gt 1 ]; then
        prev_accepted_blocks_own[$i]=${accepted_blocks_own[$i]:-0}
        prev_accepted_blocks_others[$i]=${accepted_blocks_others[$i]:-0}
        prev_missing_blocks_total[$i]=${missing_blocks_total[$i]:-0}
    fi
    accepted_blocks_own[$i]=$accepted_own
    accepted_blocks_others[$i]=$accepted_others
    
    # Committed leaders - lấy tổng của tất cả commit_type (direct-commit, indirect-commit, direct-skip, indirect-skip)
    # Metric format: committed_leaders_total{authority="node-X",commit_type="..."} value
    # Sử dụng pattern chính xác hơn và escape đúng
    committed=$(echo "$metrics" | grep -E "committed_leaders_total\{authority=\"$node_name\"" | awk '{sum+=$2} END {print sum+0}' || echo "0")
    
    # Debug: hiển thị metric raw nếu cần (chỉ lần đầu trong watch mode)
    if [ "$WATCH_MODE" = true ] && [ "$iteration" -eq 1 ] && [ "$i" -eq 0 ]; then
        echo "    Debug: committed_leaders_total cho $node_name:"
        echo "$metrics" | grep -E "committed_leaders_total\{authority=\"$node_name\"" | head -3 || echo "      (không tìm thấy)"
    fi
    
    # Lưu giá trị cũ để so sánh (watch mode)
    if [ "$WATCH_MODE" = true ] && [ "$iteration" -gt 1 ]; then
        prev_committed_leaders[$i]=${committed_leaders[$i]:-0}
    fi
    committed_leaders[$i]=$committed
    
    # Leader wait - sanitize giá trị
    wait_count=$(echo "$metrics" | grep "block_proposal_leader_wait_count{authority=\"$node_name\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    wait_count=$(echo -n "$wait_count" | grep -oE '^[0-9]+' | head -1 || echo "0")
    wait_ms=$(echo "$metrics" | grep "block_proposal_leader_wait_ms{authority=\"$node_name\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    wait_ms=$(echo -n "$wait_ms" | grep -oE '^[0-9]+' | head -1 || echo "0")
    leader_wait_count[$i]=$wait_count
    leader_wait_ms[$i]=$wait_ms
    
    # Block proposal interval
    proposal_sum=$(echo "$metrics" | grep "^block_proposal_interval_sum " | awk '{print $2}' || echo "0")
    proposal_count=$(echo "$metrics" | grep "^block_proposal_interval_count " | awk '{print $2}' || echo "0")
    block_proposal_interval_sum[$i]=$proposal_sum
    block_proposal_interval_count[$i]=$proposal_count
    
    # Block commit latency
    commit_sum=$(echo "$metrics" | grep "^block_commit_latency_sum " | awk '{print $2}' || echo "0")
    commit_count=$(echo "$metrics" | grep "^block_commit_latency_count " | awk '{print $2}' || echo "0")
    block_commit_latency_sum[$i]=$commit_sum
    block_commit_latency_count[$i]=$commit_count
    
    # Bad nodes count - sanitize giá trị
    bad_count=$(echo "$metrics" | grep "^num_of_bad_nodes " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    bad_count=$(echo -n "$bad_count" | grep -oE '^[0-9]+' | head -1 || echo "0")
    bad_nodes_count[$i]=$bad_count
    
    # Last commit index (để kiểm tra epoch transition)
    # Có thể có nhiều giá trị từ các registry khác nhau (từ các epoch khác nhau)
    # Lấy giá trị CAO NHẤT vì đó là giá trị mới nhất từ epoch hiện tại
    commit_idx=$(echo "$metrics" | grep "^last_commit_index " | awk '{print $2}' | while read val; do echo "$val"; done | sort -n | tail -1 || echo "0")
    # Sanitize giá trị - loại bỏ newline và whitespace
    commit_idx=$(echo -n "$commit_idx" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    # Đảm bảo giá trị là số nguyên hợp lệ
    if [ -z "$commit_idx" ] || [ "$commit_idx" = "" ]; then
        commit_idx="0"
    fi
    last_commit_index[$i]=$commit_idx
    
    # Current epoch và Global Index - lấy từ logs vì không có metric Prometheus
    # Tìm epoch và global index mới nhất trong log file
    log_file="logs/latest/node_${i}.log"
    epoch="N/A"
    global_index="N/A"
    if [ -f "$log_file" ]; then
        # Tìm epoch từ log messages (format: epoch=211)
        epoch_from_log=$(tail -100 "$log_file" | grep -oE "epoch=[0-9]+" | tail -1 | cut -d'=' -f2 || echo "")
        if [ -n "$epoch_from_log" ]; then
            epoch=$epoch_from_log
        else
            # Thử tìm từ format khác: "epoch 211" hoặc "epoch: 211"
            epoch_from_log=$(tail -100 "$log_file" | grep -oE "epoch[:\s]+[0-9]+" | tail -1 | grep -oE "[0-9]+" || echo "")
            if [ -n "$epoch_from_log" ]; then
                epoch=$epoch_from_log
            fi
        fi
        
        # Tìm Global Index từ log messages (format mới: [Global Index: 268054] hoặc format cũ: checkpoint_seq=268054)
        global_index_from_log=$(tail -100 "$log_file" | grep -oE "\[Global Index: [0-9]+\]" | tail -1 | grep -oE "[0-9]+" || echo "")
        if [ -n "$global_index_from_log" ]; then
            global_index=$global_index_from_log
        else
            # Thử format cũ: checkpoint_seq=268054
            global_index_from_log=$(tail -100 "$log_file" | grep -oE "checkpoint_seq=[0-9]+" | tail -1 | cut -d'=' -f2 || echo "")
            if [ -n "$global_index_from_log" ]; then
                global_index=$global_index_from_log
            fi
        fi
    fi
    current_epoch[$i]=$epoch
    last_global_exec_index[$i]=$global_index
    
    # Highest accepted round - quan trọng để biết node có đang lag không
    highest_round=$(echo "$metrics" | grep "^highest_accepted_round " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    highest_round=$(echo -n "$highest_round" | grep -oE '^[0-9]+' | head -1 || echo "0")
    highest_accepted_round[$i]=$highest_round
    
    # Commit sync indices - để biết node có đang sync đúng không
    sync_quorum=$(echo "$metrics" | grep "^commit_sync_quorum_index " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    sync_quorum=$(echo -n "$sync_quorum" | grep -oE '^[0-9]+' | head -1 || echo "0")
    commit_sync_quorum_index[$i]=$sync_quorum
    
    sync_local=$(echo "$metrics" | grep "^commit_sync_local_index " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    sync_local=$(echo -n "$sync_local" | grep -oE '^[0-9]+' | head -1 || echo "0")
    commit_sync_local_index[$i]=$sync_local
    
    # Round tracker propagation delay - quan trọng cho network performance
    prop_delay=$(echo "$metrics" | grep "^round_tracker_last_propagation_delay " | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    prop_delay=$(echo -n "$prop_delay" | grep -oE '^[0-9]+' | head -1 || echo "0")
    round_tracker_last_propagation_delay[$i]=$prop_delay
    
    # Subscribed and verified blocks - để biết throughput
    # subscribed_blocks và verified_blocks có label authority, không phải source
    sub_blocks=$(echo "$metrics" | grep "subscribed_blocks{authority=\"$node_name\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    sub_blocks=$(echo -n "$sub_blocks" | grep -oE '^[0-9]+' | head -1 || echo "0")
    subscribed_blocks[$i]=$sub_blocks
    
    ver_blocks=$(echo "$metrics" | grep "verified_blocks{authority=\"$node_name\"}" | head -1 | awk '{print $2}' | tr -d '\n\r\t ' || echo "0")
    ver_blocks=$(echo -n "$ver_blocks" | grep -oE '^[0-9]+' | head -1 || echo "0")
    verified_blocks[$i]=$ver_blocks
    
    # Commit round advancement interval
    adv_sum=$(echo "$metrics" | grep "^commit_round_advancement_interval_sum " | awk '{print $2}' || echo "0")
    adv_count=$(echo "$metrics" | grep "^commit_round_advancement_interval_count " | awk '{print $2}' || echo "0")
    commit_round_advancement_interval_sum[$i]=$adv_sum
    commit_round_advancement_interval_count[$i]=$adv_count
done

# End of collect_metrics function
}

# Function để hiển thị metrics
display_metrics() {
    local display_iteration=${1:-1}
    
    echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📈 So Sánh Metrics Giữa Các Nodes"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

printf "%-8s %-12s %-15s %-15s %-20s %-15s %-15s %-15s\n" \
    "Node" "Reputation" "Missing(Total)" "Missing(Cur)" "ReceiveDelay(Total)" "AcceptedOwn" "AcceptedOther" "Committed"
echo "────────────────────────────────────────────────────────────────────────────────────────────────────────────────────"

for i in $(seq 0 $((NODES-1))); do
    node_name="node-$i"
    rep=${reputation_scores[$i]:-0}
    # Loại bỏ newline và whitespace, chỉ lấy số nguyên - đảm bảo không có newline
    rep=$(echo -n "$rep" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    miss_tot=${missing_blocks_total[$i]:-0}
    miss_tot=$(echo -n "$miss_tot" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    miss_cur=${missing_blocks_current[$i]:-0}
    miss_cur=$(echo -n "$miss_cur" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    delay=${block_receive_delay[$i]:-0}
    delay=$(echo -n "$delay" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    acc_own=${accepted_blocks_own[$i]:-0}
    acc_own=$(echo -n "$acc_own" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    acc_oth=${accepted_blocks_others[$i]:-0}
    acc_oth=$(echo -n "$acc_oth" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    comm=${committed_leaders[$i]:-0}
    comm=$(echo -n "$comm" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    
    # Hiển thị delta nếu watch mode và không phải lần đầu
    delta_comm=""
    if [ "$WATCH_MODE" = true ] && [ "$iteration" -gt 1 ] && [ -n "${prev_committed_leaders[$i]:-}" ]; then
        prev_comm=${prev_committed_leaders[$i]:-0}
        prev_comm=$(echo -n "$prev_comm" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
        if [ -n "$comm" ] && [ -n "$prev_comm" ] && [ "$comm" -gt 0 ] 2>/dev/null && [ "$prev_comm" -gt 0 ] 2>/dev/null; then
            delta=$((comm - prev_comm))
            if [ "$delta" -gt 0 ]; then
                delta_comm=" (+$delta)"
            elif [ "$delta" -lt 0 ]; then
                delta_comm=" ($delta)"
            fi
        fi
    fi
    
    # Format delay với "ms" suffix (đây là tổng delay, không phải trung bình)
    delay_display="${delay}ms"
    
    printf "%-8s %-12s %-15s %-15s %-20s %-15s %-15s %-15s\n" \
        "$node_name" "$rep" "$miss_tot" "$miss_cur" "$delay_display" "$acc_own" "$acc_oth" "$comm$delta_comm"
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "⏱️  Performance Metrics"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

printf "%-8s %-20s %-20s %-20s %-20s\n" \
    "Node" "LeaderWait(Count)" "LeaderWait(Time)" "ProposalInterval" "CommitLatency"
echo "────────────────────────────────────────────────────────────────────────────────────────────"

for i in $(seq 0 $((NODES-1))); do
    node_name="node-$i"
    wait_cnt=${leader_wait_count[$i]:-0}
    wait_cnt=$(echo -n "$wait_cnt" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    wait_time=${leader_wait_ms[$i]:-0}
    wait_time=$(echo -n "$wait_time" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    
    # Calculate average proposal interval
    prop_sum=${block_proposal_interval_sum[$i]:-0}
    prop_sum=$(echo -n "$prop_sum" | tr -d '\n\r\t ' | grep -oE '^[0-9.]+' | head -1 || echo "0")
    prop_cnt=${block_proposal_interval_count[$i]:-0}
    prop_cnt=$(echo -n "$prop_cnt" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    if [ -n "$prop_cnt" ] && [ -n "$prop_sum" ] && [ "$prop_cnt" != "0" ] && [ "$prop_sum" != "0" ] 2>/dev/null; then
        prop_avg=$(python3 -c "print(f'{${prop_sum}/${prop_cnt}:.3f}')" 2>/dev/null || echo "N/A")
    else
        prop_avg="N/A"
    fi
    
    # Calculate average commit latency
    commit_sum=${block_commit_latency_sum[$i]:-0}
    commit_sum=$(echo -n "$commit_sum" | tr -d '\n\r\t ' | grep -oE '^[0-9.]+' | head -1 || echo "0")
    commit_cnt=${block_commit_latency_count[$i]:-0}
    commit_cnt=$(echo -n "$commit_cnt" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    if [ -n "$commit_cnt" ] && [ -n "$commit_sum" ] && [ "$commit_cnt" != "0" ] && [ "$commit_sum" != "0" ] 2>/dev/null; then
        commit_avg=$(python3 -c "print(f'{${commit_sum}/${commit_cnt}:.3f}')" 2>/dev/null || echo "N/A")
    else
        commit_avg="N/A"
    fi
    
    # Format values với suffix
    wait_time_display="${wait_time}ms"
    prop_avg_display="${prop_avg}s"
    commit_avg_display="${commit_avg}s"
    
    printf "%-8s %-20s %-20s %-20s %-20s\n" \
        "$node_name" "$wait_cnt" "$wait_time_display" "$prop_avg_display" "$commit_avg_display"
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔄 Sync & Round Status"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

printf "%-8s %-20s %-20s %-20s %-20s %-20s\n" \
    "Node" "HighestAcceptedRound" "SyncQuorumIndex" "SyncLocalIndex" "PropagationDelay" "Subscribed/Verified"
echo "────────────────────────────────────────────────────────────────────────────────────────────────────────────────────"

for i in $(seq 0 $((NODES-1))); do
    node_name="node-$i"
    high_round=${highest_accepted_round[$i]:-0}
    high_round=$(echo -n "$high_round" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    sync_quorum=${commit_sync_quorum_index[$i]:-0}
    sync_quorum=$(echo -n "$sync_quorum" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    sync_local=${commit_sync_local_index[$i]:-0}
    sync_local=$(echo -n "$sync_local" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    prop_delay=${round_tracker_last_propagation_delay[$i]:-0}
    prop_delay=$(echo -n "$prop_delay" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    sub_blocks=${subscribed_blocks[$i]:-0}
    sub_blocks=$(echo -n "$sub_blocks" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    ver_blocks=${verified_blocks[$i]:-0}
    ver_blocks=$(echo -n "$ver_blocks" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    
    # Tính sync lag
    sync_lag=$((sync_quorum - sync_local))
    sync_status=""
    if [ "$sync_lag" -gt 10 ]; then
        sync_status="🔴 Lag $sync_lag"
    elif [ "$sync_lag" -gt 5 ]; then
        sync_status="🟡 Lag $sync_lag"
    else
        sync_status="✅ Synced"
    fi
    
    # Format propagation delay
    prop_delay_display="${prop_delay}ms"
    if [ "$prop_delay" -gt 1000 ]; then
        prop_delay_display="🔴 ${prop_delay}ms"
    elif [ "$prop_delay" -gt 500 ]; then
        prop_delay_display="🟡 ${prop_delay}ms"
    fi
    
    blocks_display="${sub_blocks}/${ver_blocks}"
    
    printf "%-8s %-20s %-20s %-20s %-20s %-20s\n" \
        "$node_name" "$high_round" "$sync_quorum ($sync_status)" "$sync_local" "$prop_delay_display" "$blocks_display"
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Epoch & Commit Index Info"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

printf "%-8s %-15s %-20s %-20s %-30s\n" "Node" "Last Commit Index" "Global Index" "Epoch" "Status"
echo "────────────────────────────────────────────────────────────────────────────────────────────────────────────"
for i in $(seq 0 $((NODES-1))); do
    node_name="node-$i"
    commit_idx=${last_commit_index[$i]:-0}
    # Loại bỏ newline và whitespace, chỉ lấy số nguyên
    commit_idx=$(echo -n "$commit_idx" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    global_idx=${last_global_exec_index[$i]:-N/A}
    epoch=${current_epoch[$i]:-N/A}
    
    # Phân tích status dựa trên commit index
    status=""
    if [ "$epoch" != "N/A" ]; then
        # Tính trung bình commit index của các node khác
        total_other_idx=0
        count_other=0
        for j in $(seq 0 $((NODES-1))); do
            if [ "$j" != "$i" ]; then
                other_idx=${last_commit_index[$j]:-0}
                other_idx=$(echo -n "$other_idx" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
                if [ -n "$other_idx" ] && [ "$other_idx" -gt 0 ] 2>/dev/null; then
                    total_other_idx=$((total_other_idx + other_idx))
                    count_other=$((count_other + 1))
                fi
            fi
        done
        
        if [ "$count_other" -gt 0 ]; then
            avg_other_idx=$((total_other_idx / count_other))
            if [ -n "$commit_idx" ] && [ "$commit_idx" -gt 0 ] 2>/dev/null && [ "$avg_other_idx" -gt 0 ] 2>/dev/null; then
                diff=$((avg_other_idx - commit_idx))
                diff_percent=$(python3 -c "print(f'{${diff}*100/${avg_other_idx}:.1f}')" 2>/dev/null || echo "0.0")
                if [ "$diff" -gt 100 ]; then
                    status="⚠️  Chậm ${diff_percent}%"
                elif [ "$diff" -gt 50 ]; then
                    status="🟡 Chậm ${diff_percent}%"
                elif [ "$diff" -lt -50 ]; then
                    status="🟢 Nhanh hơn ${diff_percent#-}%"
                else
                    status="✅ Bình thường"
                fi
            else
                status="❓ Không xác định"
            fi
        else
            status="✅ Bình thường"
        fi
    else
        status="❓ Không có epoch"
    fi
    
    printf "%-8s %-15s %-20s %-20s %-30s\n" "$node_name" "$commit_idx" "$global_idx" "$epoch" "$status"
done
echo ""
echo "💡 Lưu ý:"
echo "  - Commit index sẽ reset về 0 khi epoch transition"
echo "  - Global Index (checkpoint_seq) là số tuần tự toàn cục, không reset khi epoch transition"
echo "  - Global Index tăng liên tục qua các epoch, giúp theo dõi tổng số commits đã thực hiện"
echo "  - Nếu commit index > 0 nhưng committed_leaders_total không thay đổi,"
echo "    có thể metrics đang giữ nguyên từ epoch đầu tiên"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "👑 Committed Leaders Statistics"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Tìm node có reputation thấp nhất (để sử dụng trong status)
min_rep=999999
min_node=-1
for i in $(seq 0 $((NODES-1))); do
    rep=${reputation_scores[$i]:-0}
    # Loại bỏ newline và whitespace, chỉ lấy số nguyên
    rep=$(echo "$rep" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    if [ -n "$rep" ] && [ "$rep" != "0" ] && [ "$rep" -lt "$min_rep" ] 2>/dev/null; then
        min_rep=$rep
        min_node=$i
    fi
done

# Tính tổng committed leaders
total_committed=0
for i in $(seq 0 $((NODES-1))); do
    comm=${committed_leaders[$i]:-0}
    total_committed=$((total_committed + comm))
done

# Tính trung bình
avg_committed=0
if [ "$NODES" -gt 0 ]; then
    avg_committed=$((total_committed / NODES))
fi

# Tìm node có committed leaders cao nhất và thấp nhất
max_committed=0
max_node=-1
min_committed=999999
min_committed_node=-1

for i in $(seq 0 $((NODES-1))); do
    comm=${committed_leaders[$i]:-0}
    if [ "$comm" -gt "$max_committed" ]; then
        max_committed=$comm
        max_node=$i
    fi
    if [ "$comm" -lt "$min_committed" ]; then
        min_committed=$comm
        min_committed_node=$i
    fi
done

# Điều chỉnh header cho phù hợp với cột rộng hơn
if [ "$WATCH_MODE" = true ] && [ "$display_iteration" -gt 1 ]; then
    printf "%-8s %-25s %-15s %-15s %-20s\n" \
        "Node" "Committed Leaders (Δ/rate)" "Percentage" "vs Average" "Status"
else
    printf "%-8s %-25s %-15s %-15s %-20s\n" \
        "Node" "Committed Leaders" "Percentage" "vs Average" "Status"
fi
echo "────────────────────────────────────────────────────────────────────────────────────────────────────────────"

for i in $(seq 0 $((NODES-1))); do
    node_name="node-$i"
    comm=${committed_leaders[$i]:-0}
    
    # Tính phần trăm
    percent="0.0"
    if [ "$total_committed" -gt 0 ]; then
        percent=$(python3 -c "print(f'{${comm}*100/${total_committed}:.1f}')" 2>/dev/null || echo "0.0")
    fi
    
    # So sánh với trung bình
    vs_avg=""
    if [ "$avg_committed" -gt 0 ]; then
        diff=$((comm - avg_committed))
        diff_percent=$(python3 -c "print(f'{${diff}*100/${avg_committed}:.1f}')" 2>/dev/null || echo "0.0")
        if [ "$diff" -gt 0 ]; then
            vs_avg="+${diff_percent}%"
        elif [ "$diff" -lt 0 ]; then
            vs_avg="${diff_percent}%"
        else
            vs_avg="0%"
        fi
    else
        vs_avg="N/A"
    fi
    
    # Xác định status
    status=""
    if [ "$comm" -eq "0" ]; then
        status="⚠️  No commits"
    elif [ "$i" -eq "$min_committed_node" ] && [ "$min_committed_node" -ge 0 ]; then
        status="🔴 Bad Node"
    elif [ "$i" -eq "$max_node" ] && [ "$max_node" -ge 0 ]; then
        status="🟢 Best"
    elif [ "$comm" -lt "$avg_committed" ]; then
        status="🟡 Below avg"
    else
        status="🟢 Good"
    fi
    
    # Hiển thị delta nếu watch mode và không phải lần đầu
    delta_comm=""
    rate_comm=""
    if [ "$WATCH_MODE" = true ] && [ "$display_iteration" -gt 1 ] && [ -n "${prev_committed_leaders[$i]:-}" ]; then
        prev_comm=${prev_committed_leaders[$i]:-0}
        delta=$((comm - prev_comm))
        if [ "$delta" -gt 0 ]; then
            delta_comm=" (+$delta)"
            # Tính rate (commits per interval)
            if [ "$REFRESH_INTERVAL" -gt 0 ]; then
                rate=$(python3 -c "print(f'{${delta}/${REFRESH_INTERVAL}:.2f}')" 2>/dev/null || echo "0.00")
                rate_comm=" (${rate}/s)"
            fi
        elif [ "$delta" -lt 0 ]; then
            delta_comm=" ($delta)"
        fi
    fi
    
    # Hiển thị với delta và rate
    display_comm="$comm$delta_comm$rate_comm"
    printf "%-8s %-25s %-15s %-15s %-20s\n" \
        "$node_name" "$display_comm" "${percent}%" "$vs_avg" "$status"
done

echo ""
echo "📊 Tổng số committed leaders: $total_committed"
echo "📊 Trung bình mỗi node: $avg_committed"
if [ "$max_node" -ge 0 ]; then
    echo "📊 Node có nhiều nhất: node-$max_node ($max_committed)"
fi
if [ "$min_committed_node" -ge 0 ] && [ "$min_committed_node" != "$max_node" ]; then
    echo "📊 Node có ít nhất: node-$min_committed_node ($min_committed)"
fi

# Phân tích sự khác biệt về commit index giữa các nodes
echo ""
echo "⚠️  Phân Tích Sự Khác Biệt Commit Index:"
total_commit_idx=0
max_commit_idx=0
min_commit_idx=999999
max_idx_node=-1
min_idx_node=-1

for i in $(seq 0 $((NODES-1))); do
    commit_idx=${last_commit_index[$i]:-0}
    # Loại bỏ newline và whitespace, chỉ lấy số nguyên
    commit_idx=$(echo -n "$commit_idx" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    if [ -n "$commit_idx" ] && [ "$commit_idx" -gt 0 ] 2>/dev/null; then
        total_commit_idx=$((total_commit_idx + commit_idx))
        if [ "$commit_idx" -gt "$max_commit_idx" ]; then
            max_commit_idx=$commit_idx
            max_idx_node=$i
        fi
        if [ "$commit_idx" -lt "$min_commit_idx" ]; then
            min_commit_idx=$commit_idx
            min_idx_node=$i
        fi
    fi
done

if [ "$max_idx_node" -ge 0 ] && [ "$min_idx_node" -ge 0 ] && [ "$max_idx_node" != "$min_idx_node" ]; then
    avg_commit_idx=$((total_commit_idx / NODES))
    diff=$((max_commit_idx - min_commit_idx))
    diff_percent=$(python3 -c "print(f'{${diff}*100/${max_commit_idx}:.1f}')" 2>/dev/null || echo "0.0")
    
    echo "  📊 Commit Index Range:"
    echo "     - Cao nhất: node-$max_idx_node ($max_commit_idx)"
    echo "     - Thấp nhất: node-$min_idx_node ($min_commit_idx)"
    echo "     - Trung bình: $avg_commit_idx"
    echo "     - Chênh lệch: $diff commits (${diff_percent}%)"
    
    if [ "$diff" -gt 200 ]; then
        echo "  🔴 CẢNH BÁO: Chênh lệch commit index lớn ($diff commits)"
        echo "     → Node-$min_idx_node có thể đang chậm hoặc đã restart gần đây"
        echo "     → Kiểm tra logs: tail -f logs/latest/node_${min_idx_node}.log"
        echo "     → Kiểm tra process: ps aux | grep 'metanode.*node_${min_idx_node}'"
    elif [ "$diff" -gt 100 ]; then
        echo "  🟡 CẢNH BÁO: Chênh lệch commit index đáng kể ($diff commits)"
        echo "     → Node-$min_idx_node có thể đang chậm hơn các node khác"
    else
        echo "  ✅ Commit index tương đối đồng bộ giữa các nodes"
    fi
fi

# Cảnh báo nếu có vấn đề với epoch transition
avg_commit_idx=$((total_commit_idx / NODES))
# Committed leaders và commit index không nhất thiết phải bằng nhau
# Committed leaders là số lần một node được chọn làm leader và commit thành công
# Commit index là số commit đã thực hiện (có thể có nhiều commits từ cùng một leader)
# Tỷ lệ hợp lý: committed_leaders_total thường nhỏ hơn commit index
expected_min_committed=$((avg_commit_idx / 4))  # Ít nhất 1/4 commits nên có committed leaders
if [ "$avg_commit_idx" -gt 100 ] && [ "$total_committed" -lt "$expected_min_committed" ]; then
    echo ""
    echo "  🟡 CẢNH BÁO: Commit index cao ($avg_commit_idx) nhưng committed_leaders_total thấp ($total_committed)"
    echo "     → Có thể metrics đang giữ nguyên từ epoch đầu tiên hoặc chưa được cập nhật"
    echo "     → Xem chi tiết: COMMITTED_LEADERS_METRIC_ANALYSIS.md"
elif [ "$avg_commit_idx" -gt 0 ] && [ "$total_committed" -eq 0 ]; then
    echo ""
    echo "  🔴 CẢNH BÁO: Có commits ($avg_commit_idx) nhưng committed_leaders_total = 0"
    echo "     → Metrics có thể không được cập nhật đúng"
    echo "     → Kiểm tra xem có epoch transition gần đây không"
fi

# Hiển thị thông tin về tốc độ commit (watch mode)
if [ "$WATCH_MODE" = true ] && [ "$display_iteration" -gt 1 ]; then
    echo ""
    echo "📈 Tốc độ commit (trong ${REFRESH_INTERVAL}s vừa qua):"
    total_delta=0
    for i in $(seq 0 $((NODES-1))); do
        if [ -n "${prev_committed_leaders[$i]:-}" ]; then
            prev_comm=${prev_committed_leaders[$i]:-0}
            curr_comm=${committed_leaders[$i]:-0}
            delta=$((curr_comm - prev_comm))
            total_delta=$((total_delta + delta))
            if [ "$delta" -gt 0 ]; then
                rate=$(python3 -c "print(f'{${delta}/${REFRESH_INTERVAL}:.2f}')" 2>/dev/null || echo "0.00")
                echo "  node-$i: +$delta commits (${rate} commits/s)"
            elif [ "$delta" -eq 0 ]; then
                echo "  node-$i: không thay đổi"
            fi
        fi
    done
    if [ "$total_delta" -gt 0 ]; then
        total_rate=$(python3 -c "print(f'{${total_delta}/${REFRESH_INTERVAL}:.2f}')" 2>/dev/null || echo "0.00")
        echo "  Tổng: +$total_delta commits (${total_rate} commits/s)"
    fi
fi

# Phân tích phân bố
echo ""
echo "💡 Phân tích phân bố:"
if [ "$total_committed" -gt 0 ]; then
    # Tính độ lệch chuẩn (đơn giản)
    variance=0
    for i in $(seq 0 $((NODES-1))); do
        comm=${committed_leaders[$i]:-0}
        diff=$((comm - avg_committed))
        variance=$((variance + diff * diff))
    done
    if [ "$NODES" -gt 0 ]; then
        variance=$((variance / NODES))
    fi
    
    # Đánh giá phân bố
    if [ "$variance" -lt "$((avg_committed * avg_committed / 10))" ]; then
        echo "  ✅ Phân bố tương đối đều - các nodes có performance tương đương"
    elif [ "$variance" -lt "$((avg_committed * avg_committed / 4))" ]; then
        echo "  ⚠️  Phân bố không đều - có sự khác biệt giữa các nodes"
    else
        echo "  🔴 Phân bố rất không đều - có node performance kém đáng kể"
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔍 Phân Tích Nguyên Nhân"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ "$min_node" -ge 0 ]; then
    echo "⚠️  Node có reputation thấp nhất: node-$min_node (score: $min_rep)"
    echo ""
    
    echo "Các metrics của node-$min_node so với các nodes khác:"
    echo ""
    
    # So sánh với average
    total_rep=0
    count=0
    for i in $(seq 0 $((NODES-1))); do
        rep=${reputation_scores[$i]:-0}
        # Loại bỏ newline và whitespace, chỉ lấy số nguyên
        rep=$(echo "$rep" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
        if [ -n "$rep" ] && [ "$rep" != "0" ] && [ "$rep" -gt 0 ] 2>/dev/null; then
            total_rep=$((total_rep + rep))
            count=$((count + 1))
        fi
    done
    
    if [ "$count" -gt 0 ]; then
        avg_rep=$((total_rep / count))
        diff=$((avg_rep - min_rep))
        diff_percent=$(python3 -c "print(f'{${diff}*100/${avg_rep}:.1f}')" 2>/dev/null || echo "N/A")
        echo "  - Reputation score: $min_rep (trung bình: $avg_rep, thấp hơn ${diff_percent}%)"
    fi
    
    # So sánh missing blocks - sanitize giá trị
    min_miss=${missing_blocks_total[$min_node]:-0}
    min_miss=$(echo "$min_miss" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    echo "  - Missing blocks (total): $min_miss"
    
    # So sánh block receive delay - sanitize giá trị
    min_delay=${block_receive_delay[$min_node]:-0}
    min_delay=$(echo "$min_delay" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    echo "  - Block receive delay: ${min_delay}ms"
    
    # So sánh leader wait - sanitize giá trị
    min_wait_count=${leader_wait_count[$min_node]:-0}
    min_wait_count=$(echo "$min_wait_count" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    min_wait_ms=${leader_wait_ms[$min_node]:-0}
    min_wait_ms=$(echo "$min_wait_ms" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    echo "  - Leader wait: $min_wait_count lần, tổng ${min_wait_ms}ms"
    
    # So sánh committed leaders với các nodes khác
    min_committed=${committed_leaders[$min_node]:-0}
    
    # Tính trung bình committed leaders (không bao gồm min_node)
    total_comm_others=0
    count_others=0
    max_comm_others=0
    for i in $(seq 0 $((NODES-1))); do
        if [ "$i" != "$min_node" ]; then
            comm=${committed_leaders[$i]:-0}
            total_comm_others=$((total_comm_others + comm))
            count_others=$((count_others + 1))
            if [ "$comm" -gt "$max_comm_others" ]; then
                max_comm_others=$comm
            fi
        fi
    done
    
    avg_comm_others=0
    if [ "$count_others" -gt 0 ]; then
        avg_comm_others=$((total_comm_others / count_others))
    fi
    
    echo "  - Committed leaders: $min_committed"
    if [ "$avg_comm_others" -gt 0 ]; then
        diff_comm=$((min_committed - avg_comm_others))
        diff_comm_percent=$(python3 -c "print(f'{${diff_comm}*100/${avg_comm_others}:.1f}')" 2>/dev/null || echo "0.0")
        if [ "$diff_comm" -lt 0 ]; then
            echo "    (Trung bình các nodes khác: $avg_comm_others, thấp hơn ${diff_comm_percent#-}%)"
        elif [ "$diff_comm" -gt 0 ]; then
            echo "    (Trung bình các nodes khác: $avg_comm_others, cao hơn ${diff_comm_percent}%)"
        else
            echo "    (Trung bình các nodes khác: $avg_comm_others, bằng nhau)"
        fi
        if [ "$max_comm_others" -gt 0 ]; then
            echo "    (Node tốt nhất: $max_comm_others)"
        fi
    fi
    
    echo ""
    echo "💡 Nguyên nhân có thể:"
echo ""

    # Đảm bảo giá trị đã được sanitize trước khi so sánh
    min_miss=$(echo "$min_miss" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    min_delay=$(echo "$min_delay" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    min_wait_count=$(echo "$min_wait_count" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    min_wait_ms=$(echo "$min_wait_ms" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    
    if [ -n "$min_miss" ] && [ "$min_miss" -gt "0" ] 2>/dev/null; then
        echo "  ⚠️  Có missing blocks ($min_miss) - có thể do:"
        echo "     - Network latency cao (ngay cả trên localhost)"
        echo "     - Process scheduling issues"
        echo "     - Disk I/O contention"
    fi
    
    if [ -n "$min_delay" ] && [ "$min_delay" -gt "1000" ] 2>/dev/null; then
        echo "  ⚠️  Block receive delay cao (${min_delay}ms) - có thể do:"
        echo "     - CPU contention"
        echo "     - Process priority thấp"
        echo "     - Context switching overhead"
    fi
    
    if [ -n "$min_wait_count" ] && [ "$min_wait_count" -gt "0" ] 2>/dev/null; then
        avg_wait=$(python3 -c "print(f'{${min_wait_ms}/${min_wait_count}:.1f}')" 2>/dev/null || echo "N/A")
        echo "  ⚠️  Leader wait cao ($min_wait_count lần, avg ${avg_wait}ms) - có thể do:"
        echo "     - Timing issues"
        echo "     - Clock synchronization"
        echo "     - Process scheduling delays"
    fi
    
    if [ "$avg_comm_others" -gt 0 ] && [ "$min_committed" -lt "$avg_comm_others" ]; then
        diff_comm=$((avg_comm_others - min_committed))
        diff_percent=$(python3 -c "print(f'{${diff_comm}*100/${avg_comm_others}:.1f}')" 2>/dev/null || echo "0.0")
        echo "  ⚠️  Ít committed leaders ($min_committed vs trung bình $avg_comm_others, thấp hơn ${diff_percent}%) - có thể do:"
        echo "     - Ít được chọn làm leader do reputation thấp"
        echo "     - Performance kém hơn các nodes khác"
        echo "     - Timing issues khi propose blocks"
        echo "     - Block propagation chậm hơn"
    fi
    
    echo ""
    echo "🔧 Khuyến nghị:"
    echo "  1. Kiểm tra resource usage của node-$min_node:"
    echo "     - CPU: top -p \$(pgrep -f 'metanode.*node_$min_node')"
    echo "     - Memory: ps aux | grep 'metanode.*node_$min_node'"
    echo "     - Disk I/O: iostat -x 1"
    echo ""
    echo "  2. Kiểm tra logs của node-$min_node:"
    echo "     - tail -f logs/latest/node_$min_node.log | grep -i 'error\\|warn\\|delay\\|timeout'"
    echo ""
    echo "  3. Kiểm tra network trên localhost:"
    echo "     - netstat -an | grep '127.0.0.1:9'"
    echo "     - ss -tuln | grep '9'"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Bad Nodes Status"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

for i in $(seq 0 $((NODES-1))); do
    bad_count=${bad_nodes_count[$i]:-0}
    bad_count=$(echo "$bad_count" | tr -d '\n\r ' | grep -E '^[0-9]+$' | head -1 || echo "0")
    echo "  Node $i báo có $bad_count bad node(s)"
done

echo ""
echo "💡 Giải thích:"
total_bad_nodes=0
for i in $(seq 0 $((NODES-1))); do
    bad_count=${bad_nodes_count[$i]:-0}
    bad_count=$(echo -n "$bad_count" | tr -d '\n\r\t ' | grep -oE '^[0-9]+' | head -1 || echo "0")
    if [ -n "$bad_count" ] && [ "$bad_count" -gt 0 ] 2>/dev/null; then
        total_bad_nodes=$((total_bad_nodes + bad_count))
    fi
done

if [ "$total_bad_nodes" -gt 0 ]; then
    echo "  - Có $total_bad_nodes bad node(s) được phát hiện trong hệ thống"
    if [ "$min_node" -ge 0 ]; then
        echo "  - Node có reputation thấp nhất (node-$min_node) có thể được đánh dấu là bad"
    fi
    echo "  - Đây là behavior BÌNH THƯỜNG - hệ thống tự động phát hiện node performance kém"
    echo "  - Bad node sẽ được swap khi được chọn làm leader"
else
    echo "  - Không có bad node nào được phát hiện - tất cả nodes đang hoạt động tốt"
    echo "  - Reputation scores đang được tính toán và cập nhật"
fi
echo ""
}

# Main execution
main() {
    local iteration=1
    
    # Trap Ctrl+C để exit gracefully
    trap 'echo ""; echo "🛑 Đã dừng watch mode"; exit 0' INT
    
    while true; do
        collect_metrics $iteration true
        display_metrics $iteration
        
        # Watch mode: sleep và tiếp tục loop
        if [ "$WATCH_MODE" = true ]; then
            echo "⏳ Đang chờ ${REFRESH_INTERVAL}s để refresh... (Nhấn Ctrl+C để dừng)"
            sleep "$REFRESH_INTERVAL"
            iteration=$((iteration + 1))
        else
            # Normal mode: exit
            break
        fi
    done
}

# Run main function
main
