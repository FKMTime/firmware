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
| BLE | starts (patched esp-radio), but esp-emu never answers its HCI commands: no BLE flows yet |
| Sleep | off |

`sim/run.sh` builds against patched copies of esp-hal (esp-emu never sets the interrupt matrix
status registers, which esp-hal relies on) and esp-radio (BLE bring-up behind the ESP-IDF names
esp-emu intercepts), without touching `Cargo.lock`. The patches live in
`$ESP_SIM_PATCHES` (default `~/tmp/esp-sim-patches`, see its README). The sim build also trades
12 KB of heap for main stack: wifimanager 0.6.1's Wi-Fi init overruns the ~15.6 KB the sim statics
leave. Control port `127.0.0.1:5557` takes `reset` (flash kept).

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

## Wi-Fi provisioning

```sh
SIM_PROVISION=1 sim/run.sh                                    # nothing seeded: the firmware starts its AP
curl --compressed http://127.0.0.1:18080/                     # panel (also /list, /update)
curl -X POST http://127.0.0.1:18080/setup \
  --data '{"ssid":"myssid","psk":"mypassword","data":{"mdns":false,"ws_url":"ws://192.168.4.1:8080"}}'
printf 'reset\n' | nc 127.0.0.1 5557                          # reboots straight onto the saved Wi-Fi
```

esp-emu joins the firmware's AP and gets a lease from its DHCP server (`RUST_LOG=debug` shows it);
the panel is forwarded to `127.0.0.1:18080`. Bad setups (invalid JSON, wrong password, unknown
network) are logged and the AP keeps running.

## BLE (not working yet)

`SIM_BLE=tcp:localhost:9544 sim/run.sh` connects the firmware to a Bumble controller, e.g.
`~/tmp/esp-sim-patches/venv/bin/python sim/ble_phone.py` (a "phone" for BLE provisioning). esp-emu
intercepts the patched esp-radio and receives the first HCI command (`BLE HCI: Reset`), but never
calls esp-radio's `notify_host_recv`/`notify_host_send_available`, so no response arrives and the
host stack waits forever. AP provisioning keeps working next to it.

A solve: `card 69420`, `btn 1`, stackmat `start` … `stop`, `btn 3`, `card 3004425529`, `card 69420`.

Cargo prints "patch `esp-hal` … was not used" for the separately built std library; the firmware
does use the patched copy.
