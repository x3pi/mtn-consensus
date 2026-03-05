#!/bin/bash

# Script để test hệ thống MetaNode consensus
# Test networking connectivity và submit test transactions

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Get script directory and change to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

print_info() {
    echo -e "${GREEN}ℹ️  $1${NC}"
}

print_warn() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

# Function to check if port is listening
check_port_listening() {
    local host=$1
    local port=$2
    local service=$3

    if nc -z "$host" "$port" 2>/dev/null; then
        print_success "Port $port ($service) is listening on $host"
        return 0
    else
        print_error "Port $port ($service) is NOT listening on $host"
        return 1
    fi
}

# Function to test TCP connectivity between nodes
test_node_connectivity() {
    local from_node=$1
    local to_host=$2
    local to_port=$3
    local to_service=$4

    # Use timeout to avoid hanging
    if timeout 5 nc -z "$to_host" "$to_port" 2>/dev/null; then
        print_success "Node $from_node can connect to $to_host:$to_port ($to_service)"
        return 0
    else
        print_warn "Node $from_node CANNOT connect to $to_host:$to_port ($to_service)"
        return 1
    fi
}

# Function to submit test transaction
submit_test_transaction() {
    local node_id=$1
    local rpc_port=$2

    # Create a simple test transaction (just some dummy data)
    local tx_data='{"type": "test_transaction", "timestamp": "'$(date +%s)'", "node_id": '$node_id', "message": "Test transaction from test script"}'

    print_info "Submitting test transaction to node $node_id (RPC port $rpc_port)..."

    # Use curl to submit transaction via RPC
    local response
    response=$(curl -s -X POST "http://127.0.0.1:$rpc_port" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\": \"2.0\", \"method\": \"submit_transaction\", \"params\": {\"transaction\": \"$tx_data\"}, \"id\": 1}" 2>/dev/null || echo "curl_failed")

    if [ "$response" = "curl_failed" ]; then
        print_error "Failed to connect to RPC server for node $node_id (port $rpc_port)"
        return 1
    fi

    # Check if response contains success
    if echo "$response" | grep -q '"result"\|"success"'; then
        print_success "Test transaction submitted successfully to node $node_id"
        return 0
    else
        print_warn "Test transaction submission to node $node_id may have failed. Response: $response"
        return 1
    fi
}

# Function to check if tmux session is running
check_node_running() {
    local node_id=$1

    if tmux has-session -t "metanode-$node_id" 2>/dev/null; then
        print_success "Node $node_id is running (tmux session metanode-$node_id exists)"
        return 0
    else
        print_error "Node $node_id is NOT running (tmux session metanode-$node_id not found)"
        return 1
    fi
}

# Main test function
main() {
    echo "=========================================="
    echo "🔍 MetaNode System Test Script"
    echo "=========================================="
    echo ""

    # Node configuration
    declare -A nodes
    nodes[0]="127.0.0.1:9000:10100"  # consensus_port:rpc_port
    nodes[1]="127.0.0.1:9001:10101"
    nodes[2]="127.0.0.1:9002:10102"
    nodes[3]="127.0.0.1:9003:10103"

    echo "📋 Node Configuration:"
    for node_id in "${!nodes[@]}"; do
        IFS=':' read -r consensus_host consensus_port rpc_port <<< "${nodes[$node_id]}"
        echo "  Node $node_id: Consensus $consensus_host:$consensus_port, RPC 127.0.0.1:$rpc_port"
    done
    echo ""

    # Step 1: Check if nodes are running
    print_info "Step 1: Checking if all nodes are running..."
    local running_nodes=0
    for node_id in "${!nodes[@]}"; do
        if check_node_running "$node_id"; then
            ((running_nodes++))
        fi
    done

    if [ "$running_nodes" -lt 4 ]; then
        print_error "Not all nodes are running! Only $running_nodes/4 nodes found."
        print_info "Please start all nodes first with: ./scripts/run_full_system.sh"
        exit 1
    fi
    print_success "All $running_nodes nodes are running"
    echo ""

    # Step 2: Check port listening
    print_info "Step 2: Checking if consensus ports are listening..."
    local listening_ports=0
    for node_id in "${!nodes[@]}"; do
        IFS=':' read -r consensus_host consensus_port rpc_port <<< "${nodes[$node_id]}"
        if check_port_listening "$consensus_host" "$consensus_port" "consensus-node-$node_id"; then
            ((listening_ports++))
        fi
    done

    if [ "$listening_ports" -lt 4 ]; then
        print_error "Not all consensus ports are listening! Only $listening_ports/4 ports available."
    else
        print_success "All $listening_ports consensus ports are listening"
    fi
    echo ""

    # Step 3: Test inter-node connectivity
    print_info "Step 3: Testing inter-node connectivity..."
    local connectivity_tests=0
    local successful_tests=0

    for from_node in "${!nodes[@]}"; do
        for to_node in "${!nodes[@]}"; do
            if [ "$from_node" != "$to_node" ]; then
                IFS=':' read -r to_host to_port _ <<< "${nodes[$to_node]}"
                if test_node_connectivity "$from_node" "$to_host" "$to_port" "consensus-node-$to_node"; then
                    ((successful_tests++))
                fi
                ((connectivity_tests++))
            fi
        done
    done

    print_info "Connectivity test results: $successful_tests/$connectivity_tests successful"
    if [ "$successful_tests" -lt "$connectivity_tests" ]; then
        print_warn "Some nodes cannot connect to each other. This may cause consensus issues."
    fi
    echo ""

    # Step 4: Submit test transactions
    print_info "Step 4: Submitting test transactions to trigger consensus..."

    # Submit to node 0 first (the one with executor_enabled=true)
    IFS=':' read -r _ _ rpc_port <<< "${nodes[0]}"
    if submit_test_transaction 0 "$rpc_port"; then
        print_success "Test transaction submitted to node 0"
    else
        print_warn "Failed to submit test transaction to node 0"
    fi

    # Wait a bit for consensus to process
    print_info "Waiting 10 seconds for consensus to process transactions..."
    sleep 10

    # Submit to other nodes
    for node_id in 1 2 3; do
        IFS=':' read -r _ _ rpc_port <<< "${nodes[$node_id]}"
        if submit_test_transaction "$node_id" "$rpc_port"; then
            print_success "Test transaction submitted to node $node_id"
        else
            print_warn "Failed to submit test transaction to node $node_id"
        fi
    done

    echo ""
    print_info "Test completed! Check the logs for consensus activity."
    print_info "Use 'tmux attach -t metanode-0' to view node 0 logs"
    print_info "Use 'tmux attach -t metanode-1' to view node 1 logs"
    echo ""

    # Summary
    echo "=========================================="
    echo "📊 Test Summary:"
    echo "=========================================="
    echo "Running nodes: $running_nodes/4"
    echo "Listening ports: $listening_ports/4"
    echo "Connectivity tests: $successful_tests/$connectivity_tests"
    echo ""

    if [ "$running_nodes" -eq 4 ] && [ "$listening_ports" -eq 4 ] && [ "$successful_tests" -eq "$connectivity_tests" ]; then
        print_success "All tests passed! System appears to be working correctly."
    else
        print_warn "Some tests failed. Check the issues above."
        print_info "Common solutions:"
        print_info "  1. Restart nodes: ./scripts/stop_full_system.sh && ./scripts/run_full_system.sh"
        print_info "  2. Check firewall: sudo ufw status"
        print_info "  3. Check logs for specific errors"
    fi
}

# Run main function
main "$@"
