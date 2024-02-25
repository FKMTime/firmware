FROM kicad/kicad:8.0
RUN sudo apt update && sudo apt install -y \
    python3-pip \
    && sudo rm -rf /var/lib/apt/lists/*

RUN pip3 install kikit --break-system-packages
RUN sudo ln -s ~/.local/bin/kikit /bin/kikit
