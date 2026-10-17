# esp-emu patches

Emulator-only patches that let esp-hal firmware run in [esp-emu](https://github.com/espressif/esp-emulator)
(Espressif, beta, closed source). Never use the patched crates for real hardware.

| Patch | Why |
|---|---|
| `esp-hal-1.2.2.patch` | esp-emu never sets the interrupt matrix status registers: bounded wait in `SoftwareInterrupt::raise`, and `handle_interrupts` calls every mapped handler when the status reads empty |<br>Bus tap (`src/sim_tap.rs`, only with `ESP_SIM_TAP` set at build time): GPIO, I2C, SPI and ADC go to a host over UART0, see below |
| `esp-radio-1.0.0-beta.1.patch` | BLE: the controller bring-up and the VHCI host API go through the ESP-IDF names esp-emu intercepts (`esp_bt_controller_init/enable/deinit`, `esp_vhci_host_*`), `coex_init` runs outside the stubbed init, the callback struct lives in RAM, and a sent packet frees its buffer at once (esp-emu never calls `notify_host_send_available`) |

`sim/run.sh` applies them itself: `apply.sh esp-hal-1.2.2` copies that crate from the cargo registry
into `target/sim-patches/`, applies its patch (only again when the patch changed) and prints the
path, which goes into `cargo build --config 'patch.crates-io.esp-hal.path="…"'`. A new esp-hal
version needs its own `<crate>-<version>.patch` (`diff -ruN` of the registry copy and the patched one).

The BLE scripts need Bumble (`python3.14 -m venv venv && venv/bin/pip install 'bumble>=0.0.220,<0.1'`).

Known esp-emu gap: with the esp-radio patch the controller is intercepted and HCI commands reach
esp-emu, but it never calls the registered VHCI callbacks back, so no HCI events are delivered.

## Bus tap (esp-hal)

Built with `ESP_SIM_TAP=1` in the environment, the patched esp-hal hands bus traffic to a host
program that models the devices, so the firmware runs its real drivers. Without the variable the
patched crate behaves like the unpatched one. Hooks (one line each): `AnyPin::set_output_high` /
`is_input_high` (GPIO), the I2C master's `transaction_impl[_async]`, the SPI master's and SPI DMA
`read/write/transfer/transfer_in_place[_async]`, the RISC-V ADC `read_oneshot`.

Transport: esp-emu's UART0 (`--uart-tcp HOST:PORT`, with `--console usb-jtag` so esp-println stays
on UART0), through the ROM's `uart_tx_one_char`/`uart_rx_one_char` (esp-emu intercepts those; the
FIFO registers don't reach the socket). Frames in both directions are `F5 <type> <len u16 LE>
<payload>`; every other byte is text (ROM, bootloader and esp-println logs share the line).

Every request (`Q`, `I`, `S`, `A`) starts with a sequence byte; the reply repeats it, and a
reply to an earlier request that timed out is dropped.

| From the firmware | Payload | Reply |
|---|---|---|
| `G` GPIO outputs | `[t0 µs u32]` then `(pin \| level << 7, µs since previous)` per change; a batch is sent before any other frame, when full (255 changes) or when 255 µs passed | – |
| `Q` input query | `[now µs u32]`, sent on an input read when outputs changed since the last read (a matrix scan's answer depends on them) | `[0][levels u32]` |
| `I` I2C | `[addr]` then `(W, len u16, data)` / `(R, len u16)` per operation | only with reads: `[status][read bytes]` |
| `S` SPI | `[read? u8][MOSI bytes]` | only if read: `[status][MISO bytes]` |
| `A` ADC | `[channel]` | `[status][value u16]` |
| `P` pulse ended | `[pin]`: an input pulse from a `g` frame ran out | – |

| From the host | Payload |
|---|---|
| `r` | `[seq][status][data]`: the reply to the request in flight; status 0 = ok, 1 = NACK address, 2 = NACK data, else error |
| `g` | `(pin, level, µs u32)`…: the input reads `level`, after `µs` (0 = never) the other level again, timed from when the firmware sees it |
| `c` | console text for the firmware, read with `esp_hal::sim_tap::console(&mut buf)` |

Input reads without output changes in between don't wait for the host. A request waits up to
500 ms for its reply (then: I2C `Timeout`, SPI `Unknown`, ADC 0). No GPIO interrupts.
