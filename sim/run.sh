#!/bin/sh
# Builds the `sim` firmware with the esp-emu esp-hal patch and runs it in esp-emu.
#   sim/run.sh                 mdns finds the server
#   SIM_WS_URL=ws://192.168.4.1:8080 sim/run.sh     fixed url (192.168.4.1 = this machine)
# Extra arguments go to esp-emu (e.g. --timeout 60s).
set -eu
cd "$(dirname "$0")/.."

T=target/sim
VER=$(grep -A1 '^name = "esp-hal"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
SRC=$(ls -d "$HOME"/.cargo/registry/src/*/"esp-hal-$VER" | head -1)
if [ ! -f "$T/esp-hal-$VER/.patched" ]; then
    rm -rf "$T/esp-hal-$VER" && mkdir -p "$T"
    cp -R "$SRC" "$T/esp-hal-$VER"
    patch -s -d "$T/esp-hal-$VER" -p1 <sim/esp-hal-emu.patch
    touch "$T/esp-hal-$VER/.patched"
fi

# the patch must not leak into the committed lockfile
cp Cargo.lock "$T/Cargo.lock.bak"
trap 'cp "$T/Cargo.lock.bak" Cargo.lock' EXIT
CARGO_TARGET_DIR=$T cargo build -r --no-default-features --features v4,sim \
    --config "patch.crates-io.esp-hal.path=\"$PWD/$T/esp-hal-$VER\""
cp "$T/Cargo.lock.bak" Cargo.lock
trap - EXIT

ELF=$T/riscv32imc-unknown-none-elf/release/fkm-firmware
espflash save-image --chip esp32c3 --merge --partition-table partitions.csv "$ELF" "$T/sim.bin"
exec esp-emu --chip esp32c3 --firmware "$T/sim.bin" --elf "$ELF" --net user \
    --uart1-tcp 127.0.0.1:5556 "$@"
