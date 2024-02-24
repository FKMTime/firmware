#!/bin/bash

# If token is in env, then check if latest release hash is the same as the current files hash, 
# if it is stop the build, if it is not continue the build
if [ ! -z "$GH_TOKEN" ]; then
    FILES_HASH=$(bash ./hash.sh)
    FILES_HASH=${FILES_HASH:0:8}

    JSON=$(curl -s -L -H "Accept: application/vnd.github+json" -H "Authorization: Bearer $GH_TOKEN" https://api.github.com/repos/filipton/fkm-timer/releases/latest)
    RELEASE_HASH=$(echo $JSON | jq -r '.name' | cut -d'-' -f2)
    echo "Files hash: $FILES_HASH Release hash: $RELEASE_HASH"

    if [ "$FILES_HASH" == "$RELEASE_HASH" ]; then
        echo "No changes in the files, stopping the build"
        echo "SKIP_BUILD=1" >> $GITHUB_ENV

        exit 0
    fi
fi

# if pio doesnt exists install it (for github actions)
if ! [ -x "$(command -v pio)" ]; then
    pip install --upgrade platformio
fi

pio run
