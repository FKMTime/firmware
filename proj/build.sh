#!/bin/bash

# If first argument is a token, then check if latest release hash is the same as the current files hash, 
# if it is stop the build, if it is not continue the build
if [ $# -eq 1 ]; then
    FILES_HASH=$(find ./platformio.ini ./src ./lib ./include -type f -print0 | sort -fdz | xargs -0 sha1sum | grep -v ./src/version.h | sha1sum | awk '{print $1}')
    FILES_HASH=${FILES_HASH:0:8}

    JSON=$(curl -s -L -H "Accept: application/vnd.github+json" -H "Authorization: Bearer $1" https://api.github.com/repos/filipton/fkm-timer/releases/latest)
    RELEASE_HASH=$(echo $JSON | jq -r '.name')

    if [ "$FILES_HASH" == "$RELEASE_HASH" ]; then
        echo "No changes in the files, stopping the build"
        exit 0
    fi
fi

# if pio doesnt exists install it (for github actions)
if ! [ -x "$(command -v pio)" ]; then
    pip install --upgrade platformio
fi

pio run
