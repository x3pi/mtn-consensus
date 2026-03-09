#!/bin/bash
# install-services.sh — Install and enable metanode systemd services
# Usage: sudo ./install-services.sh [num_nodes] (default: 5)
#
# This script:
# 1. Copies service templates to /etc/systemd/system/
# 2. Enables services for each node
# 3. Optionally starts them
#
# ⚠️  Run with sudo

set -e

NUM_NODES=${1:-5}
DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🔧 Installing Metanode systemd services ($NUM_NODES nodes)  ${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# 1. Copy template service files
echo -e "${YELLOW}📋 Copying service templates...${NC}"
cp "$DEPLOY_DIR/metanode-go-master@.service" /etc/systemd/system/
cp "$DEPLOY_DIR/metanode-go-sub@.service" /etc/systemd/system/
cp "$DEPLOY_DIR/metanode-rust@.service" /etc/systemd/system/

# 2. Reload systemd
systemctl daemon-reload

# 3. Enable services for each node
for i in $(seq 0 $((NUM_NODES - 1))); do
    echo -e "${YELLOW}  Node $i: enabling services...${NC}"
    systemctl enable metanode-go-master@${i}.service
    systemctl enable metanode-go-sub@${i}.service
    systemctl enable metanode-rust@${i}.service
done

echo ""
echo -e "${GREEN}✅ Services installed and enabled.${NC}"
echo ""
echo "To start all nodes:"
echo "  for i in \$(seq 0 $((NUM_NODES - 1))); do"
echo "    sudo systemctl start metanode-go-master@\${i}"
echo "    sleep 2"
echo "    sudo systemctl start metanode-go-sub@\${i}"
echo "    sleep 1"
echo "    sudo systemctl start metanode-rust@\${i}"
echo "  done"
echo ""
echo "To check status:"
echo "  systemctl status metanode-go-master@0"
echo "  systemctl status metanode-rust@0"
echo ""
echo "To view logs:"
echo "  journalctl -u metanode-go-master@0 -f"
echo ""
