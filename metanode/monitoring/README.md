# mtn-consensus Node Monitoring System

Hệ thống monitoring thời gian thực cho các node blockchain mtn-consensus.

## 📁 Cấu trúc

```
monitoring/
├── dashboard.html    # Frontend dashboard
├── monitor.py        # Backend monitoring server
├── start_monitor.sh  # Startup script
└── README.md         # Documentation này
```

## 🚀 Khởi động Monitoring

### 1. Cài đặt dependencies (Python)

Script sẽ tự động tạo virtual environment và install Flask + psutil:

```bash
# Từ thư mục monitoring
./start_monitor.sh

# Từ bất kỳ thư mục nào (tự động tìm metanode directory)
/path/to/monitoring/start_monitor.sh

# Production mode (default - khuyến nghị)
./start_monitor.sh --production
# hoặc ngắn gọn
./start_monitor.sh -p

# Development mode (cho debugging)
./start_monitor.sh --development
# hoặc ngắn gọn
./start_monitor.sh -d

# Chạy trên port tùy chỉnh
./start_monitor.sh --port 3000

# Chỉ định đường dẫn metanode cụ thể
./start_monitor.sh --path /custom/path/to/metanode

# Kết hợp các options
./start_monitor.sh --development --port 3000 --path /custom/metanode

# Tạo alias để chạy từ bất kỳ đâu
echo 'alias mysticeti-monitor="/path/to/monitoring/start_monitor.sh"' >> ~/.bashrc
source ~/.bashrc
mysticeti-monitor --help

# Hoặc thêm vào PATH
sudo ln -s /path/to/monitoring/start_monitor.sh /usr/local/bin/mysticeti-monitor
mysticeti-monitor --production
```

**Production Mode:**
- Sử dụng Gunicorn WSGI server
- Multiple workers cho performance tốt hơn
- Không có warning về development server
- Phù hợp cho production deployment

**Development Mode:**
- Sử dụng Flask development server
- Auto-reload khi code thay đổi
- Debug logging chi tiết
- Chỉ dùng cho development/debugging

Hoặc manual setup:

```bash
# Tạo virtual environment
python3 -m venv venv

# Activate và install
source venv/bin/activate
pip install flask psutil
```

### 2. Chạy monitoring server

```bash
cd /home/abc/chain-n/mtn-consensus/metanode/monitoring
chmod +x start_monitor.sh
./start_monitor.sh
```

Hoặc chạy trực tiếp:

```bash
cd /home/abc/chain-n/mtn-consensus/metanode
python3 monitoring/monitor.py
```

### 3. Truy cập Dashboard

- **Dashboard**: http://localhost:8080/dashboard
- **API Data**: http://localhost:8080/api/data

## 📊 Các Metrics được theo dõi

### System Metrics
- CPU Usage ✅
- Memory Usage ✅
- Network Health
- Active Connections

### Epoch Tracking
- Current Epoch ✅ (read from logs)
- Epoch Progress ✅ (estimated timing)
- Epoch Transitions ✅ (real-time updates)

### Transaction Monitoring
- Real TPS Calculation ✅ (from actual transaction logs)
- Moving Average TPS ✅ (60-second window)
- Total Transactions ✅ (cumulative count)

### Portable Deployment
- Run from any directory ✅
- Auto-detect metanode path ✅
- Custom path configuration ✅
- Environment variable support ✅

### Node Metrics
- Node Status (Healthy/Warning/Error)
- Current Epoch
- **Latest Global Block**: Global execution index (blocks sent to Go)
- **Latest Local Block**: Local epoch block index (B1, B2, etc.)
- Commit Index
- Error/Warning Count
- Active Connections

### Epoch Progress
- Current Epoch
- Epoch Duration (180s default)
- Elapsed Time
- Remaining Time
- Progress Bar

### Performance
- Blocks per Second
- Transactions per Second
- CPU/Memory Usage ✅ (enabled with virtual environment)
- Production WSGI Server ✅ (Gunicorn)

### Logs
- Real-time log streaming
- Error/Warning highlighting
- Node-specific logs

## 🔌 API Endpoints

## 📊 TPS Calculation

### How Transactions Per Second (TPS) is Calculated

**Before (Estimation):**
```
TPS = Blocks/sec × 5  // Rough estimate
```

**Now (Accurate):**
```
1. Parse real transaction counts from logs:
   "4 blocks, 2 total transactions" → Extract: 2 transactions

2. Calculate real TPS:
   TPS = (current_total_tx - previous_total_tx) / time_elapsed

3. Apply moving average (60-second window):
   - Collect TPS measurements over time
   - Average last N measurements for stability
   - Filter out unrealistic spikes (>1000 TPS)
```

**Example:**
```
Log: "4 blocks, 3 total transactions"
Previous total: 100 tx
Time elapsed: 2 seconds
TPS = (103 - 100) / 2 = 1.5 TPS
```

### GET /api/data

Trả về JSON data cho dashboard:

```json
{
  "timestamp": 1703123456.789,
  "nodes": [
    {
      "id": 0,
      "status": "healthy",
      "epoch": 0,
      "blocks": 150,           // Local epoch block index (B1, B2, etc.)
      "global_blocks": 2450,   // Global execution index (sent to Go)
      "commit_index": 150,
      "errors": 0,
      "warnings": 2,
      "last_activity": 1703123450.123
    }
  ],
  "epoch": {
    "current_epoch": 22,
    "epoch_start_timestamp": 1767329108,
    "elapsed_seconds": 0,
    "remaining_seconds": 180,
    "epoch_duration": 180
  },
  "system": {
    "cpu_usage": "45.2%",
    "memory_usage": "256MB",
    "memory_percent": "12.3%"
  },
  "logs": [
    {
      "timestamp": 1703123456.789,
      "level": "info",
      "message": "[INFO] Block committed successfully",
      "node": "0"
    }
  ],
  "aggregates": {
    "total_nodes": 4,
    "healthy_nodes": 4,
    "latest_block": 2450,        // Global execution index (primary metric)
    "latest_local_block": 150,   // Local epoch block index
    "total_errors": 0,
    "total_warnings": 2,
    "network_health": "healthy"
  }
}
```

## 🎨 Dashboard Features

### Overview Tab
- Real-time metrics cards
- Epoch progress visualization
- System performance indicators
- Network health status

### Nodes Tab
- Individual node status cards
- Per-node metrics (blocks, epoch, connections)
- Status indicators (healthy/warning/error)

### Logs Tab
- Live log streaming
- Color-coded log levels (info/warn/error)
- Auto-scroll to latest entries
- Clear logs functionality

### Controls
- **Refresh**: Manual data refresh
- **Auto Refresh**: Toggle automatic updates (2s interval)
- **Clear Logs**: Clear log display

## 🔧 Customization

### Thay đổi Port
```python
# Trong monitor.py
server = MonitoringServer(monitor, port=9090)  # Thay 8080 thành port khác
```

### Thay đổi Refresh Interval
```javascript
// Trong dashboard.html
let refreshInterval = 5000; // 5 seconds thay vì 2 seconds
```

### Thêm Metrics mới
```python
# Trong monitor.py - method get_dashboard_data()
'custom_metric': calculate_custom_metric(),
```

## 🚨 Troubleshooting

### Dashboard không load
```bash
# Check if monitoring server is running
curl http://localhost:8080/api/data

# Check virtual environment
cd monitoring && source venv/bin/activate
python -c "import flask, psutil; print('OK')"
```

### No data displayed
```bash
# Check log files exist
ls -la /home/abc/chain-n/mtn-consensus/metanode/logs/latest/

# Check file permissions
ls -la /home/abc/chain-n/mtn-consensus/metanode/logs/latest/node_0.log
```

### High CPU usage
```bash
# Reduce refresh interval
# Chỉnh refreshInterval trong dashboard.html
let refreshInterval = 5000; // 5 seconds
```

## 🔒 Security Notes

- Dashboard chạy trên localhost only
- Không có authentication
- Chỉ nên dùng trong development/testing
- Không expose port 8080 ra internet

## ⚙️ Production Configuration

### Environment Variables
```bash
export PORT=8080                       # Server port (default: 8080)
export MTN_CONSENSUS_METANODE_DIR=/path/to/metanode  # Path to metanode directory
export MONITORING_MODE=production       # production/development (legacy)
```

### Gunicorn Configuration
Production mode sử dụng các settings tối ưu:
- **Workers**: 2 (số CPU cores)
- **Threads**: 4 per worker
- **Timeout**: 30 seconds
- **Max requests**: 1000 (auto restart worker)
- **Keep alive**: 10 seconds

### Systemd Service (Linux)
Tạo file `/etc/systemd/system/mysticeti-monitor.service`:
```ini
[Unit]
Description=mtn-consensus Node Monitor
After=network.target

[Service]
Type=simple
User=mysticeti
WorkingDirectory=/home/abc/chain-n/mtn-consensus/metanode/monitoring
Environment=PATH=/home/abc/chain-n/mtn-consensus/metanode/monitoring/venv/bin
ExecStart=/home/abc/chain-n/mtn-consensus/metanode/monitoring/start_monitor.sh
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable và start service:
```bash
sudo systemctl daemon-reload
sudo systemctl enable mysticeti-monitor
sudo systemctl start mysticeti-monitor
sudo systemctl status mysticeti-monitor
```

## 🔒 Security & Best Practices

### Production Deployment
- **Luôn sử dụng production mode** cho production environments
- **Bind to specific IP** thay vì 0.0.0.0 nếu có thể
- **Use reverse proxy** (nginx) cho SSL termination
- **Enable access logs** để monitor traffic
- **Configure firewall** để chỉ allow internal access

### Development
- **Chỉ dùng development mode** cho local development
- **Không expose development server** ra internet
- **Enable debug mode** chỉ khi cần troubleshooting

### Monitoring
- **Setup log rotation** để tránh disk full
- **Monitor system resources** để detect memory leaks
- **Setup alerts** cho critical metrics (node down, high CPU)
- **Regular backups** của monitoring data nếu cần

## 🐛 Troubleshooting

### Cannot Find Metanode Directory

**Error:** `❌ Cannot find mtn-consensus metanode directory!`

**Solutions:**
1. **Set environment variable:**
   ```bash
   export MTN_CONSENSUS_METANODE_DIR=/path/to/metanode
   ./start_monitor.sh
   ```

2. **Use --path option:**
   ```bash
   ./start_monitor.sh --path /path/to/metanode
   ```

3. **Run from within metanode directory:**
   ```bash
   cd /path/to/metanode/monitoring
   ./start_monitor.sh
   ```

### Port Already in Use

**Error:** Port 8080 already in use

**Solution:**
```bash
./start_monitor.sh --port 3000
# or
export PORT=3000
./start_monitor.sh
```

### Permission Issues

**Error:** Permission denied

**Solutions:**
```bash
# Make script executable
chmod +x start_monitor.sh

# Check log directory permissions
ls -la /path/to/metanode/logs/
```

### Virtual Environment Issues

**Error:** Flask/psutil not available

**Solution:**
```bash
# Remove old venv and recreate
rm -rf venv
./start_monitor.sh
```

## 📈 Future Enhancements

- [ ] Add alerting system
- [ ] Historical data charts
- [ ] Configuration management
- [ ] Multi-cluster support
- [ ] Prometheus/Grafana integration
