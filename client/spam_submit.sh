#!/bin/bash
#
# Spam transactions continuously to MetaNode RPC using metanode-client.
#
# Examples:
#   ./spam_submit.sh
#   ./spam_submit.sh --endpoint http://127.0.0.1:10100 --interval-ms 50
#   ./spam_submit.sh --bytes 128 --count 1000
#   ./spam_submit.sh --tps 20
#   ./spam_submit.sh --quiet                    # Chạy không hiển thị log (chỉ hiển thị lỗi)
#   ./spam_submit.sh --tps 20 --quiet           # Chạy với TPS 20 và không hiển thị log
#
set -euo pipefail

ENDPOINT="http://127.0.0.1:10100"
INTERVAL_MS="100"
BYTES="18"
COUNT="0"         # 0 = infinite
TPS="0"           # if >0, overrides interval-ms
QUIET="0"
RETRY_ON_ERROR="1"        # 1 = retry forever on transient errors (epoch transition), 0 = exit on first error
MAX_RETRY_SLEEP_MS="2000" # backoff cap per tx

usage() {
  cat <<EOF
Usage: $0 [options]

Options:
  --endpoint URL        RPC endpoint (default: $ENDPOINT)
  --interval-ms MS      Delay between submits in ms (default: $INTERVAL_MS)
  --tps N               Target TPS (overrides --interval-ms). Example: 20 => 50ms interval
  --bytes N             Random payload size in bytes (default: $BYTES)
  --count N             Number of tx to send (default: $COUNT; 0 = infinite)
  --quiet               Reduce per-tx output
  --no-retry            Exit on first error (default: retry forever)
  -h, --help            Show help

Notes:
  - Payload is random bytes, sent as hex string (like your manual command).
  - During in-process epoch transition, RPC can temporarily return:
      "consensus is shutting down: channel closed"
    This script retries by default so it won't stop during epoch changes.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --endpoint) ENDPOINT="$2"; shift 2 ;;
    --interval-ms) INTERVAL_MS="$2"; shift 2 ;;
    --tps) TPS="$2"; shift 2 ;;
    --bytes) BYTES="$2"; shift 2 ;;
    --count) COUNT="$2"; shift 2 ;;
    --quiet) QUIET="1"; shift ;;
    --no-retry) RETRY_ON_ERROR="0"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown arg: $1"; usage; exit 1 ;;
  esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLIENT_BIN="$ROOT_DIR/target/release/metanode-client"

if [[ ! -x "$CLIENT_BIN" ]]; then
  echo "❌ metanode-client not found or not executable: $CLIENT_BIN"
  echo "Build first:"
  echo "  cd $ROOT_DIR && cargo build --release"
  exit 1
fi

if [[ "$TPS" != "0" ]]; then
  if ! [[ "$TPS" =~ ^[0-9]+$ ]] || [[ "$TPS" -le 0 ]]; then
    echo "❌ --tps must be a positive integer"
    exit 1
  fi
  # interval_ms = ceil(1000 / tps)
  INTERVAL_MS="$(python3 - <<PY
import math
tps=int("$TPS")
print(int(math.ceil(1000.0/tps)))
PY
)"
fi

if ! [[ "$INTERVAL_MS" =~ ^[0-9]+$ ]]; then
  echo "❌ --interval-ms must be an integer"
  exit 1
fi
if ! [[ "$BYTES" =~ ^[0-9]+$ ]] || [[ "$BYTES" -le 0 ]]; then
  echo "❌ --bytes must be a positive integer"
  exit 1
fi
if ! [[ "$COUNT" =~ ^[0-9]+$ ]]; then
  echo "❌ --count must be an integer"
  exit 1
fi

echo "ℹ️  Spamming tx..."
echo "  endpoint     = $ENDPOINT"
echo "  bytes/tx     = $BYTES"
echo "  interval_ms  = $INTERVAL_MS"
if [[ "$COUNT" = "0" ]]; then
  echo "  count        = infinite"
else
  echo "  count        = $COUNT"
fi
echo ""

i=0
while true; do
  i=$((i+1))
  if [[ "$COUNT" != "0" ]] && [[ "$i" -gt "$COUNT" ]]; then
    echo "✅ Done. Sent $COUNT tx."
    exit 0
  fi

  # Generate random payload as hex string
  # (python3 is already a requirement in our tooling)
  DATA_HEX="$(python3 - <<PY
import os, binascii
n=int("$BYTES")
print(binascii.hexlify(os.urandom(n)).decode("ascii"))
PY
)"

  # Retry loop to survive in-process epoch restart ("channel closed" / "consensus is shutting down").
  sleep_ms=50
  while true; do
    if [[ "$QUIET" = "1" ]]; then
      if "$CLIENT_BIN" submit --endpoint "$ENDPOINT" --data "$DATA_HEX" >/dev/null 2>&1; then
        break
      fi
    else
      echo "➡️  tx #$i data_len=${BYTES}B"
      if "$CLIENT_BIN" submit --endpoint "$ENDPOINT" --data "$DATA_HEX"; then
        break
      fi
    fi

    if [[ "$RETRY_ON_ERROR" = "0" ]]; then
      echo "❌ submit failed; exiting (--no-retry enabled)"
      exit 1
    fi

    # Backoff (brief downtime during epoch transition is expected).
    sleep "$(python3 - <<PY
ms=int("$sleep_ms")
print(ms/1000.0)
PY
)"
    sleep_ms=$((sleep_ms * 2))
    if [[ "$sleep_ms" -gt "$MAX_RETRY_SLEEP_MS" ]]; then
      sleep_ms="$MAX_RETRY_SLEEP_MS"
    fi
  done

  if [[ "$INTERVAL_MS" -gt 0 ]]; then
    sleep "$(python3 - <<PY
ms=int("$INTERVAL_MS")
print(ms/1000.0)
PY
)"
  fi
done


