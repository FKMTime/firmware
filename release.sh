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
LAST_FIRMWARE_VERSION=$(gh release list | head -n 1 | cut -f 1)
echo "Last firmware version: $LAST_FIRMWARE_VERSION"
echo -n "Enter new firmware version: "
RELEASE_VERSION=""
while [ -z "$RELEASE_VERSION" ]; do
    read RELEASE_VERSION
done

RELEASE_BUILD="$RELEASE_VERSION" pio run

cd $SCRIPT_DIR
VERSION=$(cat ./firmware/src/version.h | grep 'FIRMWARE_VERSION' | cut -d'"' -f 2)
echo "Version: $VERSION"

if gh release list | grep -q "$VERSION" ; then
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
