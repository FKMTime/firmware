#!/bin/sh
# Builds the `sim` firmware against the esp-emu patched crates and runs it in esp-emu.
# The patches live in $ESP_SIM_PATCHES (default ~/tmp/esp-sim-patches, see its README).
#   sim/run.sh                 mdns finds the server
#   SIM_WS_URL=ws://192.168.4.1:8080 sim/run.sh     fixed url (192.168.4.1 = this machine)
#   SIM_PROVISION=1 sim/run.sh  no seeded Wi-Fi: AP provisioning, panel on http://127.0.0.1:18080
#   SIM_WIFIMANAGER=~/projects/esp-hal-wifimanager sim/run.sh   local wifimanager checkout
#   SIM_BLE=tcp:localhost:9544 sim/run.sh   BLE through a Bumble controller (sim/ble_*.py)
#   SIM_HW=v3 sim/run.sh       v3 hardware (HD44780 behind the shift registers) instead of v4
#   SIM_BUILD_ONLY=1, SIM_HTTP_PORT/SIM_UART1_PORT/SIM_CONTROL_PORT: used by sim/monkey.py
# Extra arguments go to esp-emu (e.g. --timeout 60s).
set -eu
cd "$(dirname "$0")/.."

HW="${SIM_HW:-v4}"
case "$HW" in v3) T=target/sim-v3 ;; v4) T=target/sim ;; *) echo "SIM_HW must be v3 or v4" >&2; exit 2 ;; esac
mkdir -p "$T"
PATCHES=""
ESP_SIM_PATCHES="${ESP_SIM_PATCHES:-$HOME/tmp/esp-sim-patches}"
locked() { grep -A1 "^name = \"$1\"\$" Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p'; }
for CRATE in esp-hal esp-radio; do
    DIR=$("$ESP_SIM_PATCHES/apply.sh" "$CRATE-$(locked "$CRATE")")
    PATCHES="$PATCHES --config patch.crates-io.$CRATE.path=\"$DIR\""
done
if [ -n "${SIM_WIFIMANAGER:-}" ]; then
    PATCHES="$PATCHES --config patch.crates-io.esp-hal-wifimanager.path=\"$SIM_WIFIMANAGER\""
fi

# the patches must not leak into the committed lockfile
cp Cargo.lock "$T/Cargo.lock.bak"
trap 'cp "$T/Cargo.lock.bak" Cargo.lock' EXIT
# shellcheck disable=SC2086 # PATCHES holds several --config arguments
if [ -n "${SIM_WIFIMANAGER:-}" ]; then
    # a local version differing from the lockfile's would otherwise be ignored by the patch
    cargo update -q -p esp-hal-wifimanager $PATCHES
fi
# shellcheck disable=SC2086
CARGO_TARGET_DIR=$T cargo build -r --no-default-features --features $HW,sim $PATCHES
cp "$T/Cargo.lock.bak" Cargo.lock
trap - EXIT

ELF=$T/riscv32imc-unknown-none-elf/release/fkm-firmware
espflash save-image --chip esp32c3 --merge --partition-table partitions.csv "$ELF" "$T/sim.bin"
# SIM_BUILD_ONLY=1: stop here (sim/monkey.py copies $T/sim.bin and the ELF per instance)
[ -z "${SIM_BUILD_ONLY:-}" ] || exit 0
# firmware port 80 (its own AP while provisioning) on host :18080; `reset` on :5557 keeps flash
exec esp-emu --chip esp32c3 --firmware "$T/sim.bin" --elf "$ELF" \
    --net "user,hostfwd=tcp:127.0.0.1:${SIM_HTTP_PORT:-18080}-:80" \
    --uart1-tcp "127.0.0.1:${SIM_UART1_PORT:-5556}" \
    --control-tcp "127.0.0.1:${SIM_CONTROL_PORT:-5557}" ${SIM_BLE:+--ble-hci "$SIM_BLE"} "$@"
