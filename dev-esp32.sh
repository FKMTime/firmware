#!/bin/bash
set -e
source ~/export-esp.sh

cargo build --no-default-features --features=esp32,"$@" --target=xtensa-esp32-none-elf -r
VERSION=$(cat ./src/version.rs | grep VERSION | cut -d'"' -f 2)
EPOCH=$(date +%s)

espflash save-image --chip esp32 ./target/xtensa-esp32-none-elf/release/fkm-firmware "/tmp/fkm-build/v2_STATION_${VERSION}.bin"
./append_metadata.sh "/tmp/fkm-build/v2_STATION_${VERSION}.bin" "$VERSION" "STATION" "v2" "$EPOCH"
