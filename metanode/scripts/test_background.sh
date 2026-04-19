#!/bin/bash
cd /home/abc/chain-n/mtn-simple-2025/cmd/simple_chain
for i in {0..4}; do
    ulimit -n 100000
    export RUST_BACKTRACE=full
    export GOTRACEBACK=crash
    export GOMEMLIMIT=4GiB
    export XAPIAN_BASE_PATH="sample/node${i}/data/data/xapian"
    export MVM_LOG_DIR="/home/abc/chain-n/mtn-consensus/metanode/logs/node_${i}"
    nohup ./simple_chain -config=config-master-node${i}.json > "/home/abc/chain-n/mtn-consensus/metanode/logs/node_${i}/go-master-stdout.log" 2>&1 &
    sleep 4
done
echo "All 5 nodes started in background!"
