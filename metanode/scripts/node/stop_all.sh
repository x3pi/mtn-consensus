#!/bin/bash
# Usage: ./stop_all.sh
# Gracefully stop ALL nodes (0-4)

# NOTE: Do NOT use 'set -e' here — we want to continue even if some kills fail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo ""
echo -e "${GREEN}🛑 Stopping ALL nodes...${NC}"
echo ""

# SIGTERM all Go and Rust processes first (for LevelDB flush)
# NOTE: We intentionally do NOT kill 'ngrok' or any process in protected tmux sessions
echo -e "${YELLOW}📤 Sending SIGTERM to all processes...${NC}"
pkill -f "simple_chain" 2>/dev/null || true
pkill -f "metanode start" 2>/dev/null || true
pkill -f "metanode run" 2>/dev/null || true
pkill -f "tps_blast" 2>/dev/null || true

echo "⏳ Waiting 15s for graceful shutdown (FlushAll)..."
sleep 15

# Kill all tmux sessions EXCEPT those in KEEP_SESSIONS
# Add session names here to protect them from being killed:
KEEP_SESSIONS=("ngrok")

# Auto-detect the current tmux session (if this script is running inside tmux)
# and add it to KEEP_SESSIONS so we don't kill ourselves
if [ -n "$TMUX" ]; then
    CURRENT_SESSION=$(tmux display-message -p '#S' 2>/dev/null || true)
    if [ -n "$CURRENT_SESSION" ]; then
        KEEP_SESSIONS+=("$CURRENT_SESSION")
    fi
fi

echo -e "${YELLOW}🗑️  Cleaning tmux sessions (protected: ${KEEP_SESSIONS[*]})...${NC}"

# Read actual running sessions from tmux ls, kill everything NOT in KEEP_SESSIONS
while IFS= read -r session_line; do
    session_name="${session_line%%:*}"
    [ -z "$session_name" ] && continue

    # Check if this session is protected
    protected=false
    for keep in "${KEEP_SESSIONS[@]}"; do
        if [ "$session_name" = "$keep" ]; then
            protected=true
            break
        fi
    done

    if [ "$protected" = "true" ]; then
        echo -e "${YELLOW}  ⚠️  Skipping protected session: $session_name${NC}"
    else
        echo -e "  🗑️  Killing session: $session_name"
        tmux kill-session -t "$session_name" 2>/dev/null || true
    fi
done < <(tmux ls 2>/dev/null || true)

# Force kill stragglers (explicitly exclude ngrok)
pkill -9 -f "simple_chain" 2>/dev/null || true
pkill -9 -f "metanode start" 2>/dev/null || true
pkill -9 -f "metanode run" 2>/dev/null || true

# Clean all sockets
rm -f /tmp/executor*.sock /tmp/rust-go-*.sock /tmp/metanode-tx-*.sock 2>/dev/null || true

echo ""
echo -e "${GREEN}✅ All nodes stopped.${NC}"
echo ""
