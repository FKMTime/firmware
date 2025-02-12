#!/bin/bash
set -e
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"

if ! command -v gh &> /dev/null
then
    echo "'gh' could not be found"
    exit
fi

if ! command -v cargo &> /dev/null
then
    echo "'cargo' could not be found"
    exit
fi

cd $SCRIPT_DIR
LAST_FIRMWARE_VERSION=$(gh release list | head -n 1 | cut -f 1)
echo "Last firmware version: $LAST_FIRMWARE_VERSION"
echo -n "Enter new firmware version: "
RELEASE_VERSION=""
while [ -z "$RELEASE_VERSION" ]; do
    read RELEASE_VERSION
done

source ~/export-esp.sh
RELEASE_BUILD="$RELEASE_VERSION" cargo build -r
RELEASE_BUILD="$RELEASE_VERSION" cargo esp32 -r

VERSION=$(cat ./src/version.rs | grep VERSION | cut -d'"' -f 2)
EPOCH=$(date +%s)

espflash save-image --chip esp32 ./target/xtensa-esp32-none-elf/release/fkm-firmware "/tmp/fkm-build/v2_STATION_${VERSION}.bin"
espflash save-image --chip esp32c3 ./target/riscv32imc-unknown-none-elf/release/fkm-firmware "/tmp/fkm-build/v3_STATION_${VERSION}.bin"
./append_metadata.sh "/tmp/fkm-build/v2_STATION_${VERSION}.bin" "$VERSION" "STATION" "v2" "$EPOCH"
./append_metadata.sh "/tmp/fkm-build/v3_STATION_${VERSION}.bin" "$VERSION" "STATION" "v3" "$EPOCH"

cd $SCRIPT_DIR
VERSION=$(cat ./src/version.rs | grep 'VERSION' | cut -d'"' -f 2)
echo "Version: $VERSION"

if gh release view "$VERSION" ; then
    echo "Release already exists"
    exit
fi


BUILD_FILES=$(ls /tmp/fkm-build/*_"$VERSION".bin)
if [ -z "$BUILD_FILES" ]; then
    echo "No build files found"
    exit
fi

gh release create "$VERSION" -t "$VERSION" --generate-notes
for file in $BUILD_FILES; do
    echo "Uploading $file"
    gh release upload "$VERSION" "$file"
done
