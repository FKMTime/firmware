#!/bin/sh
# Local server for the sim: micro-connector (dev mode, plain ws on :8080, no BT, no auto-update)
# plus its fake FKMTime backend using the cards/groups/rounds in tests.json.
#   sim/server.sh ~/projects/micro-connector
set -eu
MC=$(cd "${1:?usage: sim/server.sh <micro-connector dir>}" && pwd)
D=${TMPDIR:-/tmp}/fkm-sim-server
mkdir -p "$D/fw" "$D/logs"
cp "$MC/tests.json" "$D/tests.json"

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
