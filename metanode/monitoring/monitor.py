#!/usr/bin/env python3
"""
mtn-consensus Node Monitor Backend
Collects real-time data from node logs and metrics endpoints
"""

import os
import re
import json
import time
import glob
from datetime import datetime, timedelta
from pathlib import Path
import subprocess
import threading
try:
    from flask import Flask, jsonify
    HAS_FLASK = True
except ImportError:
    HAS_FLASK = False

try:
    import psutil
    HAS_PSUTIL = True
    print("✅ psutil loaded successfully")
except ImportError:
    HAS_PSUTIL = False
    print("⚠️  psutil not available - system metrics will show 'N/A'")

class NodeMonitor:
    def __init__(self, metanode_path):
        self.metanode_path = Path(metanode_path)
        self.log_path = self.metanode_path / "logs" / "latest"
        self.config_path = self.metanode_path / "config"
        self.metrics_data = {}
        self.node_data = {}
        self.last_update = time.time()

        # TPS calculation with moving average
        self.tps_history = []  # List of (timestamp, transactions) tuples
        self.tps_window = 10   # Number of data points for moving average
        self.last_tx_count = 0
        self.last_tx_time = time.time()

    def get_node_configs(self):
        """Get all node configurations"""
        configs = {}
        for config_file in glob.glob(str(self.config_path / "node_*.toml")):
            node_id = int(re.search(r'node_(\d+)\.toml', config_file).group(1))
            configs[node_id] = config_file
        return configs

    def parse_log_file(self, log_file):
        """Parse a single log file for metrics"""
        metrics = {
            'blocks': 0,           # Local block index in epoch (B1, B2, etc.)
            'global_blocks': 0,    # Global execution index (what Rust sends to Go)
            'transactions': 0,
            'epoch': 0,
            'commit_index': 0,
            'errors': 0,
            'warnings': 0,
            'last_activity': 0,
            'status': 'unknown'
        }

        try:
            with open(log_file, 'r', encoding='utf-8', errors='ignore') as f:
                lines = f.readlines()[-1000:]  # Read last 1000 lines for performance

                for line in lines:
                    # Extract epoch info
                    epoch_match = re.search(r'epoch=(\d+)', line)
                    if epoch_match:
                        metrics['epoch'] = int(epoch_match.group(1))

                    # Extract block info
                    block_match = re.search(r'Created block B(\d+)', line)
                    if block_match:
                        metrics['blocks'] = int(block_match.group(1))
                        metrics['last_activity'] = time.time()

                    # Extract commit index
                    commit_match = re.search(r'commit_index=(\d+)', line)
                    if commit_match:
                        metrics['commit_index'] = int(commit_match.group(1))

                    # Extract global execution index (what Rust sends to Go)
                    global_match = re.search(r'global_exec_index=(\d+)', line)
                    if global_match:
                        metrics['global_blocks'] = int(global_match.group(1))

                    # Alternative: extract from executor send logs
                    executor_match = re.search(r'Sent committed sub-DAG to Go executor: global_exec_index=(\d+)', line)
                    if executor_match:
                        metrics['global_blocks'] = int(executor_match.group(1))

                    # Extract transactions from commit execution logs
                    # Pattern: "X blocks, Y total transactions" or "X blocks, Y transactions"
                    tx_match = re.search(r'(\d+) blocks, (\d+) total transactions', line)
                    if not tx_match:
                        tx_match = re.search(r'(\d+) blocks, (\d+) transactions', line)
                    if tx_match:
                        blocks_in_commit = int(tx_match.group(1))
                        tx_in_commit = int(tx_match.group(2))
                        metrics['transactions'] += tx_in_commit

                    # Count errors and warnings
                    if '[ERROR]' in line or 'ERROR' in line:
                        metrics['errors'] += 1
                    if '[WARN]' in line or 'WARN' in line:
                        metrics['warnings'] += 1

                    # Check for recent activity (within 30 seconds)
                    try:
                        # Extract timestamp from log line
                        timestamp_match = re.search(r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})', line)
                        if timestamp_match:
                            log_time = datetime.fromisoformat(timestamp_match.group(1).replace('Z', '+00:00'))
                            if (datetime.now() - log_time).total_seconds() < 30:
                                metrics['status'] = 'healthy'
                                metrics['last_activity'] = log_time.timestamp()
                    except:
                        pass

                # If no recent activity, mark as warning
                if metrics['last_activity'] == 0 or (time.time() - metrics['last_activity']) > 60:
                    metrics['status'] = 'warning'
                elif metrics['errors'] > 0:
                    metrics['status'] = 'error'
                else:
                    metrics['status'] = 'healthy'

        except Exception as e:
            print(f"Error parsing log file {log_file}: {e}")
            metrics['status'] = 'error'

        return metrics

    def update_tps_calculation(self, current_tx_count):
        """Update TPS calculation with moving average"""
        current_time = time.time()
        time_diff = current_time - self.last_tx_time

        # Only calculate if we have meaningful time difference (> 0.1 seconds)
        # and transaction increase
        if time_diff >= 0.1 and current_tx_count > self.last_tx_count:
            # Calculate instantaneous TPS
            tx_diff = current_tx_count - self.last_tx_count
            instantaneous_tps = tx_diff / time_diff

            # Cap TPS at reasonable maximum (e.g., 1000 TPS) to avoid spikes
            instantaneous_tps = min(instantaneous_tps, 1000.0)

            # Add to history
            self.tps_history.append((current_time, instantaneous_tps))

            # Keep only recent history (last 60 seconds)
            cutoff_time = current_time - 60
            self.tps_history = [(t, tps) for t, tps in self.tps_history if t > cutoff_time]

            # Update tracking
            self.last_tx_count = current_tx_count
            self.last_tx_time = current_time
        elif current_tx_count > self.last_tx_count:
            # If time diff is too small but we have tx increase, just update counters
            self.last_tx_count = current_tx_count
            self.last_tx_time = current_time

    def get_calculated_tps(self):
        """Get TPS using moving average"""
        if len(self.tps_history) < 2:
            return 0.0

        # Use exponential moving average for smoothness
        if len(self.tps_history) >= self.tps_window:
            # Use last N measurements for moving average
            recent_tps = [tps for _, tps in self.tps_history[-self.tps_window:]]
            return sum(recent_tps) / len(recent_tps)
        else:
            # Use all available data
            recent_tps = [tps for _, tps in self.tps_history]
            return sum(recent_tps) / len(recent_tps)

    def get_system_metrics(self):
        """Get system-level metrics"""
        if not HAS_PSUTIL:
            return {
                'cpu_usage': 'N/A (install psutil)',
                'memory_usage': 'N/A (install psutil)',
                'memory_percent': 'N/A (install psutil)'
            }

        try:
            cpu_percent = psutil.cpu_percent(interval=0.1)
            memory = psutil.virtual_memory()
            return {
                'cpu_usage': f"{cpu_percent:.1f}%",
                'memory_usage': f"{memory.used // (1024*1024)}MB",
                'memory_percent': f"{memory.percent:.1f}%"
            }
        except Exception as e:
            return {
                'cpu_usage': f'Error: {e}',
                'memory_usage': f'Error: {e}',
                'memory_percent': f'Error: {e}'
            }

    def collect_node_data(self):
        """Collect data from all nodes"""
        configs = self.get_node_configs()
        node_data = {}

        for node_id, config_file in configs.items():
            log_file = self.log_path / f"node_{node_id}.log"
            metrics = self.parse_log_file(log_file)

            node_data[node_id] = {
                'id': node_id,
                'status': metrics['status'],
                'epoch': metrics['epoch'],
                'blocks': metrics['blocks'],           # Local epoch block index
                'global_blocks': metrics['global_blocks'], # Global execution index
                'transactions': metrics['transactions'],   # Total transactions processed
                'commit_index': metrics['commit_index'],
                'errors': metrics['errors'],
                'warnings': metrics['warnings'],
                'last_activity': metrics['last_activity'],
                'config_file': config_file
            }

        return node_data

    def collect_epoch_data(self):
        """Collect epoch-related data"""
        epoch_data = {
            'current_epoch': 0,
            'epoch_start_timestamp': 0,
            'elapsed_seconds': 0,
            'remaining_seconds': 0,
            'epoch_duration': 180  # Default 3 minutes
        }

        # First, try to read current epoch from log files (most accurate)
        max_epoch = 0
        for log_file in glob.glob(str(self.log_path / "node_*.log")):
            try:
                with open(log_file, 'r', encoding='utf-8', errors='ignore') as f:
                    # Read last 1000 lines to find latest epoch
                    lines = f.readlines()[-1000:]

                    for line in reversed(lines):  # Start from most recent
                        # Look for epoch in various log patterns
                        epoch_match = re.search(r'epoch=(\d+)', line)
                        if epoch_match:
                            current_epoch = int(epoch_match.group(1))
                            max_epoch = max(max_epoch, current_epoch)
                            break  # Found latest epoch in this file

            except Exception as e:
                print(f"Error reading epoch from {log_file}: {e}")

        if max_epoch > 0:
            epoch_data['current_epoch'] = max_epoch

        # Try to read from genesis.json for base timestamp
        genesis_timestamp = 0
        try:
            genesis_file = Path(self.metanode_path).parent / "mtn-simple-2025" / "cmd" / "simple_chain" / "genesis.json"
            if genesis_file.exists():
                with open(genesis_file, 'r') as f:
                    genesis = json.load(f)
                    genesis_timestamp = genesis.get('config', {}).get('epoch_timestamp_ms', 0) // 1000
        except Exception as e:
            print(f"Error reading genesis.json: {e}")

        # Estimate epoch start timestamp based on epoch progression
        # Each epoch transition happens every epoch_duration seconds
        if genesis_timestamp > 0 and epoch_data['current_epoch'] > 0:
            # Calculate approximate start time for current epoch
            epoch_transitions = epoch_data['current_epoch']  # Number of transitions from epoch 0
            estimated_start = genesis_timestamp + (epoch_transitions * epoch_data['epoch_duration'])
            epoch_data['epoch_start_timestamp'] = estimated_start

        # Calculate elapsed time
        if epoch_data['epoch_start_timestamp'] > 0:
            epoch_data['elapsed_seconds'] = int(time.time() - epoch_data['epoch_start_timestamp'])
            epoch_data['remaining_seconds'] = max(0, epoch_data['epoch_duration'] - epoch_data['elapsed_seconds'])
        else:
            # If no timestamp available, just show current epoch without time calculations
            epoch_data['elapsed_seconds'] = 0
            epoch_data['remaining_seconds'] = epoch_data['epoch_duration']

        return epoch_data

    def collect_logs(self, max_entries=50):
        """Collect recent log entries"""
        logs = []

        for log_file in glob.glob(str(self.log_path / "node_*.log")):
            try:
                with open(log_file, 'r', encoding='utf-8', errors='ignore') as f:
                    lines = f.readlines()[-100:]  # Last 100 lines from each file

                    for line in lines:
                        # Extract timestamp and level
                        timestamp_match = re.search(r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})', line)
                        if timestamp_match:
                            timestamp = datetime.fromisoformat(timestamp_match.group(1).replace('Z', '+00:00')).timestamp()
                        else:
                            timestamp = time.time()

                        level = 'info'
                        if '[ERROR]' in line or 'ERROR' in line:
                            level = 'error'
                        elif '[WARN]' in line or 'WARN' in line:
                            level = 'warn'

                        # Extract node ID from filename (handle both .log and .epoch.log)
                        node_match = re.search(r'node_(\d+)\.', log_file)
                        node_id = node_match.group(1) if node_match else 'unknown'

                        logs.append({
                            'timestamp': timestamp,
                            'level': level,
                            'message': line.strip(),
                            'node': node_id
                        })
            except Exception as e:
                print(f"Error reading log file {log_file}: {e}")

        # Sort by timestamp and return most recent
        logs.sort(key=lambda x: x['timestamp'], reverse=True)
        return logs[:max_entries]

    def get_dashboard_data(self):
        """Get all data for dashboard"""
        node_data = self.collect_node_data()
        epoch_data = self.collect_epoch_data()
        system_metrics = self.get_system_metrics()
        logs = self.collect_logs()

        # Calculate aggregates
        total_nodes = len(node_data)
        healthy_nodes = sum(1 for node in node_data.values() if node['status'] == 'healthy')
        latest_block = max((node['global_blocks'] for node in node_data.values()), default=0)
        latest_local_block = max((node['blocks'] for node in node_data.values()), default=0)
        total_errors = sum(node['errors'] for node in node_data.values())
        total_warnings = sum(node['warnings'] for node in node_data.values())

        # Calculate TPS with real transaction data
        total_transactions = sum(node['transactions'] for node in node_data.values())
        self.update_tps_calculation(total_transactions)
        calculated_tps = self.get_calculated_tps()

        return {
            'timestamp': time.time(),
            'nodes': list(node_data.values()),
            'epoch': epoch_data,
            'system': system_metrics,
            'logs': logs,
            'aggregates': {
                'total_nodes': total_nodes,
                'healthy_nodes': healthy_nodes,
                'latest_block': latest_block,           # Global execution index (what matters)
                'latest_local_block': latest_local_block, # Local epoch block index
                'total_errors': total_errors,
                'total_warnings': total_warnings,
                'total_transactions': total_transactions,
                'transactions_per_second': calculated_tps,
                'network_health': 'healthy' if healthy_nodes >= total_nodes * 0.75 else 'warning'
            }
        }

class MonitoringServer:
    def __init__(self, monitor, port=8080):
        self.monitor = monitor
        self.port = port

    def create_flask_app(self):
        """Create Flask application"""
        monitor = self.monitor

        if not HAS_FLASK:
            print("❌ Flask not available. Please install Flask: pip install flask")
            return None

        app = Flask(__name__)

        @app.route('/')
        @app.route('/dashboard')
        def dashboard():
            dashboard_path = monitor.metanode_path / "monitoring" / "dashboard.html"
            try:
                with open(dashboard_path, 'r', encoding='utf-8') as f:
                    content = f.read()
                return content
            except FileNotFoundError:
                return "<h1>Dashboard not found</h1><p>Please ensure dashboard.html exists in the monitoring directory.</p>"

        @app.route('/api/data')
        def api_data():
            data = monitor.get_dashboard_data()
            response = jsonify(data)
            response.headers.add('Access-Control-Allow-Origin', '*')
            return response

        return app

    def start(self):
        """Start the monitoring server"""
        app = self.create_flask_app()
        if app is None:
            return

        # Try to start server, with fallback ports if port is busy
        ports_to_try = [self.port, 8081, 8082, 8083, 8084]

        for port in ports_to_try:
            try:
                print(f"🔍 Trying to bind to port {port}...")
                self.port = port  # Update actual port
                print(f"✅ Selected port {port}")
                break
            except Exception as e:
                print(f"⚠️  Error with port {port}: {e}, trying next port...")
                continue

        print(f"🚀 Monitoring server started on http://localhost:{self.port}")
        print(f"📊 Dashboard: http://localhost:{self.port}/dashboard")
        print(f"🔌 API: http://localhost:{self.port}/api/data")
        print("Press Ctrl+C to stop...")

        try:
            app.run(host='0.0.0.0', port=self.port, debug=False, use_reloader=False)
        except KeyboardInterrupt:
            print("\n👋 Monitoring server stopped")

def main():
    # Detect metanode path
    script_dir = Path(__file__).parent.parent
    metanode_path = script_dir

    print("🔍 mtn-consensus Node Monitor")
    print(f"📁 Monitoring path: {metanode_path}")

    # Create monitor
    monitor = NodeMonitor(metanode_path)

    # Test data collection
    print("📊 Testing data collection...")
    data = monitor.get_dashboard_data()
    print(f"✅ Found {data['aggregates']['total_nodes']} nodes")
    print(f"📦 Latest block: {data['aggregates']['latest_block']}")
    print(f"❤️  Healthy nodes: {data['aggregates']['healthy_nodes']}")

    # Start server
    server = MonitoringServer(monitor, port=8080)
    server.start()

if __name__ == "__main__":
    main()
