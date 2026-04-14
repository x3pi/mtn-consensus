#!/bin/bash
# install-services.sh — Install and enable metanode systemd services
# Usage: sudo ./install-services.sh [num_nodes] [go_mem_limit]
#   go_mem_limit: Default 4GiB.

set -e

NUM_NODES=${1:-5}
GOMEMLIMIT=${2:-"4GiB"}
DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Find Rust consensus dir and Go simple chain dir based on standard layout
RUST_WORKDIR="$(dirname "$DEPLOY_DIR")" 
GO_WORKDIR="$(cd "$RUST_WORKDIR/../../mtn-simple-2025" && pwd)"

# Get the actual user calling sudo
ACTUAL_USER="${SUDO_USER:-$(whoami)}"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

if [ "$EUID" -ne 0 ]; then
  echo -e "${RED}Please run as root (sudo)${NC}"
  exit 1
fi

echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🔧 Installing Metanode systemd services ($NUM_NODES nodes)  ${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"

# Check system memory
TOTAL_RAM_MB=$(free -m | awk '/^Mem:/{print $2}')
if [ "$TOTAL_RAM_MB" -lt 15000 ]; then
    echo -e "${RED}⚠️  WARNING: System memory is $TOTAL_RAM_MB MB. Production nodes (Rust NOMT + Go) require at least 16GB RAM to avoid OOM kills!${NC}"
else
    echo -e "${GREEN}✅ System memory looks good: $TOTAL_RAM_MB MB${NC}"
fi

echo -e "${YELLOW}📋 Processing service templates...${NC}"
echo "    User: $ACTUAL_USER"
echo "    Rust Dir: $RUST_WORKDIR"
echo "    Go Dir: $GO_WORKDIR"

# Process templates with sed
for svc in metanode-go-master@.service metanode-go-sub@.service metanode-rust@.service; do
    sed -e "s|{{USER}}|$ACTUAL_USER|g" \
        -e "s|{{RUST_WORKDIR}}|$RUST_WORKDIR|g" \
        -e "s|{{GO_WORKDIR}}|$GO_WORKDIR|g" \
        -e "s|{{GOMEMLIMIT}}|$GOMEMLIMIT|g" \
        "$DEPLOY_DIR/$svc" > "/etc/systemd/system/$svc"
done

# Install logrotate
if [ -f "$DEPLOY_DIR/config-templates/metanode-logrotate.conf" ]; then
    echo -e "${YELLOW}📋 Installing logrotate configuration...${NC}"
    sed -e "s|{{RUST_WORKDIR}}|$RUST_WORKDIR|g" \
        -e "s|{{USER}}|$ACTUAL_USER|g" \
        "$DEPLOY_DIR/config-templates/metanode-logrotate.conf" > "/etc/logrotate.d/metanode"
    chmod 644 /etc/logrotate.d/metanode
fi

# Reload systemd
systemctl daemon-reload

# Enable services for each node
for i in $(seq 0 $((NUM_NODES - 1))); do
    echo -e "${YELLOW}  Node $i: enabling services...${NC}"
    systemctl enable metanode-go-master@${i}.service
    systemctl enable metanode-go-sub@${i}.service
    systemctl enable metanode-rust@${i}.service
done

echo -e "${GREEN}✅ Services installed and enabled.${NC}"
echo "To start all nodes:"
echo "  sudo mtn-ops start 0"
echo ""
echo "Or use the standard loop:"
echo "  for i in \$(seq 0 $((NUM_NODES - 1))); do"
echo "    sudo systemctl start metanode-go-master@\${i}"
echo "    sleep 2"
echo "    sudo systemctl start metanode-go-sub@\${i}"
echo "    sleep 1"
echo "    sudo systemctl start metanode-rust@\${i}"
echo "  done"

# Install mtn-ops CLI globally
if [ -f "$DEPLOY_DIR/mtn-ops.sh" ]; then
    echo -e "${YELLOW}📋 Installing mtn-ops CLI to /usr/local/bin...${NC}"
    ln -sf "$DEPLOY_DIR/mtn-ops.sh" /usr/local/bin/mtn-ops
    chmod +x "$DEPLOY_DIR/mtn-ops.sh"
    echo -e "${GREEN}✅ mtn-ops installed! You can now run 'sudo mtn-ops help' anywhere.${NC}"
fi
