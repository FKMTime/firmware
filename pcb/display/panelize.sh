#!/bin/bash

if [ ! -f ./kikit-config.json ]; then
    echo "kikit-config.json not found!"
    exit 1
fi

# use kikit in docker
kikit="docker run --rm -v $(pwd):/files filipton/kikit-kicad:8.0"
mkdir -p ./output

$kikit panelize -p ./kikit-config.json ./display.kicad_pcb ./panel.kicad_pcb
$kikit fab jlcpcb ./display.kicad_pcb ./output

echo -e "\n\n"
echo "Done!"
echo "Gerbers are in ./output"
echo -e "\n\n"
