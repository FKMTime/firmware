#!/usr/bin/env python3
"""Host side of the esp-sim-patches bus tap: esp-emu's UART0 carries the firmware's GPIO, I2C,
SPI and ADC traffic (frames `F5 <type> <len u16> <payload>`, see the patches' README); the
boards here wire the device models of sim/devices.py to it like the real PCBs do.

  python3 sim/bus.py run --hw v3 --bin target/sim-v3/sim.bin --elf ...   # interactive device
  python3 sim/bus.py nvs <image.bin> [ws_url]                           # seed Wi-Fi into NVS

Interactive commands: btn <1-4> [ms] | card <uid> | bat <0-100> | screen | lcd|oled raw |
lcd glitch|drop|reset|cpu <n>|stats | rfid error [n] | i2c nack <addr hex> [n]; anything else
(state, wifi off|on) goes to the firmware's console.
"""
import argparse
import os
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

from devices import BQ27441, MFRC522, SSD1309, Card, Chain595, HD44780

ROOT = Path(__file__).resolve().parent.parent
EMU = shutil.which("esp-emu") or str(Path.home() / ".local/bin/esp-emu")
MARK = 0xF5
FRAME_TYPES, MAX_FRAME = b"GQISAP", 520  # what the tap sends; its largest is a full G batch
CARD_IN_FIELD_S = 3.0
NVS_OFFSET, NVS_SIZE = 0x9000, 0x4000  # partitions.csv
NVS_TOOL = os.environ.get("SIM_NVS_TOOL", str(Path.home() / "projects/esp-nvs/target/debug/esp-nvs-partition-tool"))


# ── boards ──────────────────────────────────────────────────────────────────────────────────

class Board:
    """Device models + the wiring of one hardware revision. All calls hold `lock`."""

    def __init__(self, hw: str, emit):
        self.hw, self.emit, self.lock = hw, emit, threading.Lock()
        self.tap = None
        self.emu_us = 0  # the firmware's clock in its last timed frame (G, Q)
        self.raw, self.last_print, self.printed = False, 0.0, None
        self.held = {}  # button 1-4 → (start µs, end µs)
        self.battery = 100
        self.rfid = MFRC522()
        self.card_since = 0.0
        self.cards = {}  # uid → Card: what was written to a card stays on it
        self.nack = {}  # i2c address → NACKs left
        if hw == "v3":
            self.chain, self.lcd, self.levels, self.cs_low = Chain595(2), HD44780(), {}, False
            # the LCD's clock: emulated µs with back-to-back pin writes (< 50 µs apart: no wait
            # in between) `cpu`× closer, as fast as a real C3 bit-bangs them
            self.cpu, self.lcd_t, self.last_t = 1, 0.0, 0
        else:
            self.oled, self.gauge = SSD1309(), BQ27441()

    def saw_time(self, us: int):
        self.emu_us = us

    def held_mask(self):
        """Buttons still held as the firmware sees it (v3: by its clock in the last frame;
        v4: pulses it hasn't reported over)."""
        if self.hw == "v4":
            return sum(1 << (b - 1) for b in self.held)
        return sum(1 << (b - 1) for b, (s, e) in self.held.items() if s <= self.emu_us < e)

    def pulse_end(self, pin: int):
        self.held.pop(pin + 1, None)

    def power_on(self):
        """Power cycle: the peripherals reset with the MCU."""
        self.rfid.soft_reset()
        if self.hw == "v3":
            self.lcd.reset()
            self.chain = Chain595(2)

    # bus traffic
    def gpio_batch(self, t: int, p: bytes) -> int:
        """A `G` frame's changes (`p` = its payload, `t` = the first change's µs); returns the
        last change's µs. v3: GPIO10/21/1 = shift register data/clock/latch."""
        if self.hw != "v3":
            return t + sum(p[5::2])
        chain, levels, cpu = self.chain, self.levels, self.cpu
        dt = t - self.last_t
        for i in range(4, len(p) - 1, 2):
            if i > 4:
                dt = p[i + 1]
                t += dt
            if dt >= 0:  # (the clock restarts when the firmware reboots)
                self.lcd_t += dt / cpu if dt < 50 else dt
            pin, high = p[i] & 0x7F, p[i] >> 7
            if pin == 10:
                chain.data = high
            elif high and not levels.get(pin):
                if pin == 21:
                    chain.sr = (chain.sr >> 1) | (chain.data << 15)  # Chain595.clock, inlined
                elif pin == 1:
                    chain.latch()
                    self.latched(self.lcd_t)
            levels[pin] = high
        self.last_t = t
        return t

    def latched(self, t_us):
        # LCD register: ShifterPin bit b = mask 0x80 >> b: CS, backlight, RS, E, DB4..DB7
        out = self.chain.reg(1)
        db = (out & 1) << 3 | (out >> 1 & 1) << 2 | (out >> 2 & 1) << 1 | (out >> 3 & 1)
        self.lcd.pins(bool(out & 0x20), bool(out & 0x10), db, t_us)
        cs_low = not out & 0x80
        if cs_low and not self.cs_low:
            self.rfid.spi_select()
        self.cs_low = cs_low

    def inputs(self, now_us: int) -> int:
        """Input levels for a `Q` query (v3: the button matrix behind GPIO3)."""
        if self.hw != "v3":
            return 0
        rows = self.chain.reg(0)
        pressed = any(rows & (0x80 >> (b - 1)) for b, (s, e) in self.held.items() if s <= now_us < e)
        return int(pressed) << 3

    def i2c(self, addr: int, ops: list):
        """ops: [("W", bytes) | ("R", n)] → (status, read bytes)."""
        if self.nack.get(addr):
            self.nack[addr] -= 1
            return 1, b""
        dev = {0x3C: "oled", 0x55: "gauge", 0x28: "rfid"}.get(addr) if self.hw == "v4" else None
        if dev is None:
            return 1, b""
        out, pending = bytearray(), bytearray()
        for kind, x in ops + [("end", None)]:
            if kind != "W" and pending:
                {"oled": self.oled.write, "gauge": self.gauge.write, "rfid": self.rfid.i2c_write}[dev](bytes(pending))
                pending.clear()
            if kind == "W":
                pending += x
            elif kind == "R":
                out += self.gauge.read(x) if dev == "gauge" else self.rfid.i2c_read(x) if dev == "rfid" else bytes(x)
        return 0, bytes(out)

    def spi(self, mosi: bytes) -> bytes:
        if self.hw == "v3" and self.cs_low:
            return self.rfid.spi(mosi)
        return bytes(len(mosi))

    def adc(self, channel: int) -> int:
        """v3 battery divider on ADC1 channel 2, as the firmware's calibration reads it."""
        if self.hw != "v3" or channel != 2:
            return 0
        curve = [(3350, 0), (3400, 13), (3450, 19), (3500, 25), (3550, 31), (3600, 38),
                 (3700, 50), (3800, 63), (3900, 75), (4000, 88), (4100, 100)]
        mv = curve[-1][0]
        for (v1, p1), (v2, p2) in zip(curve, curve[1:]):
            if self.battery <= p2:
                mv = v1 + (self.battery - p1) * (v2 - v1) / (p2 - p1)
                break
        return int((mv + 3 - 66.6103) / 1.69874) + 1  # inverse of utils::battery::adc_to_mv

    # host commands
    def command(self, line: str) -> bool:
        w = line.split()
        if not w:
            return False
        num = lambda i, d=None: int(w[i], 0) if len(w) > i and w[i].lstrip("-").isdigit() else d
        if w[0] == "btn" and num(1) in (1, 2, 3, 4):
            b, ms = num(1), num(2, 150)
            if self.hw == "v4":  # GPIO0..3 high while pressed, timed by the tap
                if ms:
                    self.held[b] = None
                else:
                    self.held.pop(b, None)
                if self.tap:
                    self.tap.send(b"g", struct.pack("<BBI", b - 1, 1 if ms else 0, ms * 1000))
            else:  # from the firmware's clock as of its last frame (one every ~1 ms on v3)
                self.held[b] = (self.emu_us, self.emu_us + ms * 1000)
        elif w[0] == "card" and num(1) is not None:
            uid = num(1) & ((1 << 56) - 1)
            self.rfid.card = self.cards.setdefault(uid, Card(uid))
            self.rfid.card.state = "idle"
            self.card_since = time.monotonic()
        elif w[0] == "bat" and num(1) is not None and 0 <= num(1) <= 100:
            self.battery = num(1)
            if self.hw == "v4":
                self.gauge.percent = num(1)
        elif w[0] == "screen":
            self.print_display(force=True)
        elif w[0] in ("oled", "lcd") and w[1:2] == ["raw"]:
            self.raw = True
            self.print_display(force=True)
        elif w[0] == "lcd" and self.hw == "v3" and len(w) > 1:
            if w[1] == "glitch":
                self.lcd.skip_edges = 1  # a lost E edge: later bytes pair the wrong nibbles
            elif w[1] == "drop":
                self.lcd.skip_transfers = 1  # one write lost
            elif w[1] == "reset":
                self.lcd.reset()  # module brownout while the MCU runs on
            elif w[1] == "cpu" and num(2):
                self.cpu = num(2)
            elif w[1] == "stats":
                self.emit(f"lcd: {self.lcd.busy_drops} writes ignored while busy (last: {self.lcd.last_drop} "
                          f"= (RS, byte, us early)), least slack {self.lcd.min_slack} us (since the last stats)")
                self.lcd.busy_drops, self.lcd.last_drop, self.lcd.min_slack = 0, None, None
            else:
                return False
        elif w[0] == "tap" and w[1:2] == ["stats"] and self.tap:
            r = sorted(self.tap.rtt) or [0]
            self.emit(f"tap: {self.tap.stats['frames']} frames, {self.tap.stats['gpio']} pin changes "
                      f"({self.tap.gap_sum / max(1, self.tap.gaps):.2f} us apart in a batch); query round trip "
                      f"median {r[len(r) // 2]} us, p90 {r[len(r) * 9 // 10]} us, max {r[-1]} us ({len(r)} samples)")
        elif w[0] == "rfid" and w[1:2] == ["error"]:
            self.rfid.rf_errors = num(2, 1)
        elif w[0] == "i2c" and w[1:2] == ["nack"] and len(w) > 2:
            self.nack[int(w[2], 16)] = num(3, 1)
        else:
            return False
        return True

    # display
    def tick(self):
        if self.rfid.card and time.monotonic() - self.card_since > CARD_IN_FIELD_S:
            self.rfid.card = None
        self.print_display()

    def frame(self):
        if self.hw == "v3":
            bl = bool(self.chain.reg(1) & 0x40)
            return (tuple(bytes(r) for r in self.lcd.visible()), self.lcd.cursor_pos(), bl)
        return bytes(self.oled.gram)

    def print_display(self, force=False):
        now = time.monotonic()
        if not force and now - self.last_print < (0.02 if self.raw else 0.2):
            return
        f = self.frame()
        if f == self.printed and not force:
            return
        self.printed, self.last_print = f, now
        if self.hw == "v4":
            if self.raw:
                return self.emit("@oled " + f.hex())
            self.emit("┌" + "─" * 128 + "┐")
            for row in range(32):
                self.emit("│" + "".join(" ▀▄█"[self.oled.pixel(x, 2 * row) | self.oled.pixel(x, 2 * row + 1) << 1]
                                         for x in range(128)) + "│")
            return self.emit("└" + "─" * 128 + "┘")
        rows, cur, bl = f
        if self.raw:
            c = "-" if cur is None else f"{cur[0]},{cur[1]}"
            return self.emit(f"@lcd {rows[0].hex()}{rows[1].hex()} bl={int(bl)} cur={c} cg={self.lcd.cgram.hex()}")
        glyph = lambda c: chr(c) if 0x20 <= c < 0x7F else "▯" if c < 8 else "▒"
        self.emit("┌" + "─" * 16 + "┐" + ("" if bl else " (backlight off)"))
        for r in rows:
            self.emit("│" + "".join(glyph(c) for c in r) + "│")
        self.emit("└" + "─" * 16 + "┘")


# ── the tap connection ──────────────────────────────────────────────────────────────────────

class Tap:
    """Talks to the firmware over esp-emu's UART0 socket; non-frame bytes (ROM text) are
    passed on as lines."""

    def __init__(self, port: int, board: Board, on_text):
        self.port, self.board, self.on_text = port, board, on_text
        self.sock, self.alive, self.wlock = None, True, threading.Lock()
        self.pending = []  # frames sent before the connection is up
        self.stats = {"frames": 0, "gpio": 0}
        # emulated µs: a Q query until the firmware's next pin write (= the reply's round trip
        # plus a few instructions), and the spacing of pin writes inside a batch
        self.rtt, self.last_q, self.gap_sum, self.gaps = [], None, 0, 0
        board.tap = self
        threading.Thread(target=self.run, daemon=True).start()

    def send(self, ty: bytes, payload: bytes = b""):
        data = bytes([MARK]) + ty + struct.pack("<H", len(payload)) + payload
        with self.wlock:
            if self.sock is None:
                self.pending.append(data)
                return
            try:
                self.sock.sendall(data)
            except OSError:
                pass

    def run(self):
        while self.alive and self.sock is None:
            try:
                sock = socket.create_connection(("127.0.0.1", self.port), timeout=1)
            except OSError:
                time.sleep(0.05)
                continue
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            sock.settimeout(None)
            with self.wlock:
                self.sock = sock
                for data in self.pending:
                    sock.sendall(data)
                self.pending = []
        buf, text = bytearray(), bytearray()
        t_hi, t_last = 0, 0
        while self.alive:
            try:
                chunk = self.sock.recv(65536)
            except OSError:
                break
            if not chunk:
                break
            buf += chunk
            while buf:
                if buf[0] != MARK:
                    i = buf.find(MARK)
                    i = len(buf) if i < 0 else i
                    text += buf[:i]
                    del buf[:i]
                    while b"\n" in text:
                        line, _, rest = text.partition(b"\n")
                        self.on_text(line.decode(errors="replace").rstrip("\r"))
                        text = bytearray(rest)
                    continue
                if len(buf) < 4:
                    break
                n = struct.unpack_from("<H", buf, 2)[0]
                if buf[1] not in FRAME_TYPES or n > MAX_FRAME:
                    # not a frame (a reset cut the last one short): resync on the next marker
                    text.append(buf.pop(0))
                    continue
                if len(buf) < 4 + n:
                    break
                ty, payload = buf[1], bytes(buf[4:4 + n])
                del buf[:4 + n]
                self.stats["frames"] += 1

                def unwrap(t32):  # µs counter (u32) → 64-bit
                    nonlocal t_hi, t_last
                    if t32 < t_last and t_last - t32 > 1 << 31:
                        t_hi += 1 << 32
                    t_last = t32
                    return t_hi + t32

                seq = b""
                if ty in b"QISA" and payload:  # requests start with their sequence byte
                    seq, payload = payload[:1], payload[1:]
                with self.board.lock:
                    reply = self.frame(ty, payload, unwrap)
                if reply is not None:
                    self.send(b"r", seq + reply)

    def frame(self, ty, p, unwrap):
        b = self.board
        if ty == ord("G"):
            t = unwrap(struct.unpack_from("<I", p)[0])
            if self.last_q is not None:
                self.rtt = (self.rtt + [t - self.last_q])[-2000:]
                self.last_q = None
            self.gap_sum += sum(p[i + 1] for i in range(6, len(p) - 1, 2))
            self.gaps += max(0, (len(p) - 6) // 2)
            t = b.gpio_batch(t, p)
            self.stats["gpio"] += (len(p) - 4) // 2
            b.saw_time(t)
        elif ty == ord("Q"):
            now = unwrap(struct.unpack_from("<I", p)[0])
            self.last_q = now
            b.saw_time(now)
            return b"\x00" + struct.pack("<I", b.inputs(now))
        elif ty == ord("I"):
            addr, i, ops = p[0], 1, []
            while i + 3 <= len(p):
                kind, n = chr(p[i]), struct.unpack_from("<H", p, i + 1)[0]
                i += 3
                if kind == "W":
                    ops.append(("W", p[i:i + n]))
                    i += n
                else:
                    ops.append(("R", n))
            status, data = b.i2c(addr, ops)
            return bytes([status]) + data if any(k == "R" for k, _ in ops) else None
        elif ty == ord("S"):
            miso = b.spi(p[1:])
            return b"\x00" + miso if p[0] else None
        elif ty == ord("A"):
            return b"\x00" + struct.pack("<H", b.adc(p[0]))
        elif ty == ord("P"):
            b.pulse_end(p[0])
        return None

    def stop(self):
        self.alive = False
        if self.sock:
            try:
                self.sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


# ── esp-emu with the tap ────────────────────────────────────────────────────────────────────

class Emu:
    """esp-emu running a sim image with the bus tap on a UART0 socket (`ports["tap"]`), which
    also carries the firmware's log text and console input. `send` takes device commands
    (card, btn, …) itself and passes the rest (state, wifi) to the firmware console."""

    def __init__(self, bin_: Path, elf: Path, ports: dict, log: Path, efuse: Path = None, hw: str = "v4"):
        self.log = open(log, "w", buffering=1)
        self.lines, self.lock = [], threading.Lock()
        self.lcd_bad_since, self.lcd_sampled = None, 0.0
        extra = ["--efuse", str(efuse)] if efuse else []
        # esp-emu serves sockets between instruction batches: smaller batches, quicker replies
        extra += ["--batch-size", os.environ.get("SIM_BATCH", "5000")]
        if os.environ.get("SIM_BLE"):  # e.g. tcp:localhost:9544, a Bumble controller
            extra += ["--ble-hci", os.environ["SIM_BLE"]]
        self.proc = subprocess.Popen(
            [EMU, "--chip", "esp32c3", "--firmware", str(bin_), "--elf", str(elf),
             "--net", f"user,hostfwd=tcp:127.0.0.1:{ports['http']}-:80",
             "--uart1-tcp", f"127.0.0.1:{ports['uart1']}",
             "--uart-tcp", f"127.0.0.1:{ports['tap']}",
             "--console", "usb-jtag",  # keeps esp-println on UART0 (no USB host seen)
             "--control-tcp", f"127.0.0.1:{ports['control']}", *extra],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)
        self.control = ports["control"]
        self.board = Board(hw, self.add)
        self.tap = Tap(ports["tap"], self.board, lambda l: self.add(l))
        threading.Thread(target=self.read, daemon=True).start()
        threading.Thread(target=self.ticker, daemon=True).start()

    def add(self, line: str):
        now = time.monotonic()
        if line.startswith("@state "):
            line = self.annotate(line)
        elif "INFO - Hardware Rev:" in line:  # booted: no press survives a reset
            with self.board.lock:
                self.board.held.clear()
        self.log.write(f"{now:.3f} {line}\n")
        with self.lock:
            self.lines.append((now, line))

    def annotate(self, line: str) -> str:
        """Adds what only the host knows: held buttons, and on v3 how long (ms) the LCD has
        shown something else than the firmware wrote (`lcdtext`)."""
        kv = dict(x.split("=", 1) for x in line[7:].split() if "=" in x)
        with self.board.lock:
            line += f" held={self.board.held_mask()}"
            if "lcdtext" in kv and self.board.hw == "v3":
                shown = b"".join(bytes(r) for r in self.board.lcd.visible()).hex()
                now = time.monotonic()
                if shown == kv["lcdtext"]:
                    self.lcd_bad_since = None
                elif self.lcd_bad_since is None or now - self.lcd_sampled > 3.0:
                    # only back-to-back samples tell it stayed wrong (scrolling rewrites briefly)
                    self.lcd_bad_since = now
                self.lcd_sampled = now
                line += f" lcd={0 if self.lcd_bad_since is None else int((now - self.lcd_bad_since) * 1000)}"
        return line

    def read(self):
        buf = b""
        while True:
            chunk = self.proc.stdout.read(4096)
            if not chunk:
                break
            buf += chunk
            *lines, buf = buf.split(b"\n")
            for raw in lines:
                self.add(raw.decode(errors="replace").rstrip("\r"))

    def ticker(self):
        while self.proc.poll() is None:
            with self.board.lock:
                self.board.tick()
            time.sleep(0.02)

    def send(self, line: str):
        with self.board.lock:
            if self.board.command(line):
                return
        self.tap.send(b"c", (line + "\n").encode())  # firmware console (state, wifi)

    def since(self, t0: float) -> list:
        with self.lock:
            return [l for t, l in self.lines if t >= t0]

    def control_cmd(self, cmd: str) -> str:
        with socket.create_connection(("127.0.0.1", self.control), timeout=5) as s:
            s.sendall(cmd.encode() + b"\n")
            return s.makefile().readline().strip()

    def power_cycle(self):
        with self.board.lock:
            self.board.power_on()
        self.control_cmd("reset")

    def reseed(self, nvs_bin: Path):
        """Writes the seeded NVS partition back (after `Reset settings`) and reboots."""
        self.control_cmd(f"write-region {NVS_OFFSET:#x} {nvs_bin}")
        self.power_cycle()

    def stop(self):
        self.tap.stop()
        self.proc.send_signal(signal.SIGINT)
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# ── Wi-Fi seeding ───────────────────────────────────────────────────────────────────────────

def nvs_partition(ws_url: str, out: Path) -> Path:
    """The NVS partition with the emulator's Wi-Fi (myssid/mypassword) and the server url, as
    the wifimanager stores it after provisioning (`SIM_PROVISION=1` builds skip it)."""
    data = '{"mdns":false,"ws_url":"%s"}' % ws_url if ws_url else '{"mdns":true,"ws_url":null}'
    with tempfile.TemporaryDirectory() as d:
        Path(d, "wifi.txt").write_text('{"ssid":"myssid","psk":"mypassword","data":%s}' % data)
        Path(d, "nvs.csv").write_text("key,type,encoding,value\nwifimanager,namespace,,\nWIFI_SETUP,file,string,wifi.txt\n")
        subprocess.run([NVS_TOOL, "generate", str(Path(d, "nvs.csv")), str(out), "--size", hex(NVS_SIZE)],
                       check=True, stdout=subprocess.DEVNULL)
    return out


def seed_image(image: Path, ws_url: str) -> Path:
    """Splices the seeded NVS partition into a merged flash image; returns the partition file."""
    part = nvs_partition(ws_url, image.with_suffix(".nvs"))
    with open(image, "r+b") as f:
        f.seek(NVS_OFFSET)
        f.write(part.read_bytes())
    return part


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--hw", choices=["v3", "v4"], default="v4")
    r.add_argument("--bin", required=True)
    r.add_argument("--elf", required=True)
    r.add_argument("--http", type=int, default=18080)
    r.add_argument("--uart1", type=int, default=5556)
    r.add_argument("--control", type=int, default=5557)
    r.add_argument("--tap", type=int, default=5558)
    n = sub.add_parser("nvs")
    n.add_argument("image")
    n.add_argument("ws_url", nargs="?", default="")
    a = p.parse_args()
    if a.cmd == "nvs":
        seed_image(Path(a.image), a.ws_url)
        return
    log = ROOT / "target" / f"sim-{a.hw}.log"
    emu = Emu(Path(a.bin), Path(a.elf), dict(http=a.http, uart1=a.uart1, control=a.control, tap=a.tap),
              log, hw=a.hw)
    seen = 0

    def echo():
        nonlocal seen
        while emu.proc.poll() is None:
            with emu.lock:
                new = emu.lines[seen:]
                seen += len(new)
            for _, l in new:
                print(l, flush=True)
            time.sleep(0.05)

    threading.Thread(target=echo, daemon=True).start()
    print(__doc__.split("Interactive commands:")[1].strip(), flush=True)
    try:
        for line in sys.stdin:
            emu.send(line.strip())
    except KeyboardInterrupt:
        pass
    finally:
        emu.stop()


if __name__ == "__main__":
    main()
