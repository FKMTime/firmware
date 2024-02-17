#!/bin/bash
if [ -n "$(which pio)" ]; then
    echo "PlatformIO is already installed"
    exit 0
fi

if [ -z "$(which curl)" ]; then
    sudo apt-get install curl
fi

if [ -z "$(which python3)" ]; then
    sudo apt-get install python3
fi

curl -fsSL -o /tmp/get-platformio.py https://raw.githubusercontent.com/platformio/platformio-core-installer/master/get-platformio.py
python3 /tmp/get-platformio.py
rm /tmp/get-platformio.py

mkdir -p ~/.local/bin
ln -s ~/.platformio/penv/bin/platformio ~/.local/bin/platformio
ln -s ~/.platformio/penv/bin/pio ~/.local/bin/pio
ln -s ~/.platformio/penv/bin/piodebuggdb ~/.local/bin/piodebuggdb

if [ -f ~/.zshrc ]; then
    echo "export PATH=\$PATH:\$HOME/.local/bin" >> ~/.zshrc
else
    echo "export PATH=\$PATH:\$HOME/.local/bin" >> ~/.bashrc
fi
