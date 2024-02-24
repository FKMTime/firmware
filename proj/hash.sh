#!/bin/bash
find ./platformio.ini ./src ./lib ./include -type f -print0 | sort -fdz | xargs -0 sha1sum | grep -v ./src/version.h | sha1sum | awk '{print $1}'