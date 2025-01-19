#!/bin/bash
source ~/export-esp.sh
cargo build --no-default-features --features=esp32,"$@" --target=xtensa-esp32-none-elf -r && espflash save-image --chip esp32 ./target/xtensa-esp32-none-elf/release/fkm-firmware "/tmp/fkm-build/esp32_STATION_$(cat ./src/version.rs | grep VERSION | cut -d'"' -f 2).bin"
