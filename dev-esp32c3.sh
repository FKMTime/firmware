#!/bin/bash
set -e
source ~/export-esp.sh

cargo build --no-default-features --features=esp32c3,"$@" --target=riscv32imc-unknown-none-elf -r
VERSION=$(cat ./src/version.rs | grep VERSION | cut -d'"' -f 2)
EPOCH=$(date +%s)

espflash save-image --chip esp32c3 ./target/riscv32imc-unknown-none-elf/release/fkm-firmware "/tmp/fkm-build/v3_STATION_${VERSION}.bin"
./append_metadata.sh "/tmp/fkm-build/v3_STATION_${VERSION}.bin" "$VERSION" "STATION" "v3" "$EPOCH"
