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

## In a browser

```sh
python3 sim/web.py            # builds, starts esp-emu + stackmat + micro-connector, serves :8000
```

http://127.0.0.1:8000 shows the OLED and has the four buttons (hold as long as on the device,
keys 1–4), cards, the stackmat (space = start/stop, r = reset), battery, Wi-Fi off/on, server
off/on, power cycle, the live state and the log. `--no-server` runs without micro-connector.

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

## Monkey (robustness fuzzing)

`sim/monkey.py` runs the firmware in esp-emu with its own micro-connector per instance and abuses
it with a seeded, weighted random action list: button mashing, odd hold lengths around the real
thresholds, several buttons at once, card spam in every phase (competitor, judge, multi-group,
unknown), stackmat chaos (start/stop at wrong times, unplug, garbage bytes), battery changes,
menu walks, server restarts and power cycles. Same seed ⇒ same actions.

Every few actions it checks liveness (answers `state`, no unexpected reboot/panic, heap bound) and
state invariants; every `--chunk` actions it runs a recovery probe: get back to idle the way a user
would, then a full solve that the server must receive exactly once with the stackmat's time.
Solves the server got during the abuse must be consistent (known competitor, a time the stackmat
showed, one payload per session id).

```sh
python3 sim/monkey.py --seeds 1-40 --jobs 4 --minutes 5      # campaign (builds one image per job)
python3 sim/monkey.py --replay 7                              # same actions again
python3 sim/monkey.py --replay-file sim/out/7/actions.json --shrink   # minimize a failure
python3 sim/monkey.py --seed 7 --emit-hil tests.json          # export for micro-connector's HIL
for f in sim/scenarios/*.json; do python3 sim/monkey.py --replay-file $f; done   # regressions
```

`sim/scenarios/` holds scripted regressions for bugs the monkey found (same format as
`actions.json`, plus `expect` steps that wait for a `state` field to reach a value).

### Competition day (`--mode competition`)

A seeded, realistic competition instead of abuse: 20–40 competitors (some in several events, so
they pick their group), staff and competitor judges, the first group of 3–4 events (one
blindfolded: no inspection, 25 s time limit), each cut to a share of `--max-attempts`. Every attempt
is acted out like a person would (scan, group select, inspection 5–12 s, sometimes 15–17 s (+2) or
over 17 s (DNF), solve, judge's penalty, confirm, judge and competitor cards), with clumsy slips at
a few % each (wrong card first, double taps, forgotten/restarted inspection, fumbled timer, +2
applied and corrected, DNF hold, long/double submit, wrong button, scrolling past the group,
delegate call, Wi-Fi blip, abandoned attempt, solve over the limit) plus server restarts mid-group
and a draining battery. Each step's effect is checked on the device; after each attempt the
server's `EnterAttempt` log (`srv/logs/unix.log`) must hold exactly one solve with the intended
station, competitor, time, penalty, judge, group and inspection; at the end the whole list must
match. `--devices N` runs N stations (own esp-emu, eFuse MAC from `sim/make-efuse.py`, stackmat)
on one server, round by round, a competitor never at two stations at once.

```sh
python3 sim/monkey.py --mode competition --seeds 1-6 --jobs 3 --job-base 20 --max-attempts 36
python3 sim/monkey.py --mode competition --seed 4 --devices 3 --job-base 20 --slip-rate 2
python3 sim/monkey.py --replay-file sim/out/comp-d1/4/day.json            # same day again
```

`--job-base N` moves ports (emulators 20000+10·N.., servers 18100+N..) away from other runs;
`--time-scale` shortens solves (never inspection or holds). Logs: `sim/out/comp-d<N>/<seed>/`
(`day.json` = plan, intended results, mismatches, executed steps). Not covered: sleep (off in sim
builds), delegate answers (micro-connector's e2e never resolves an incident), BLE.

Failures land in `sim/out/<seed>/` (actions, firmware log with timestamps, server log). Needs a
built micro-connector (`cargo build --bins` in it; `--mc` path). The `state` sim command prints the
machine-readable line the monkey reads. Not covered: pads (`timer-func`), BLE, Wi-Fi drops (esp-emu
can't drop the link), real display/RFID timing.
