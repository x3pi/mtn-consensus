#!/bin/bash
# debug_rust_ram.sh — Rust Metanode RAM Profiler
#
# Mục tiêu: Đo RAM của các Rust metanode process theo thời gian,
#   xác định xem RAM đang tăng ở đâu (heap, anonymous, shared libs).
#
# Nguồn dữ liệu:
#   /proc/[PID]/status        → VmRSS, VmPeak, VmSize, Threads
#   /proc/[PID]/smaps_rollup  → Pss_Anon (heap/stack thực sự), Pss_File (mmap libs)
#   /proc/[PID]/smaps         → phân tích per-mapping (top allocators)
#
# Cách dùng:
#   bash debug_rust_ram.sh [interval_sec] [times]
#   bash debug_rust_ram.sh 30 10
#
# So sánh snapshot đầu và cuối để tìm memory leak:
#   diff <(sort ram_debug/YYYYMMDD_HH/snap_1/smaps_top_1.txt) \
#        <(sort ram_debug/YYYYMMDD_HH/snap_N/smaps_top_N.txt)

INTERVAL="${1:-30}"
TIMES="${2:-8}"
OUT_DIR="./ram_debug/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$OUT_DIR"

# ── ANSI Colors ───────────────────────────────────────────────────────────────
RED='\033[0;31m'; YELLOW='\033[1;33m'; GREEN='\033[0;32m'
CYAN='\033[0;36m'; BLUE='\033[0;34m'; MAGENTA='\033[0;35m'; NC='\033[0m'

echo -e "${BLUE}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║  🦀 RUST METANODE RAM PROFILER                              ║${NC}"
echo -e "${BLUE}║  Nguồn: /proc/PID/smaps_rollup + /proc/PID/status          ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  Snapshots    : ${TIMES} lần, mỗi ${INTERVAL}s"
echo -e "  Output dir   : ${CYAN}${OUT_DIR}${NC}"
echo ""

# ── Tìm tất cả metanode PIDs (chỉ binary thực, không phải bash wrapper) ────────
find_metanode_pids() {
    pgrep -f "metanode" 2>/dev/null | while read pid; do
        # Lọc: chỉ lấy pid có comm = "metanode" (binary thực, không phải bash)
        local comm
        comm=$(cat /proc/$pid/comm 2>/dev/null || echo "")
        if [ "$comm" != "metanode" ]; then
            continue
        fi
        # Lấy cmdline để hiển thị node_id
        local cmd
        cmd=$(tr '\0' ' ' < /proc/$pid/cmdline 2>/dev/null || echo "unknown")
        echo "$pid:$cmd"
    done
}

# Hiển thị PIDs đang chạy
echo -e "${CYAN}📋 Metanode processes đang chạy:${NC}"
PIDS_FOUND=0
find_metanode_pids | while IFS=: read pid cmd; do
    local_rss=$(awk '/VmRSS/{printf "%.0f MB", $2/1024}' /proc/$pid/status 2>/dev/null || echo "?")
    echo -e "  PID ${YELLOW}$pid${NC}: $cmd"
    echo -e "       → VmRSS hiện tại: ${GREEN}$local_rss${NC}"
    PIDS_FOUND=1
done

# Kiểm tra có process không
if [ -z "$(find_metanode_pids)" ]; then
    echo -e "${RED}❌ Không có metanode process nào đang chạy!${NC}"
    echo -e "   → Kiểm tra: pgrep -a metanode"
    exit 1
fi

echo ""

# ── Hàm đọc RAM từ /proc/PID/status ─────────────────────────────────────────
read_proc_status() {
    local pid=$1
    local status_file="/proc/$pid/status"
    if [ ! -f "$status_file" ]; then
        echo "DEAD"
        return
    fi
    awk '
        /VmPeak/{ peak=$2 }
        /VmSize/{ virt=$2 }
        /VmRSS/{ rss=$2 }
        /VmSwap/{ swap=$2 }
        /Threads/{ threads=$2 }
        END {
            printf "peak=%.0f virt=%.0f rss=%.0f swap=%.0f threads=%s",
                peak/1024, virt/1024, rss/1024, swap/1024, threads
        }
    ' "$status_file"
}

# ── Hàm đọc smaps_rollup (breakdown chi tiết) ───────────────────────────────
read_smaps_rollup() {
    local pid=$1
    local rollup="/proc/$pid/smaps_rollup"
    if [ ! -f "$rollup" ]; then
        # Fallback: tính từ smaps
        smaps="/proc/$pid/smaps"
        if [ ! -f "$smaps" ]; then echo "DEAD"; return; fi
        awk '
            /^Rss/{ rss+=$2 }
            /^Pss/{ pss+=$2 }
            /^Anonymous/{ anon+=$2 }
            /^Shared_Clean/{ s_clean+=$2 }
            /^Shared_Dirty/{ s_dirty+=$2 }
            /^Private_Dirty/{ p_dirty+=$2 }
            END {
                printf "rss=%.0f pss=%.0f anon=%.0f shared=%.0f private_dirty=%.0f",
                    rss/1024, pss/1024, anon/1024, (s_clean+s_dirty)/1024, p_dirty/1024
            }
        ' "$smaps"
        return
    fi
    awk '
        /^Rss/{ rss=$2 }
        /^Pss:/{ pss=$2 }
        /^Pss_Anon/{ pss_anon=$2 }
        /^Pss_File/{ pss_file=$2 }
        /^Anonymous/{ anon=$2 }
        /^Shared_Clean/{ s_clean=$2 }
        /^Shared_Dirty/{ s_dirty=$2 }
        /^Private_Dirty/{ p_dirty=$2 }
        END {
            printf "rss=%.0f pss=%.0f pss_anon=%.0f pss_file=%.0f anon=%.0f shared=%.0f private_dirty=%.0f",
                rss/1024, pss/1024, pss_anon/1024, pss_file/1024, anon/1024,
                (s_clean+s_dirty)/1024, p_dirty/1024
        }
    ' "$rollup"
}

# ── Hàm phân tích top memory mappings từ smaps ───────────────────────────────
analyze_smaps_top() {
    local pid=$1
    local out_file=$2
    local smaps="/proc/$pid/smaps"

    if [ ! -f "$smaps" ]; then
        echo "DEAD - process $pid không còn tồn tại" > "$out_file"
        return
    fi

    echo "=== TOP MEMORY MAPPINGS (PID $pid) ===" > "$out_file"
    echo "Phân tích: Private_Dirty = heap thực sự đang dùng" >> "$out_file"
    echo "" >> "$out_file"

    # Per-mapping: lấy tên vùng nhớ và Private_Dirty
    awk '
        /^[0-9a-f]/ {
            # Lấy tên mapping (field cuối cùng trên dòng địa chỉ)
            name = $NF
            if (name == "") name = "[anonymous]"
            cur_name = name
            cur_private_dirty = 0
            cur_rss = 0
        }
        /^Private_Dirty/ { cur_private_dirty = $2 }
        /^Rss:/ {
            cur_rss = $2
            # Gom theo tên
            private_dirty[cur_name] += cur_private_dirty
            rss_total[cur_name] += cur_rss
        }
    END {
        for (k in private_dirty) {
            printf "%10.1f MB private_dirty | %10.1f MB rss | %s\n",
                private_dirty[k]/1024, rss_total[k]/1024, k
        }
    }' "$smaps" | sort -rn >> "$out_file"

    echo "" >> "$out_file"
    echo "--- HEAP REGION (anonymous mappings) ---" >> "$out_file"
    awk '
        /^\[heap\]/ { in_heap=1; next }
        /^[0-9a-f]/ { in_heap=0 }
        in_heap && /^(Rss|Private_Dirty|Size):/ { print $0 }
    ' "$smaps" >> "$out_file" 2>/dev/null || echo "  (heap region không có trong smaps)" >> "$out_file"

    echo "" >> "$out_file"
    echo "--- STACK REGIONS ---" >> "$out_file"
    grep -c "\[stack" "$smaps" 2>/dev/null | xargs -I{} echo "  Số thread stacks: {}" >> "$out_file"
}

# ── Hàm lấy log gần đây từ tmux session ─────────────────────────────────────
extract_rust_log_metrics() {
    local node_id=$1
    local snap_dir=$2
    local snap_idx=$3
    local session="metanode-${node_id}"

    # Kiểm tra session tồn tại
    if ! tmux has-session -t "$session" 2>/dev/null; then
        return
    fi

    local log_out="${snap_dir}/rust_log_${node_id}_${snap_idx}.txt"
    echo "=== RUST LOG SNAPSHOT #${snap_idx} node=${node_id} ($(date +%H:%M:%S)) ===" > "$log_out"

    # Lấy output từ tmux pane (100 dòng cuối)
    tmux capture-pane -t "${session}" -p -S -100 >> "$log_out" 2>/dev/null || true

    # Đếm các loại sự kiện quan trọng
    local commits warnings errors
    commits=$(grep -c "commit\|Commit\|COMMIT" "$log_out" 2>/dev/null || echo 0)
    warnings=$(grep -c "WARN\|warn" "$log_out" 2>/dev/null || echo 0)
    errors=$(grep -c "ERROR\|error\|panic" "$log_out" 2>/dev/null || echo 0)

    echo "" >> "$log_out"
    echo "Events: commits=$commits warnings=$warnings errors=$errors" >> "$log_out"
}

# ── Main snapshot function ────────────────────────────────────────────────────
take_snapshot() {
    local idx=$1
    local ts
    ts=$(date +%H:%M:%S)
    local snap_dir="${OUT_DIR}/snap_${idx}"
    mkdir -p "$snap_dir"

    echo ""
    echo -e "═══════════════════════════════════════════════════════"
    echo -e "📸 ${CYAN}[${ts}] Snapshot #${idx}${NC}"
    echo -e "═══════════════════════════════════════════════════════"

    local summary_line="=== SNAPSHOT #${idx} === $(date)"$'\n'

    # ── Lặp qua tất cả metanode processes ───────────────────────────────────
    find_metanode_pids | while IFS=: read pid cmd; do
        # Lấy node_id từ cmdline (config/node_X.toml)
        node_id=$(echo "$cmd" | grep -oP 'node_\K[0-9]+' | head -1)
        node_id="${node_id:-?}"

        echo -e "  ${MAGENTA}[metanode-${node_id}]${NC} PID=${YELLOW}${pid}${NC}"

        # ── /proc/PID/status ────────────────────────────────────────────────
        status_data=$(read_proc_status "$pid")
        if [ "$status_data" = "DEAD" ]; then
            echo -e "   ${RED}❌ Process đã chết${NC}"
            continue
        fi

        # Parse các giá trị
        rss_mb=$(echo "$status_data"  | grep -oP 'rss=\K[0-9.]+')
        virt_mb=$(echo "$status_data" | grep -oP 'virt=\K[0-9.]+')
        peak_mb=$(echo "$status_data" | grep -oP 'peak=\K[0-9.]+')
        swap_mb=$(echo "$status_data" | grep -oP 'swap=\K[0-9.]+')
        threads=$(echo "$status_data" | grep -oP 'threads=\K[0-9]+')

        echo -e "   📊 VmRSS (Resident RAM)  : ${GREEN}${rss_mb} MB${NC}"
        echo -e "   📊 VmPeak (Peak RAM)     : ${YELLOW}${peak_mb} MB${NC}"
        echo -e "   📊 VmSize (Virtual)      : ${virt_mb} MB"
        echo -e "   📊 VmSwap               : ${swap_mb} MB"
        echo -e "   📊 Threads              : ${threads}"

        # Lưu status
        echo "$status_data" > "${snap_dir}/status_node${node_id}_${idx}.txt"

        # ── smaps_rollup (chi tiết phân vùng nhớ) ───────────────────────────
        rollup_data=$(read_smaps_rollup "$pid")
        pss_anon=$(echo "$rollup_data" | grep -oP 'pss_anon=\K[0-9.]+')
        pss_file=$(echo "$rollup_data" | grep -oP 'pss_file=\K[0-9.]+')
        # Dùng ' anon=' (có space) để không match pss_anon=
        anon_mb=$(echo "$rollup_data"  | grep -oP '(?<= )anon=\K[0-9.]+')

        echo -e "   🧠 Pss_Anon (heap+stack) : ${RED}${pss_anon:-?} MB${NC}  ← phần này Rust thực sự dùng"
        echo -e "   📁 Pss_File (mmap/libs)  : ${pss_file:-?} MB"
        echo -e "   🔒 Anonymous (total)     : ${anon_mb:-?} MB"

        echo "$rollup_data" > "${snap_dir}/smaps_rollup_node${node_id}_${idx}.txt"

        # ── Top mappings (smaps phân tích sâu) ──────────────────────────────
        analyze_smaps_top "$pid" "${snap_dir}/smaps_top_node${node_id}_${idx}.txt"
        echo -e "   ✅ smaps top mappings saved → smaps_top_node${node_id}_${idx}.txt"

        # ── Log từ tmux ──────────────────────────────────────────────────────
        extract_rust_log_metrics "$node_id" "$snap_dir" "$idx"
        echo -e "   ✅ rust log snapshot saved"

        # ── Summary append ──────────────────────────────────────────────────
        {
            echo "  node=${node_id} pid=${pid}: rss=${rss_mb}MB peak=${peak_mb}MB pss_anon=${pss_anon}MB threads=${threads}"
        } >> "${OUT_DIR}/summary.txt"
    done

    # ── Global system memory ─────────────────────────────────────────────────
    echo ""
    echo -e "  ${BLUE}[SYSTEM]${NC}"
    free -m | awk '
        /^Mem:/ {
            printf "   💻 System RAM: used=%d MB / total=%d MB (free=%d MB, available=%d MB)\n",
                $3, $2, $4, $7
        }
    '
    free -m >> "${snap_dir}/system_mem_${idx}.txt"

    echo "" >> "${OUT_DIR}/summary.txt"
}

# ── Main loop ─────────────────────────────────────────────────────────────────
echo "═══════════════════════════════════════════════════════"
echo -e "${YELLOW}⚡ Bắt đầu thu thập RAM snapshots!${NC}"
echo -e "${YELLOW}👉 NHẤN ENTER ĐỂ KẾT THÚC SỚM${NC}"
echo "═══════════════════════════════════════════════════════"

echo "RAM Debug Session: $(date)" > "${OUT_DIR}/summary.txt"
echo "" >> "${OUT_DIR}/summary.txt"

ACTUAL_TIMES=0
for i in $(seq 1 "$TIMES"); do
    take_snapshot "$i"
    ACTUAL_TIMES=$i
    if [ "$i" -lt "$TIMES" ]; then
        echo -e "   ⏳ Waiting ${INTERVAL}s for next snapshot... (Nhấn ENTER để kết thúc sớm)"
        if read -r -t "$INTERVAL"; then
            echo -e "\n${YELLOW}⏹️ Stop sớm. Chuyển sang phân tích...${NC}"
            break
        fi
    fi
done

TIMES=$ACTUAL_TIMES

# ── Auto Analysis: RAM Growth Trend ──────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}📈 AUTO-ANALYSIS: RAM GROWTH TREND${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"

# Lấy danh sách node IDs đã capture
NODE_IDS=$(find "$OUT_DIR/snap_1" -name "status_node*_1.txt" 2>/dev/null | grep -oP 'node\K[^_]+' | sort -u)

for node_id in $NODE_IDS; do
    echo ""
    echo -e "${MAGENTA}▶ metanode-${node_id} VmRSS (Resident RAM) theo thời gian:${NC}"
    for i in $(seq 1 "$TIMES"); do
        f="${OUT_DIR}/snap_${i}/status_node${node_id}_${i}.txt"
        if [ -f "$f" ]; then
            rss=$(grep -oP 'rss=\K[0-9.]+' "$f" 2>/dev/null || echo 0)
            rss_int=${rss%.*}
            bar=$(printf '█%.0s' $(seq 1 $((rss_int / 50))))
            printf "  Snap #%-2d: %6.0f MB  %s\n" "$i" "$rss" "$bar"
        fi
    done

    echo ""
    echo -e "${MAGENTA}▶ metanode-${node_id} Pss_Anon (Heap thực sự) theo thời gian:${NC}"
    for i in $(seq 1 "$TIMES"); do
        f="${OUT_DIR}/snap_${i}/smaps_rollup_node${node_id}_${i}.txt"
        if [ -f "$f" ]; then
            anon=$(grep -oP 'pss_anon=\K[0-9.]+' "$f" 2>/dev/null || echo 0)
            anon_int=${anon%.*}
            bar=$(printf '█%.0s' $(seq 1 $((anon_int / 50))))
            printf "  Snap #%-2d: %6.0f MB anon  %s\n" "$i" "$anon" "$bar"
        fi
    done
done

# ── Detect RAM growth ─────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}🔍 PHÁT HIỆN MEMORY LEAK${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"

for node_id in $NODE_IDS; do
    echo ""
    echo -e "${MAGENTA}▶ metanode-${node_id}:${NC}"

    f1="${OUT_DIR}/snap_1/status_node${node_id}_1.txt"
    fN="${OUT_DIR}/snap_${TIMES}/status_node${node_id}_${TIMES}.txt"

    if [ -f "$f1" ] && [ -f "$fN" ]; then
        rss_start=$(grep -oP 'rss=\K[0-9.]+' "$f1" | cut -d. -f1)
        rss_end=$(grep -oP 'rss=\K[0-9.]+' "$fN" | cut -d. -f1)
        anon_start=$(grep -oP 'pss_anon=\K[0-9.]+' "${OUT_DIR}/snap_1/smaps_rollup_node${node_id}_1.txt" | cut -d. -f1)
        anon_end=$(grep -oP 'pss_anon=\K[0-9.]+' "${OUT_DIR}/snap_${TIMES}/smaps_rollup_node${node_id}_${TIMES}.txt" | cut -d. -f1)

        rss_delta=$((rss_end - rss_start))
        anon_delta=$((anon_end - anon_start))
        duration=$(( TIMES * INTERVAL ))

        echo "   VmRSS  : ${rss_start} MB → ${rss_end} MB  (Δ = ${rss_delta} MB trong ${duration}s)"
        echo "   Pss_Anon: ${anon_start} MB → ${anon_end} MB  (Δ = ${anon_delta} MB trong ${duration}s)"

        if [ "$rss_delta" -gt 50 ]; then
            echo -e "   ${RED}⚠️  RSS tăng ${rss_delta} MB — khả năng cao có memory leak!${NC}"
        elif [ "$rss_delta" -gt 20 ]; then
            echo -e "   ${YELLOW}⚠️  RSS tăng ${rss_delta} MB — đáng theo dõi${NC}"
        else
            echo -e "   ${GREEN}✅ RSS ổn định (Δ=${rss_delta} MB)${NC}"
        fi

        if [ "$anon_delta" -gt 30 ]; then
            echo -e "   ${RED}⚠️  Heap tăng ${anon_delta} MB — Rust allocator đang giữ bộ nhớ!${NC}"
            echo -e "   ${RED}   → Xem smaps_top để tìm vùng nhớ đang tăng${NC}"
        fi
    fi
done

# ── Top Mappings comparison ───────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}📊 TOP MEMORY MAPPINGS: SNAP 1 vs SNAP ${TIMES}${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"

for node_id in $NODE_IDS; do
    f1="${OUT_DIR}/snap_1/smaps_top_node${node_id}_1.txt"
    fN="${OUT_DIR}/snap_${TIMES}/smaps_top_node${node_id}_${TIMES}.txt"
    if [ -f "$f1" ] && [ -f "$fN" ]; then
        echo ""
        echo -e "${MAGENTA}▶ metanode-${node_id} — Snap #${TIMES} top mappings:${NC}"
        grep -v "^==\|^Ph\|^$" "$fN" | head -15
    fi
done

# ── HTML Dashboard Generator ──────────────────────────────────────────────────
generate_html_report() {
    local html="${OUT_DIR}/report.html"

    # ── Thu thập dữ liệu từ tất cả snapshots ─────────────────────────────────
    # Format: JSON arrays cho Chart.js
    local node_ids
    node_ids=$(find "$OUT_DIR/snap_1" -name "status_node*_1.txt" 2>/dev/null \
               | grep -oP 'node\K[^_]+' | sort -u | tr '\n' ' ')

    # Build labels (snap 1, snap 2, ...)
    local labels="["
    for i in $(seq 1 "$TIMES"); do
        labels+="\"Snap #${i}\","
    done
    labels="${labels%,}]"

    # Build per-node datasets
    local datasets_rss=""
    local datasets_anon=""
    local datasets_threads=""
    local node_colors=("\"#6ee7b7\"" "\"#93c5fd\"" "\"#fca5a5\"" "\"#fde68a\"" "\"#c4b5fd\"")
    local color_idx=0

    for node_id in $node_ids; do
        local color="${node_colors[$color_idx]}"
        color_idx=$(( color_idx + 1 ))

        local rss_arr="["
        local anon_arr="["
        local thr_arr="["

        for i in $(seq 1 "$TIMES"); do
            local sf="${OUT_DIR}/snap_${i}/status_node${node_id}_${i}.txt"
            local rf="${OUT_DIR}/snap_${i}/smaps_rollup_node${node_id}_${i}.txt"
            local rss=0 anon=0 thr=0
            [ -f "$sf" ] && rss=$(grep -oP 'rss=\K[0-9.]+' "$sf" 2>/dev/null || echo 0)
            [ -f "$rf" ] && anon=$(grep -oP 'pss_anon=\K[0-9.]+' "$rf" 2>/dev/null || echo 0)
            [ -f "$sf" ] && thr=$(grep -oP 'threads=\K[0-9]+' "$sf" 2>/dev/null || echo 0)
            rss_arr+="${rss},"
            anon_arr+="${anon},"
            thr_arr+="${thr},"
        done

        rss_arr="${rss_arr%,}]"
        anon_arr="${anon_arr%,}]"
        thr_arr="${thr_arr%,}]"

        datasets_rss+="{label:'node-${node_id} VmRSS',data:${rss_arr},borderColor:${color},backgroundColor:${color}+'33',fill:true,tension:0.3},"
        datasets_anon+="{label:'node-${node_id} Pss_Anon',data:${anon_arr},borderColor:${color},backgroundColor:${color}+'33',fill:true,tension:0.3},"
        datasets_threads+="{label:'node-${node_id} Threads',data:${thr_arr},borderColor:${color},backgroundColor:'transparent',tension:0.3},"
    done

    datasets_rss="${datasets_rss%,}"
    datasets_anon="${datasets_anon%,}"
    datasets_threads="${datasets_threads%,}"

    # ── Build top-mappings table ──────────────────────────────────────────────
    local mappings_html=""
    for node_id in $node_ids; do
        local fN="${OUT_DIR}/snap_${TIMES}/smaps_top_node${node_id}_${TIMES}.txt"
        if [ -f "$fN" ]; then
            mappings_html+="<h3>metanode-${node_id} (Snap #${TIMES})</h3><table>"
            mappings_html+="<tr><th>Private Dirty</th><th>RSS</th><th>Mapping</th></tr>"
            grep -E "MB private_dirty" "$fN" | head -15 | while IFS='|' read -r pd rss name; do
                pd=$(echo "$pd" | xargs)
                rss=$(echo "$rss" | xargs)
                name=$(echo "$name" | xargs)
                mappings_html+="<tr><td>${pd}</td><td><b>${rss}</b></td><td>${name}</td></tr>"
            done
            mappings_html+="</table>"
        fi
    done

    # ── Build summary cards ───────────────────────────────────────────────────
    local cards_html=""
    for node_id in $node_ids; do
        local f1="${OUT_DIR}/snap_1/status_node${node_id}_1.txt"
        local fN="${OUT_DIR}/snap_${TIMES}/status_node${node_id}_${TIMES}.txt"
        local rf1="${OUT_DIR}/snap_1/smaps_rollup_node${node_id}_1.txt"
        local rfN="${OUT_DIR}/snap_${TIMES}/smaps_rollup_node${node_id}_${TIMES}.txt"
        local rss_s=0 rss_e=0 anon_s=0 anon_e=0 thr=0 peak=0
        [ -f "$f1" ] && rss_s=$(grep -oP 'rss=\K[0-9.]+' "$f1" | cut -d. -f1)
        [ -f "$fN" ] && rss_e=$(grep -oP 'rss=\K[0-9.]+' "$fN" | cut -d. -f1)
        [ -f "$rf1" ] && anon_s=$(grep -oP 'pss_anon=\K[0-9.]+' "$rf1" | cut -d. -f1)
        [ -f "$rfN" ] && anon_e=$(grep -oP 'pss_anon=\K[0-9.]+' "$rfN" | cut -d. -f1)
        [ -f "$fN" ] && thr=$(grep -oP 'threads=\K[0-9]+' "$fN")
        [ -f "$fN" ] && peak=$(grep -oP 'peak=\K[0-9.]+' "$fN" | cut -d. -f1)
        local delta=$(( rss_e - rss_s ))
        local adelta=$(( anon_e - anon_s ))
        local status_color="#6ee7b7"; local status_icon="✅"
        [ "$delta" -gt 20 ] && status_color="#fde68a" && status_icon="⚠️"
        [ "$delta" -gt 50 ] && status_color="#fca5a5" && status_icon="🔴"
        cards_html+="<div class='card'>
            <div class='card-header'>${status_icon} metanode-${node_id}</div>
            <div class='stat'><span class='label'>VmRSS (cuối)</span><span class='val'>${rss_e} MB</span></div>
            <div class='stat'><span class='label'>Pss_Anon (heap)</span><span class='val'>${anon_e} MB</span></div>
            <div class='stat'><span class='label'>VmPeak</span><span class='val'>${peak} MB</span></div>
            <div class='stat'><span class='label'>Threads</span><span class='val'>${thr}</span></div>
            <div class='stat delta' style='color:${status_color}'>
                <span class='label'>ΔRSS (snap1→N)</span>
                <span class='val'>${delta:+${delta}} MB</span>
            </div>
            <div class='stat delta' style='color:${status_color}'>
                <span class='label'>ΔHeap</span>
                <span class='val'>${adelta:+${adelta}} MB</span>
            </div>
        </div>"
    done

    local session_ts
    session_ts=$(basename "$OUT_DIR")
    local total_snaps=$TIMES
    local interval_s=$INTERVAL

    # ── Write HTML ────────────────────────────────────────────────────────────
    cat > "$html" << HTMLEOF
<!DOCTYPE html>
<html lang="vi">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>🦀 Rust RAM Profiler — ${session_ts}</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.3/dist/chart.umd.min.js"></script>
<style>
  @import url('https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&family=JetBrains+Mono:wght@400;600&display=swap');
  :root {
    --bg: #0f1117; --surface: #1a1d27; --surface2: #22263a;
    --border: #2a2d3e; --text: #e2e8f0; --muted: #64748b;
    --green: #6ee7b7; --blue: #93c5fd; --red: #fca5a5;
    --yellow: #fde68a; --purple: #c4b5fd;
    --gradient: linear-gradient(135deg, #1a1d27 0%, #0f1117 100%);
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: 'Inter', sans-serif; background: var(--bg); color: var(--text);
         min-height: 100vh; padding: 24px; }
  header { text-align: center; margin-bottom: 32px; padding: 32px;
           background: var(--surface); border-radius: 16px;
           border: 1px solid var(--border);
           background: linear-gradient(135deg, #1a1d27 0%, #0d1117 100%); }
  header h1 { font-size: 2rem; font-weight: 700;
              background: linear-gradient(90deg, var(--green), var(--blue));
              -webkit-background-clip: text; -webkit-text-fill-color: transparent; }
  header p { color: var(--muted); margin-top: 8px; font-family: 'JetBrains Mono', monospace; font-size: 0.85rem; }
  .meta-row { display:flex; gap:16px; justify-content:center; margin-top:16px; flex-wrap:wrap; }
  .meta-pill { background: var(--surface2); border:1px solid var(--border);
               border-radius:999px; padding:4px 14px; font-size:0.8rem; color:var(--muted); }
  .meta-pill span { color: var(--text); font-weight:600; }

  /* Cards */
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
           gap: 16px; margin-bottom: 32px; }
  .card { background: var(--surface); border: 1px solid var(--border);
          border-radius: 12px; padding: 20px;
          transition: transform 0.2s, box-shadow 0.2s; }
  .card:hover { transform: translateY(-2px); box-shadow: 0 8px 24px rgba(0,0,0,0.4); }
  .card-header { font-size: 1.1rem; font-weight: 600; margin-bottom: 16px;
                 padding-bottom: 12px; border-bottom: 1px solid var(--border); }
  .stat { display: flex; justify-content: space-between; align-items: center;
          padding: 6px 0; border-bottom: 1px solid var(--border); }
  .stat:last-child { border-bottom: none; }
  .stat .label { color: var(--muted); font-size: 0.82rem; }
  .stat .val { font-family: 'JetBrains Mono', monospace; font-weight: 600; font-size: 0.9rem; }
  .stat.delta .val { font-size: 1rem; font-weight: 700; }

  /* Charts */
  .charts { display: grid; grid-template-columns: repeat(auto-fit, minmax(500px, 1fr));
            gap: 20px; margin-bottom: 32px; }
  .chart-box { background: var(--surface); border: 1px solid var(--border);
               border-radius: 12px; padding: 20px; }
  .chart-box h2 { font-size: 1rem; font-weight: 600; margin-bottom: 16px;
                  color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }
  canvas { width: 100% !important; }

  /* Mappings table */
  .mappings { background: var(--surface); border: 1px solid var(--border);
              border-radius: 12px; padding: 24px; margin-bottom: 32px; }
  .mappings h2 { font-size: 1rem; font-weight: 600; margin-bottom: 20px;
                 color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .mappings h3 { color: var(--blue); margin: 20px 0 10px; font-size: 0.9rem; }
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { color: var(--muted); font-weight: 500; text-align: left; padding: 8px 12px;
       border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; border-bottom: 1px solid var(--border);
       font-family: 'JetBrains Mono', monospace; }
  tr:hover td { background: var(--surface2); }

  /* Tips */
  .tips { background: var(--surface); border: 1px solid var(--border);
          border-radius: 12px; padding: 24px; }
  .tips h2 { font-size: 1rem; font-weight: 600; margin-bottom: 16px;
             color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .tip { background: var(--surface2); border-left: 3px solid var(--blue);
         border-radius: 0 8px 8px 0; padding: 12px 16px; margin-bottom: 10px; }
  .tip-label { font-weight: 600; color: var(--blue); font-size: 0.85rem; margin-bottom: 4px; }
  .tip code { font-family: 'JetBrains Mono', monospace; font-size: 0.8rem;
              background: var(--bg); padding: 2px 6px; border-radius: 4px; color: var(--green); }

  /* Refresh btn */
  .actions { text-align: center; margin-bottom: 24px; }
  .btn { background: linear-gradient(135deg, var(--green), var(--blue));
         color: #0f1117; font-weight: 700; border: none; padding: 10px 24px;
         border-radius: 8px; cursor: pointer; font-size: 0.9rem;
         transition: opacity 0.2s; text-decoration: none; display: inline-block; }
  .btn:hover { opacity: 0.85; }
</style>
</head>
<body>

<header>
  <h1>🦀 Rust Metanode RAM Profiler</h1>
  <p>Session: ${session_ts}</p>
  <div class="meta-row">
    <div class="meta-pill">Snapshots: <span>${total_snaps}</span></div>
    <div class="meta-pill">Interval: <span>${interval_s}s</span></div>
    <div class="meta-pill">Duration: <span>$(( total_snaps * interval_s ))s</span></div>
    <div class="meta-pill">Source: <span>/proc/PID/smaps_rollup</span></div>
  </div>
</header>

<div class="actions">
  <button class="btn" onclick="location.reload()">🔄 Reload</button>
</div>

<div class="cards">
${cards_html}
</div>

<div class="charts">
  <div class="chart-box">
    <h2>📊 VmRSS (Resident RAM) — MB</h2>
    <canvas id="chartRss"></canvas>
  </div>
  <div class="chart-box">
    <h2>🧠 Pss_Anon (Heap thực sự) — MB</h2>
    <canvas id="chartAnon"></canvas>
  </div>
  <div class="chart-box">
    <h2>🔁 Thread Count</h2>
    <canvas id="chartThreads"></canvas>
  </div>
</div>

<div class="mappings">
  <h2>📋 Top Memory Mappings</h2>
  ${mappings_html}
</div>

<div class="tips">
  <h2>💡 Cách phân tích sâu hơn</h2>
  <div class="tip">
    <div class="tip-label">A. So sánh smaps snap đầu vs cuối</div>
    <code>diff &lt;(grep -v "^=" snap_1/smaps_top_node0_1.txt | sort -rn) &lt;(grep -v "^=" snap_N/smaps_top_node0_N.txt | sort -rn)</code>
  </div>
  <div class="tip">
    <div class="tip-label">B. Heap profiling với heaptrack</div>
    <code>heaptrack ./metanode start --config ... && heaptrack_gui heaptrack.metanode.*.gz</code>
  </div>
  <div class="tip">
    <div class="tip-label">C. jemallocator profiling</div>
    <code>MALLOC_CONF="prof:true,prof_leak:true" ./metanode ...</code>
  </div>
  <div class="tip">
    <div class="tip-label">D. valgrind massif</div>
    <code>valgrind --tool=massif --massif-out-file=massif.out ./metanode ...</code>
  </div>
  <div class="tip">
    <div class="tip-label">E. Tokio console (async task viewer)</div>
    <code>TOKIO_CONSOLE=1 ./metanode ... && tokio-console</code>
  </div>
</div>

<script>
const chartDefaults = {
  responsive: true,
  plugins: { legend: { labels: { color: '#94a3b8', font: { family: 'Inter' } } } },
  scales: {
    x: { ticks: { color: '#64748b' }, grid: { color: '#1e2235' } },
    y: { ticks: { color: '#64748b' }, grid: { color: '#1e2235' } }
  }
};
const labels = ${labels};

new Chart(document.getElementById('chartRss'), {
  type: 'line',
  data: { labels, datasets: [${datasets_rss}] },
  options: { ...chartDefaults, plugins: { ...chartDefaults.plugins,
    title: { display: false } } }
});

new Chart(document.getElementById('chartAnon'), {
  type: 'line',
  data: { labels, datasets: [${datasets_anon}] },
  options: chartDefaults
});

new Chart(document.getElementById('chartThreads'), {
  type: 'line',
  data: { labels, datasets: [${datasets_threads}] },
  options: chartDefaults
});
</script>
</body>
</html>
HTMLEOF

    echo -e "${GREEN}✅ HTML report generated: ${CYAN}${html}${NC}"
}

# ── Generate HTML ─────────────────────────────────────────────────────────────
generate_html_report

HTTP_PORT="${3:-8099}"
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}🌐 MỞ DASHBOARD TRONG BROWSER${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════${NC}"
echo ""
echo -e "  Chạy lệnh sau để serve dashboard:"
echo -e "  ${CYAN}cd ${OUT_DIR} && python3 -m http.server ${HTTP_PORT}${NC}"
echo -e "  Rồi mở: ${YELLOW}http://localhost:${HTTP_PORT}/report.html${NC}"
echo ""

# Tự động serve nếu python3 có sẵn
if command -v python3 &>/dev/null; then
    echo -e "${GREEN}⚡ Tự động mở server tại http://localhost:${HTTP_PORT}/report.html${NC}"
    echo -e "   (Ctrl+C để dừng)"
    echo ""
    cd "$OUT_DIR" && python3 -m http.server "$HTTP_PORT" 2>/dev/null
fi
