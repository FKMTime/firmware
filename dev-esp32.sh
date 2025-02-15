#!/bin/bash
set -e
source ~/export-esp.sh

get_version() {
    local target=$1
    local build_dir="target/${target}/release/build"
    if [ ! -d "$build_dir" ]; then
        echo "Build directory not found: ${build_dir}" >&2
        return 1
    fi

    local latest_file=$(find "$build_dir" -name "version.rs" -type f -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -n1 | cut -d' ' -f2-)
    if [ -f "$latest_file" ]; then
        cat "$latest_file"
        return 0
    fi

    echo "Version file not found for target $target" >&2
    return 1
}

cargo build --no-default-features --features=esp32,"$@" --target=xtensa-esp32-none-elf -r
VERSION=$(get_version "xtensa-esp32-none-elf" | grep VERSION | cut -d'"' -f 2)
EPOCH=$(date +%s)

espflash save-image --chip esp32 ./target/xtensa-esp32-none-elf/release/fkm-firmware "/tmp/fkm-build/v2_STATION_${VERSION}.bin"
./append_metadata.sh "/tmp/fkm-build/v2_STATION_${VERSION}.bin" "$VERSION" "STATION" "v2" "$EPOCH"
