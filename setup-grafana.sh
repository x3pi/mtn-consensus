#!/bin/bash
set -e

echo "Setting up Data Source..."
cat <<EOF > /etc/grafana/provisioning/datasources/prometheus.yaml
apiVersion: 1

datasources:
  - name: Prometheus
    type: prometheus
    access: proxy
    url: http://localhost:9090
    isDefault: true
EOF

echo "Setting up Dashboards..."
cat <<EOF > /etc/grafana/provisioning/dashboards/dashboards.yaml
apiVersion: 1

providers:
  - name: "MetaNode Dashboards"
    folder: ""
    type: file
    disableDeletion: false
    updateIntervalSeconds: 10
    options:
      path: /var/lib/grafana/dashboards
EOF

mkdir -p /var/lib/grafana/dashboards
cp /home/abc/chain-n/mtn-simple-2025/deploy/grafana/metanode-dashboard.json /var/lib/grafana/dashboards/
chown -R grafana:grafana /var/lib/grafana/dashboards

echo "Restarting Grafana..."
systemctl restart grafana-server
echo "Done! Refresh your Grafana page."
