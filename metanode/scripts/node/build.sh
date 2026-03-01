#!/bin/bash
# Usage: ./build.sh
# Build cả Rust metanode và Go simple_chain binaries
# Chạy trước khi dùng run_node.sh hoặc resume_node.sh

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
METANODE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GO_PROJECT_ROOT="$(cd "$METANODE_ROOT/../.." && pwd)/mtn-simple-2025"
GO_SIMPLE_ROOT="$GO_PROJECT_ROOT/cmd/simple_chain"

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🔨 BUILD — Rust Metanode + Go Simple Chain       ${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""

# ─── Rust ─────────────────────────────────────────────────────
echo -e "${BLUE}📋 Step 1: Build Rust metanode...${NC}"
export PATH="/home/abc/protoc3/bin:$PATH"
cd "$METANODE_ROOT" && cargo +nightly build --release --bin metanode
echo -e "${GREEN}  ✅ Rust binary: $METANODE_ROOT/target/release/metanode${NC}"

# ─── Go ───────────────────────────────────────────────────────
echo -e "${BLUE}📋 Step 2: Build Go simple_chain...${NC}"
cd "$GO_SIMPLE_ROOT" && go build -o simple_chain .
echo -e "${GREEN}  ✅ Go binary: $GO_SIMPLE_ROOT/simple_chain${NC}"

# ─── Done ─────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  ✅ BUILD HOÀN TẤT${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo -e "  Bây giờ có thể chạy:"
echo -e "    ${BLUE}./run_node.sh N${NC}      # Fresh start 1 node"
echo -e "    ${BLUE}./resume_node.sh N${NC}   # Resume 1 node"
echo ""
