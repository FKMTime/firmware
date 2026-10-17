#!/bin/sh
# Local server for the sim: micro-connector (dev mode, plain ws on :8080, no BT, no auto-update)
# plus its e2e backend answering card lookups from tests.json (its test cases replaced by one
# idle test, so it doesn't drive the device).
#   sim/server.sh ~/projects/micro-connector
set -eu
MC=$(cd "${1:?usage: sim/server.sh <micro-connector dir>}" && pwd)
D=${TMPDIR:-/tmp}/fkm-sim-server
mkdir -p "$D/fw" "$D/logs"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["tests"]=[{"name":"idle (sim)","steps":[{"type":"Sleep","data":86400000}]}]; json.dump(d,open(sys.argv[2],"w"))' \
    "$MC/tests.json" "$D/tests.json"

(cd "$MC" && cargo build -q --bins)
export SOCKET_PATH="$D/socket.sock"
rm -f "$SOCKET_PATH"
(cd "$D" && exec "$MC/target/debug/e2e") >"$D/backend.log" 2>&1 &
BACKEND=$!
trap 'kill $BACKEND' EXIT
sleep 1
echo "server on :8080, backend log: $D/backend.log"
DEV=1 NO_TLS=1 NO_BT=1 PORT=8080 FIRMWARE_DIR="$D/fw" DEVICE_LOGS="$D/logs" \
    "$MC/target/debug/backend"
