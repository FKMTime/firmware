#!/bin/bash

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"

if ! command -v gh &> /dev/null
then
    echo "'gh' could not be found"
    exit
fi

if ! command -v pio &> /dev/null
then
    echo "'pio' could not be found"
    exit
fi

cd $SCRIPT_DIR/firmware
pio run

cd $SCRIPT_DIR
VERSION=$(head -c 8 ./firmware/.versum)
BUILD_TIME=$(printf "%d\n" "0x$(cat ./firmware/src/version.h | grep 'BUILD_TIME' | cut -d'"' -f 2)")
BUILD_TIME_HEX=$(printf "%08x\n" $BUILD_TIME)

if gh release list | grep -q "$BUILD_TIME-$VERSION" ; then
    echo "Release already exists"
    exit
fi

BUILD_FILES=$(ls $SCRIPT_DIR/firmware/build/*.$VERSION.$BUILD_TIME_HEX.bin)
if [ -z "$BUILD_FILES" ]; then
    echo "No build files found"
    exit
fi

gh release create "$BUILD_TIME-$VERSION" -t "$BUILD_TIME-$VERSION" --generate-notes
for file in $BUILD_FILES; do
    echo "Uploading $file"
    gh release upload "$BUILD_TIME-$VERSION" "$file"
done
