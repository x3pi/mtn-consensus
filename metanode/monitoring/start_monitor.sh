#!/bin/bash

# mtn-consensus Node Monitor Startup Script
# This script can be run from any directory and will automatically find the metanode directory

echo "🔍 Starting mtn-consensus Node Monitor..."

# Function to find metanode directory
find_metanode_dir() {
    local script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    # Option 1: Use environment variable if set
    if [ ! -z "$MTN_CONSENSUS_METANODE_DIR" ]; then
        if [ -d "$MTN_CONSENSUS_METANODE_DIR" ]; then
            echo "$MTN_CONSENSUS_METANODE_DIR"
            return 0
        else
            echo "⚠️  MTN_CONSENSUS_METANODE_DIR is set but directory doesn't exist: $MTN_CONSENSUS_METANODE_DIR" >&2
        fi
    fi

    # Option 2: Check if script is in metanode/monitoring directory
    local candidate_dir="$(dirname "$script_dir")"
    if [ -f "$candidate_dir/src/main.rs" ] && [ -d "$candidate_dir/config" ]; then
        echo "$candidate_dir"
        return 0
    fi

    # Option 3: Search upwards for metanode directory
    local current_dir="$script_dir"
    for i in {1..5}; do
        current_dir="$(dirname "$current_dir")"
        if [ -f "$current_dir/src/main.rs" ] && [ -d "$current_dir/config" ]; then
            echo "$current_dir"
            return 0
        fi
    done

    # Option 4: Try common relative paths from current working directory
    local cwd="$(pwd)"
    local relative_paths=(
        "."
        "../metanode"
        "../../mtn-consensus/metanode"
        "../../../chain-n/mtn-consensus/metanode"
        "$HOME/chain-n/mtn-consensus/metanode"
    )

    for path in "${relative_paths[@]}"; do
        if [ -f "$path/src/main.rs" ] && [ -d "$path/config" ]; then
            echo "$(cd "$path" && pwd)"
            return 0
        fi
    done

    return 1
}

# Find metanode directory
METANODE_DIR=$(find_metanode_dir)
if [ $? -ne 0 ]; then
    echo "❌ Cannot find mtn-consensus metanode directory!"
    echo ""
    echo "Please set the MTN_CONSENSUS_METANODE_DIR environment variable:"
    echo "  export MTN_CONSENSUS_METANODE_DIR=/path/to/metanode"
    echo ""
    echo "Or run this script from within the metanode directory structure."
    exit 1
fi

SCRIPT_DIR="$METANODE_DIR/monitoring"

echo "📁 Found metanode directory: $METANODE_DIR"
echo "📊 Monitoring logs directory: $METANODE_DIR/logs"

# Check if Python is available
if ! command -v python3 &> /dev/null; then
    echo "❌ Python3 is not installed. Please install Python3 to run the monitor."
    exit 1
fi

# Setup virtual environment with psutil
echo "🔧 Setting up monitoring environment..."
cd "$METANODE_DIR/monitoring"

# Check if virtual environment exists
if [ ! -d "venv" ]; then
    echo "📦 Creating virtual environment..."
    python3 -m venv venv
fi

# Always activate venv
echo "🔧 Activating virtual environment..."
source venv/bin/activate

# Install psutil if not present
python -c "import psutil" 2>/dev/null
if [ $? -ne 0 ]; then
    echo "📦 Installing psutil..."
    pip install psutil
fi

# Verify Flask and psutil are available
python -c "import flask, psutil; print('✅ Flask and psutil available')" 2>/dev/null
if [ $? -ne 0 ]; then
    echo "❌ Failed to setup Flask or psutil."
    echo "   Install missing packages:"
    echo "   pip install flask psutil"
    exit 1
fi

# Check for existing monitoring process and kill it
echo "🔍 Checking for existing monitoring processes..."
MONITOR_PID=$(lsof -ti :8080 2>/dev/null)
if [ ! -z "$MONITOR_PID" ]; then
    echo "⚠️  Found existing monitoring process (PID: $MONITOR_PID), killing it..."
    kill $MONITOR_PID 2>/dev/null
    sleep 2
    # Force kill if still running
    if kill -0 $MONITOR_PID 2>/dev/null; then
        echo "🔨 Force killing process..."
        kill -9 $MONITOR_PID 2>/dev/null
        sleep 1
    fi
fi

# Parse command line arguments
MODE="production"
METANODE_PATH=""
PORT=${PORT:-8080}

while [[ $# -gt 0 ]]; do
    case $1 in
        --development|-d)
            MODE="development"
            shift
            ;;
        --production|-p)
            MODE="production"
            shift
            ;;
        --path|-P)
            METANODE_PATH="$2"
            shift 2
            ;;
        --port)
            PORT="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --development, -d    Run in development mode (Flask dev server)"
            echo "  --production, -p     Run in production mode (Gunicorn WSGI server) [default]"
            echo "  --path, -P PATH      Specify metanode directory path"
            echo "  --port PORT          Specify server port [default: 8080]"
            echo "  --help, -h           Show this help message"
            echo ""
            echo "Environment Variables:"
            echo "  MTN_CONSENSUS_METANODE_DIR    Path to metanode directory"
            echo "  PORT                      Server port (can also use --port)"
            echo ""
            echo "Examples:"
            echo "  $0                         # Production mode, auto-detect path"
            echo "  $0 --development           # Development mode"
            echo "  $0 --path /custom/path     # Specify custom metanode path"
            echo "  $0 --port 3000             # Run on port 3000"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information."
            exit 1
            ;;
    esac
done

# Override metanode dir if specified via command line
if [ ! -z "$METANODE_PATH" ]; then
    if [ -d "$METANODE_PATH" ]; then
        METANODE_DIR="$(cd "$METANODE_PATH" && pwd)"
        echo "📁 Using specified metanode directory: $METANODE_DIR"
    else
        echo "❌ Specified metanode directory does not exist: $METANODE_PATH"
        exit 1
    fi
fi

# Start the monitoring server
echo ""
if [ "$MODE" = "development" ]; then
    echo "🚀 Starting DEVELOPMENT server on http://localhost:$PORT"
    echo "📊 Dashboard: http://localhost:$PORT/dashboard"
    echo "🔌 API: http://localhost:$PORT/api/data"
    echo "⚠️  WARNING: Development server - not for production use!"
    echo ""
    echo "Press Ctrl+C to stop the monitor"
else
    echo "🚀 Starting PRODUCTION server on http://localhost:$PORT"
    echo "📊 Dashboard: http://localhost:$PORT/dashboard"
    echo "🔌 API: http://localhost:$PORT/api/data"
    echo "🔒 Using Gunicorn WSGI server"
    echo ""
    echo "Press Ctrl+C to stop the monitor"
fi

cd "$METANODE_DIR"
# Ensure we're in the right directory and venv is activated
cd monitoring

if [ "$MODE" = "development" ]; then
    export PORT=$PORT
    exec python wsgi.py
else
    # Production mode with Gunicorn
    export PORT=$PORT
    exec gunicorn \
        --bind 0.0.0.0:$PORT \
        --workers 2 \
        --threads 4 \
        --worker-class sync \
        --max-requests 1000 \
        --max-requests-jitter 50 \
        --timeout 30 \
        --keep-alive 10 \
        --access-logfile - \
        --error-logfile - \
        --log-level info \
        wsgi:app
fi
