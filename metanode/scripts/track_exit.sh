#!/bin/bash
cd /home/abc/chain-n/mtn-simple-2025/cmd/simple_chain
ulimit -n 100000
export RUST_BACKTRACE=full
export GOTRACEBACK=crash
export GOMEMLIMIT=4GiB
export XAPIAN_BASE_PATH="sample/node1/data/data/xapian"
export MVM_LOG_DIR="/home/abc/chain-n/mtn-consensus/metanode/logs/node_1"
./simple_chain -config=config-master-node1.json >> "/home/abc/chain-n/mtn-consensus/metanode/logs/node_1/go-master-stdout.log" 2>&1
echo "NODE 1 EXITED WITH CODE: $?" > track_exit_code.log
