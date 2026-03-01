#!/bin/bash
# Script to open necessary ports for mtn-consensus Metanode & Simple Chain communication
# Usage: sudo ./setup_firewall.sh

if [[ $EUID -ne 0 ]]; then
   echo "This script must be run as root (sudo)"
   exit 1
fi

echo "🔧 Configuring firewall rules for mtn-consensus cluster..."

# 1. Rust Metanode Ports
echo "  - Opening Rust P2P & Peer RPC (9000-9004, 19000-19004)..."
ufw allow 9000:9004/tcp
ufw allow 19000:19004/tcp
ufw allow 9100:9104/tcp # Metrics (Optional but helpful)

# 2. Go Master Ports
echo "  - Opening Go Master Connections (4201, 6201, 8201, 10201, 12201)..."
# Each node increments by 2000 for connection_address
ufw allow 4201/tcp
ufw allow 6201/tcp
ufw allow 8201/tcp
ufw allow 10201/tcp
ufw allow 12201/tcp

echo "  - Opening Go Master RPC (8747, 10747, 12747, 14747, 16747)..."
ufw allow 8747/tcp
ufw allow 10747/tcp
ufw allow 12747/tcp
ufw allow 14747/tcp
ufw allow 16747/tcp

echo "  - Opening Go Master Database Internal (6001:6009, 8001:8009, ...)..."
ufw allow 6001:6009/tcp
ufw allow 8001:8009/tcp
ufw allow 10001:10009/tcp
ufw allow 12001:12009/tcp
ufw allow 14001:14009/tcp

echo "  - Opening Go Master Peer RPC (19200:19204)..."
ufw allow 19200:19204/tcp

# 3. Go Sub Ports
echo "  - Opening Go Sub Connections & RPC (4200, 8646, etc.)..."
ufw allow 4200/tcp
ufw allow 6200/tcp # (If Node 1 uses 6200)
ufw allow 8646/tcp
ufw allow 10646/tcp

# 4. SSH (Ensure we don't lock ourselves out)
echo "  - Ensuring SSH is allowed..."
ufw allow ssh

echo ""
echo "🚀 Firewall rules updated!"
echo "Current status:"
ufw status verbose
