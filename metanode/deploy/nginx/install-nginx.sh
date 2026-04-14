#!/bin/bash
# install-nginx.sh — Install Nginx and deploy MetaNode Security Proxy
# Usage: sudo ./install-nginx.sh

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

if [ "$EUID" -ne 0 ]; then
  echo -e "${RED}Please run as root (sudo)${NC}"
  exit 1
fi

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_FILE="$DEPLOY_DIR/metanode.conf"

echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  🛡️  Deploying Nginx Security Hardening for MetaNode RPC  ${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"

# 1. Install Nginx
if ! command -v nginx &> /dev/null; then
    echo -e "${YELLOW}📋 Nginx not found. Installing via apt...${NC}"
    apt-get update -qq
    apt-get install -y nginx
else
    echo -e "${GREEN}✅ Nginx is already installed.$(nginx -v 2>&1)${NC}"
fi

# 2. Deploy Configuration
if [ ! -f "$CONF_FILE" ]; then
    echo -e "${RED}⛔ Error: Configuration file not found at $CONF_FILE${NC}"
    exit 1
fi

echo -e "${YELLOW}📋 Copying metanode.conf to /etc/nginx/sites-available/...${NC}"
cp "$CONF_FILE" /etc/nginx/sites-available/metanode.conf

# Remove default site if it exists to avoid port conflicts (optional but recommended)
if [ -f "/etc/nginx/sites-enabled/default" ]; then
    echo -e "${YELLOW}📋 Disabling default Nginx site...${NC}"
    rm -f /etc/nginx/sites-enabled/default
fi

# Create symlink
if [ ! -f "/etc/nginx/sites-enabled/metanode.conf" ]; then
    echo -e "${YELLOW}📋 Enabling metanode.conf...${NC}"
    ln -s /etc/nginx/sites-available/metanode.conf /etc/nginx/sites-enabled/metanode.conf
fi

# 3. Test & Restart
echo -e "${YELLOW}📋 Testing Nginx configuration...${NC}"
nginx -t

echo -e "${YELLOW}📋 Restarting Nginx service...${NC}"
systemctl restart nginx
systemctl enable nginx

echo -e "${GREEN}✅ Nginx Security Hardening deployed successfully!${NC}"
echo "    Main Public RPC   : http://<server_ip>:8545"
echo "    Protected Target  : http://127.0.0.1:8646 (Go Sub Node)"
echo "    Limits Configured : 20 tx/s per IP, 10 conn/IP, 5MB Max Body"
