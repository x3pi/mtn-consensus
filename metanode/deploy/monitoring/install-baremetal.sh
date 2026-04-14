#!/bin/bash
# install-baremetal.sh
# Systemd/Bare-metal setup for Prometheus & Grafana without Docker
# Works on Debian/Ubuntu systems
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

if [ "$EUID" -ne 0 ]; then
  echo -e "${RED}Please run as root (sudo)${NC}"
  exit 1
fi

echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  📊 Deploying Prometheus & Grafana (Bare Metal / Native)${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"

# ==========================================
# 1. INSTALL PROMETHEUS
# ==========================================
PROM_VERSION="2.51.1" # Standard stable version
echo -e "${YELLOW}📋 Installing Prometheus...${NC}"

# Stop any running instances to avoid "Text file busy" errors
systemctl stop prometheus 2>/dev/null || true
systemctl stop grafana-server 2>/dev/null || true

# Create Prometheus system user and directories
useradd --no-create-home --shell /bin/false prometheus || true
mkdir -p /etc/prometheus
mkdir -p /var/lib/prometheus
chown -R prometheus:prometheus /etc/prometheus /var/lib/prometheus

# Download & Extract
cd /tmp
wget -q "https://github.com/prometheus/prometheus/releases/download/v${PROM_VERSION}/prometheus-${PROM_VERSION}.linux-amd64.tar.gz"
tar -xf "prometheus-${PROM_VERSION}.linux-amd64.tar.gz"

# Move binaries
cp "prometheus-${PROM_VERSION}.linux-amd64/prometheus" /usr/local/bin/
cp "prometheus-${PROM_VERSION}.linux-amd64/promtool" /usr/local/bin/
chown prometheus:prometheus /usr/local/bin/prometheus /usr/local/bin/promtool

# Move UI libraries
cp -r "prometheus-${PROM_VERSION}.linux-amd64/consoles" /etc/prometheus
cp -r "prometheus-${PROM_VERSION}.linux-amd64/console_libraries" /etc/prometheus
chown -R prometheus:prometheus /etc/prometheus/consoles /etc/prometheus/console_libraries

# Set up Config
if [ -f "$SCRIPT_DIR/prometheus.yml" ]; then
    cp "$SCRIPT_DIR/prometheus.yml" /etc/prometheus/prometheus.yml
else
    # Fallback default configuration
    cat <<EOF > /etc/prometheus/prometheus.yml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: "prometheus"
    static_configs:
      - targets: ["localhost:9090"]

  - job_name: "metanode-rust-consensus"
    static_configs:
      - targets: ["localhost:9100", "localhost:9101", "localhost:9102", "localhost:9103", "localhost:9104"]
EOF
fi
chown prometheus:prometheus /etc/prometheus/prometheus.yml

# Setup Systemd Service
cat <<EOF > /etc/systemd/system/prometheus.service
[Unit]
Description=Prometheus Time Series Collection and Processing Server
Wants=network-online.target
After=network-online.target

[Service]
User=prometheus
Group=prometheus
Type=simple
ExecStart=/usr/local/bin/prometheus \
    --config.file /etc/prometheus/prometheus.yml \
    --storage.tsdb.path /var/lib/prometheus/ \
    --web.console.templates=/etc/prometheus/consoles \
    --web.console.libraries=/etc/prometheus/console_libraries

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable prometheus
systemctl start prometheus

# ==========================================
# 2. INSTALL GRAFANA
# ==========================================
echo -e "${YELLOW}📋 Installing Grafana...${NC}"

# Clean up broken installations from previous failed runs
rm -f /etc/apt/sources.list.d/grafana.list
rm -f /etc/apt/keyrings/grafana.gpg

apt-get update -qq
apt-get install -y apt-transport-https software-properties-common wget

mkdir -p /etc/apt/keyrings/
wget -q -O - https://apt.grafana.com/gpg.key | gpg --dearmor --batch --yes -o /etc/apt/keyrings/grafana.gpg
echo "deb [signed-by=/etc/apt/keyrings/grafana.gpg] https://apt.grafana.com stable main" | tee /etc/apt/sources.list.d/grafana.list > /dev/null

apt-get update -qq
apt-get install -y grafana

systemctl enable grafana-server
systemctl start grafana-server

echo -e "${GREEN}✅ Installation Complete!${NC}"
echo "    Prometheus URL : http://<server_ip>:9090"
echo "    Grafana URL    : http://<server_ip>:3000 (Default login: admin / admin)"
echo "    Next steps: Login to Grafana, add Prometheus (http://localhost:9090) as Data Source, and create Dashboards."
