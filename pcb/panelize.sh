#!/bin/bash

if [ ! -f ./kikit-config.json ]; then
    echo "kikit-config.json not found!"
    exit 1
fi

if ! command -v kikit &> /dev/null
then
    echo "kikit not found, installing..."
    pip3 install kikit
fi

mkdir -p ./output

kikit panelize -p ./kikit-config.json ./stackmat.kicad_pcb ./panel.kicad_pcb
kikit fab jlcpcb ./panel.kicad_pcb ./output

echo -e "\n\n"
echo "Done!"
echo "Gerbers are in ./output"
echo -e "\n\n"
