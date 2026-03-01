#!/usr/bin/env python3
"""
WSGI Application for mtn-consensus Node Monitor
Used by Gunicorn in production mode
"""

import sys
import os

# Add parent directory to path for imports
sys.path.insert(0, os.path.dirname(__file__))
sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))

from monitor import NodeMonitor, MonitoringServer

# Create monitor instance
monitor = NodeMonitor(os.path.dirname(os.path.dirname(__file__)))

# Create Flask app
server = MonitoringServer(monitor, port=int(os.environ.get('PORT', 8080)))
app = server.create_flask_app()

if __name__ == "__main__":
    # For development/testing
    app.run(host='0.0.0.0', port=int(os.environ.get('PORT', 8080)), debug=False)
