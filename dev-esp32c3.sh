#!/bin/bash
set -e
source ~/export-esp.sh

get_version() {
    local target=$1
    local build_dir="target/${target}"

    if [ ! -d "$build_dir" ]; then
        echo "Build directory not found: ${build_dir}" >&2
        return 1
    fi

    local latest_file=$(find "$build_dir" -type f -name "version.rs" -printf "%T@ %p\n" | sort -nr | head -n 1 | awk '{print $2}')

    if [ -f "$latest_file" ]; then
        cat "$latest_file"
        return 0
    fi

    echo "Version file not found for target $target" >&2
    return 1
}

cargo build --no-default-features --features="$@" --target=riscv32imc-unknown-none-elf -r
VERSION=$(get_version "riscv32imc-unknown-none-elf" | grep VERSION | cut -d'"' -f 2)
EPOCH=$(date +%s)

espflash save-image --chip esp32c3 ./target/riscv32imc-unknown-none-elf/release/fkm-firmware "/tmp/fkm-build/v3_STATION_${VERSION}.bin"
./append_metadata.sh "/tmp/fkm-build/v3_STATION_${VERSION}.bin" "$VERSION" "STATION" "v3" "$EPOCH"
