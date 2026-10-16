# Running the firmware in esp-emu

[esp-emu](https://github.com/espressif/esp-emulator) (Espressif, beta, closed source) runs the real
ESP32-C3 image with Wi-Fi. The `sim` feature (v4, UART stackmat) fills the gaps of the emulator:

| Real device | In `sim` builds |
|---|---|
| OLED (I2C) | fake SSD1309 on the I2C bus, printed to the console (half blocks) when it changes |
| RFID reader | `card <uid>` on stdin |
| Buttons | `btn <1-4> [ms]` on stdin (1 inspect, 2 delegate, 3 submit, 4 penalty) |
| Battery gauge | `bat <0-100>` on stdin (starts at 100) |
| Stackmat | the real UART driver; `sim/stackmat.py` plays the timer |
| Wi-Fi setup | seeded for the emulator's AP; also adds itself to the server (`auto_add`) |
| BLE, sleep | off (esp-emu can't run esp-radio's BLE) |

`sim/run.sh` builds with `sim/esp-hal-emu.patch` applied to a copy of esp-hal (esp-emu never sets the
interrupt matrix status registers, which esp-hal relies on), without touching `Cargo.lock`.

## Run

```sh
~/.local/bin/esp-emu --version                    # install: see esp-emulator's README
sim/server.sh ~/projects/micro-connector          # terminal 1: server on :8080 (dev mode, no auto-update)
SIM_WS_URL=ws://192.168.4.1:8080 sim/run.sh       # terminal 2: firmware; type commands here
python3 sim/stackmat.py                           # terminal 3: start | stop | reset | off | on
```

`192.168.4.1` is this machine inside the emulator. Without `SIM_WS_URL` the firmware uses mDNS,
which doesn't get answers through esp-emu's relay on macOS. Cards in micro-connector's `tests.json`:
`69420` (competitor), `3004425529` (judge).

A solve: `card 69420`, `btn 1`, stackmat `start` … `stop`, `btn 3`, `card 3004425529`, `card 69420`.

Cargo prints "patch `esp-hal` … was not used" for the separately built std library; the firmware
does use the patched copy.
