#!/bin/bash

# if pio doesnt exists install it (for github actions)
if ! [ -x "$(command -v pio)" ]; then
    pip install --upgrade platformio
fi

pio run
