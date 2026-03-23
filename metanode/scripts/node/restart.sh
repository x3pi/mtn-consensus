#!/bin/bash
# Usage: ./restart.sh
# Safely restart all nodes in a SEPARATE tmux session,
# so your current session (e.g. ngrok) is NOT killed.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SESSION="restart-nodes"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Kill old restart session if exists
tmux kill-session -t "$SESSION" 2>/dev/null || true

echo -e "${GREEN}🚀 Launching run_all.sh in detached tmux session: $SESSION${NC}"
echo -e "${YELLOW}   To follow logs: tmux attach -t $SESSION${NC}"
echo ""

tmux new-session -d -s "$SESSION" bash -c "
    cd '$SCRIPT_DIR'
    ./run_all.sh
    echo ''
    echo '=== run_all.sh completed. Press any key to close ==='
    read -n1
"

echo -e "${GREEN}✅ Started. Attach with:${NC}  tmux attach -t $SESSION"
