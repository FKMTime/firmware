#!/bin/bash
source ~/export-esp.sh
cargo build --no-default-features --features=esp32c3 --target=riscv32imc-unknown-none-elf -r && espflash save-image --chip esp32c3 ./target/riscv32imc-unknown-none-elf/release/fkm-firmware "/tmp/fkm-build/esp32c3_STATION_$(cat ./src/version.rs | grep VERSION | cut -d'"' -f 2).bin"
