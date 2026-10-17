# Running the firmware in esp-emu

[esp-emu](https://github.com/espressif/esp-emulator) (Espressif, beta, closed source) runs the real
ESP32-C3 image with Wi-Fi. The firmware runs its real drivers: the bus tap in the patched esp-hal
(`sim/patches/`, `ESP_SIM_TAP=1`) hands every GPIO, I2C, SPI and ADC access to the host over
esp-emu's UART0 socket, where `sim/bus.py` wires the device models of `sim/devices.py` like the
PCB does (`SIM_HW=v3|v4`, `--hw v3|v4`; v4 is the default):

| Real device | Model on the host |
|---|---|
| v4 OLED (I2C 0x3C) | SSD1309 page-addressing GRAM, printed to the console (half blocks) when it changes |
| v3 LCD | 74HC595 chain decoded from the GPIO10/21/1 (data/clock/latch) edges → HD44780 on the second register (RS, E, DB4–7, backlight): 8-bit power-on state, 4-bit nibble pairing, DDRAM/CGRAM, entry mode/shift; a transfer starting within the busy time (37/41 µs, 1.52 ms for clear/home) is lost. Printed as a 16×2 box |
| RFID | MFRC522 register model (v4: I2C 0x28, v3: SPI with CS from the shift register) answering the real esp-hal-mfrc522 driver: REQA/anticollision/select/HLTA, MFAuthent against the card's sector trailer, MIFARE read/write. `card <uid>` holds a 1K card in the field for 3 s; what is written to a card stays on it |
| Buttons | `btn <1-4> [ms]` (1 inspect, 2 delegate, 3 submit, 4 penalty). v4: GPIO0–3 pulses, timed in emulated time by the tap; v3: the matrix (shift register row × GPIO3) answered per scan |
| Battery | `bat <0-100>` (starts at 100). v4: BQ27441 on I2C 0x55; v3: ADC1 channel 2 through the divider |
| Stackmat | the real UART1 driver; `sim/stackmat.py` plays the timer |
| Wi-Fi setup | the host writes Wi-Fi + server url into the image's NVS partition (`bus.py nvs`, esp-nvs-partition-tool); the firmware also adds itself to the server (`auto_add`) |
| BLE | starts (patched esp-radio), but esp-emu never answers its HCI commands: no BLE flows yet |
| Sleep | off |

The firmware's `sim` feature (`SIM_CLIPPY=1 sim/run.sh` lints it) only adds what the host can't see: `state` (the `@state` line the
scripts read, v3 also with lcd_v3's change cache as `lcdtext`), `heap` (current/peak/capacity,
also available during provisioning), and `wifi off|on` (radio torn down
/ restarted), read from the tap's console frames; it needs the patched esp-hal. Faults the host
injects: `lcd glitch|drop|reset|slow <n>` (below), `rfid error [n]` (parity errors on the next
transceives), `i2c nack <addr hex> [n]`. `tap stats` prints the tap's traffic and round-trip times.

`sim/run.sh` builds against patched copies of esp-hal (esp-emu never sets the interrupt matrix
status registers, which esp-hal relies on; the bus tap) and esp-radio (BLE bring-up behind the
ESP-IDF names esp-emu intercepts), without touching `Cargo.lock`. The patches are in
`sim/patches/` (see its README); the patched crates go to `target/sim-patches/`. Control port
`127.0.0.1:5557` takes `reset` (flash kept).

## Run

```sh
~/.local/bin/esp-emu --version                    # install: see esp-emulator's README
sim/server.sh ~/projects/micro-connector          # terminal 1: server on :8080 (dev mode, no auto-update)
SIM_WS_URL=ws://192.168.4.1:8080 sim/run.sh       # terminal 2: firmware; type commands here
python3 sim/stackmat.py                           # terminal 3: start | stop | reset | off | on
```

`sim/run.sh` takes the device commands above; `state` and `wifi off|on` go to the firmware.
`192.168.4.1` is this machine inside the emulator. Without `SIM_WS_URL` the firmware uses mDNS,
which doesn't get answers through esp-emu's relay on macOS. Cards in micro-connector's `tests.json`:
`69420` (competitor), `3004425529` (judge).

## v3 LCD

`SIM_HW=v3 sim/run.sh` (or `--hw v3` for `web.py`/`monkey.py`). The host adds `lcd=<ms>` to
`@state`: how long the emulated LCD has shown something else than lcd_v3's cache (0 = in sync);
`monkey.py` fails a run when that exceeds 4 s. Faults at the LCD model:

| Command | Real-world fault |
|---|---|
| `lcd glitch` | one E edge lost (noise): every later byte pairs the wrong nibbles |
| `lcd drop` | one write lost |
| `lcd reset` | module brownout: back to 8-bit, display off, while the MCU keeps running |
| `lcd cpu <n>` | back-to-back pin writes (no wait between them, < 50 µs apart) count n× closer for the LCD's timing. Through the tap one pin change costs ~2.6 µs of emulated time, ~20× a real C3's bit-banging, so `lcd cpu 20` puts the gap between a character and the next command where real hardware has it (~20 µs, inside the controller's 37–41 µs busy time) |
| `lcd stats` | writes lost to the busy time and the least slack seen since the last `stats` |

The fuzzer injects the first three on v3; `sim/scenarios/lcd-heal.json` checks each heals.

## In a browser

```sh
python3 sim/web.py            # builds, starts esp-emu + stackmat + micro-connector, serves :8000
```

http://127.0.0.1:8000 shows the OLED (or with `--hw v3` the 16×2 LCD) and has the four buttons (hold as long as on the device,
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
`venv/bin/python sim/ble_phone.py` (Bumble, see `sim/patches/README.md`) (a "phone" for BLE provisioning). esp-emu
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
delegate call, Wi-Fi blip, abandoned attempt, solve over the limit, lost answer to a solve) plus
server restarts mid-group and a draining battery. Each step's effect is checked on the device.

micro-connector's `e2e` binary plays FKMTime here (fake server included on `master`
branch: `--mc <checkout>`): it keeps attempts by session id like FKMTime, refuses resends
("already entered"), ends a round at its cutoff ("No attempts left" + reset), DNFs at the time
limit, and turns delegate calls into incidents. A seeded delegate answers each call after a few
seconds (`--delegate-rate` multiplies the call rate): accept (cards confirm it), change time/penalty
(cards confirm it), resolve without cards (reset), extra attempt (reset, the competitor solves
again, the server replaces the original), delete (reset, redo), or only after the device's 5 min
window (the device must ignore it; its resend is refused). The device must show "waiting for
delegate" until the answer and then the decision. The oracle keeps the attempts FKMTime should
hold and after each attempt and at the end compares them with e2e's (station, competitor, judge,
group, number, extra, status, value, penalty, replacement, inspection); a session's sends must
carry the same data. `--devices N` runs N stations (own esp-emu, eFuse MAC from
`sim/make-efuse.py`, stackmat) on one server, round by round, a competitor never at two stations
at once.

```sh
python3 sim/monkey.py --mode competition --seeds 1-6 --jobs 3 --job-base 20 --max-attempts 36 \
    --mc ~/projects/micro-connector
python3 sim/monkey.py --mode competition --seeds 1-4 --delegate-rate 8   # every decision a few times
python3 sim/monkey.py --mode competition --hw v3 --seeds 1-6 --jobs 3 --job-base 20 \
    --mc ~/projects/micro-connector
python3 sim/monkey.py --mode competition --seed 4 --devices 3 --job-base 20 --slip-rate 2
python3 sim/monkey.py --replay-file sim/out/comp-d1/4/day.json            # same day again
```

`--job-base N` moves ports (emulators 20000+10·N.., servers 18100+N..) away from other runs;
`--time-scale` shortens solves (never inspection or holds). Logs: `sim/out/comp-d<N>/<seed>/`
(`day.json` = plan, intended results, mismatches, executed steps; `srv/e2e.jsonl` = every request
with its answer, pushes and delegate decisions). Not covered: sleep (off in sim builds), attendance
stations (the firmware never asks for attendance), BLE.

e2e's control socket (`Server.ctl`, JSON lines): `attempts`, `resolve` (`sessionId`, `status`
resolved / standard / extra_given, optional `value` cs, `penalty`, `noCards`), `delete`,
`drop_reply` (`espId`: the next attempt is entered, its answer lost). Scenarios use it with
optional `"server"` (e.g. `rounds` with attempts/cutoff/limit in cs) and the
steps `resolve [args]` / `delete` (the open incident), `drop_reply`, `expect_db <competitor>
[rows]`; `expect <field> <value> [s]` takes an optional timeout.

Failures land in `sim/out/<seed>/` (actions, firmware log with timestamps, server log). Needs a
built micro-connector (`cargo build --bins` in it; `--mc` path). The `state` sim command prints the
machine-readable line the monkey reads. Not covered: pads (`timer-func`), BLE, Wi-Fi drops (esp-emu
can't drop the link), real display/RFID timing.
