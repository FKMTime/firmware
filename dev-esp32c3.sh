#!/bin/bash
set -e
source ~/export-esp.sh

get_version() {
    local target=$1
    local hw=$2
    local build_dir="target/${target}"
    if [ ! -d "$build_dir" ]; then
        echo "Build directory not found: ${build_dir}" >&2
        return 1
    fi

    local files
    if stat -c "%Y" /dev/null >/dev/null 2>&1; then
        files=$(find "$build_dir" -type f -name "version.rs" -exec stat -c "%Y %n" {} \; | sort -nr | cut -d' ' -f2-)
    else
        files=$(find "$build_dir" -type f -name "version.rs" -exec stat -f "%m %N" {} \; | sort -nr | cut -d' ' -f2-)
    fi
    local f
    while IFS= read -r f; do
        [ -z "$f" ] && continue
        if grep -q "HW_VER: &str = \"$hw\"" "$f"; then
            cat "$f"
            return 0
        fi
    done <<< "$files"
    echo "Version file not found for target $target (hw=$hw)" >&2
    return 1
}

EXTRA_FEATURES=""
for arg in "$@"; do
    IFS=',' read -ra parts <<< "$arg"
    for part in "${parts[@]}"; do
        if [ "$part" != "v3" ] && [ "$part" != "v4" ] && [ -n "$part" ]; then
            EXTRA_FEATURES="${EXTRA_FEATURES:+$EXTRA_FEATURES,}$part"
        fi
    done
done

EPOCH=$(date +%s)
mkdir -p /tmp/fkm-build

for HW_FEATURE in v3 v4; do
    FEATURES="$HW_FEATURE${EXTRA_FEATURES:+,$EXTRA_FEATURES}"
    echo "=== Building $FEATURES ==="

    cargo build --no-default-features --features="$FEATURES" --target=riscv32imc-unknown-none-elf -r
    VERSION_RS=$(get_version "riscv32imc-unknown-none-elf" "$HW_FEATURE")
    VERSION=$(echo "$VERSION_RS" | grep VERSION | cut -d'"' -f 2)
    HW=$(echo "$VERSION_RS" | grep HW_VER | cut -d'"' -f 2)
    if [ -z "$VERSION" ] || [ -z "$HW" ]; then
        echo "Failed to read version/hw from generated version.rs for $HW_FEATURE" >&2
        exit 1
    fi

    espflash save-image --chip esp32c3 ./target/riscv32imc-unknown-none-elf/release/fkm-firmware "/tmp/fkm-build/${HW}_STATION_${VERSION}.bin"
    ./append_metadata.sh "/tmp/fkm-build/${HW}_STATION_${VERSION}.bin" "$VERSION" "STATION" "${HW}" "$EPOCH"
done

echo "=== Done, builds in /tmp/fkm-build: ==="
ls -1 /tmp/fkm-build/*_STATION_D*.bin 2>/dev/null || true
