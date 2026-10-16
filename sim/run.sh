#!/bin/sh
# Builds the `sim` firmware with the esp-emu patches (sim/*-emu.patch) and runs it in esp-emu.
#   sim/run.sh                 mdns finds the server
#   SIM_WS_URL=ws://192.168.4.1:8080 sim/run.sh     fixed url (192.168.4.1 = this machine)
#   SIM_PROVISION=1 sim/run.sh  no seeded Wi-Fi: AP provisioning, panel on http://127.0.0.1:18080
#   SIM_WIFIMANAGER=~/projects/esp-hal-wifimanager sim/run.sh   local wifimanager checkout
# Extra arguments go to esp-emu (e.g. --timeout 60s).
set -eu
cd "$(dirname "$0")/.."

T=target/sim
mkdir -p "$T"
PATCHES=""
# copy of a locked registry crate with sim/<crate>-emu.patch applied
for CRATE in esp-hal esp-hal-wifimanager; do
    VER=$(grep -A1 "^name = \"$CRATE\"\$" Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
    DIR="$T/$CRATE-$VER"
    SRC="$(ls -d "$HOME"/.cargo/registry/src/*/"$CRATE-$VER" | head -1)"
    if [ "$CRATE" = esp-hal-wifimanager ] && [ -n "${SIM_WIFIMANAGER:-}" ]; then
        # always re-copy: the checkout is being edited
        SRC="$SIM_WIFIMANAGER" DIR="$T/$CRATE-local"
        rm -f "$DIR/.patched"
    fi
    if [ ! -f "$DIR/.patched" ]; then
        rm -rf "$DIR"
        mkdir -p "$DIR" && (cd "$SRC" && tar cf - --exclude target --exclude .git .) | (cd "$DIR" && tar xf -)
        patch -s -d "$DIR" -p1 <"sim/$CRATE-emu.patch"
        touch "$DIR/.patched"
    fi
    PATCHES="$PATCHES --config patch.crates-io.$CRATE.path=\"$PWD/$DIR\""
done

# the patches must not leak into the committed lockfile
cp Cargo.lock "$T/Cargo.lock.bak"
trap 'cp "$T/Cargo.lock.bak" Cargo.lock' EXIT
# shellcheck disable=SC2086 # PATCHES holds several --config arguments
if [ -n "${SIM_WIFIMANAGER:-}" ]; then
    # a local version differing from the lockfile's would otherwise be ignored by the patch
    cargo update -q -p esp-hal-wifimanager $PATCHES
fi
# shellcheck disable=SC2086
CARGO_TARGET_DIR=$T cargo build -r --no-default-features --features v4,sim $PATCHES
cp "$T/Cargo.lock.bak" Cargo.lock
trap - EXIT

ELF=$T/riscv32imc-unknown-none-elf/release/fkm-firmware
espflash save-image --chip esp32c3 --merge --partition-table partitions.csv "$ELF" "$T/sim.bin"
# firmware port 80 (its own AP while provisioning) on host :18080; `reset` on :5557 keeps flash
exec esp-emu --chip esp32c3 --firmware "$T/sim.bin" --elf "$ELF" \
    --net "user,hostfwd=tcp:127.0.0.1:18080-:80" --uart1-tcp 127.0.0.1:5556 \
    --control-tcp 127.0.0.1:5557 "$@"
