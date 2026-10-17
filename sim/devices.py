"""Device models behind the esp-sim-patches bus tap (pure: no I/O, host-tested by the
self-check at the bottom). They see what the real chips see: pin edges, I2C/SPI bytes."""

# ── HD44780 character LCD (v3) ──────────────────────────────────────────────────────────────

EXEC_US, EXEC_DATA_US, EXEC_CLEAR_US = 37, 41, 1520  # datasheet, fosc = 270 kHz
LINE = 40


class HD44780:
    """The controller as seen from RS/E/DB7..DB4: 8-bit power-on state, 4-bit nibble pairing,
    DDRAM/CGRAM, entry mode/shift, and transfers that start while it is busy are lost (the
    datasheet allows no writes while BF = 1). Faults: `skip_edges` (lost E edge → nibble
    desync), `skip_transfers` (lost write), `reset()` (module brownout), `slow` (×exec times)."""

    def __init__(self):
        self.slow, self.busy_drops, self.last_drop, self.min_slack = 1, 0, None, None
        self.skip_edges = self.skip_transfers = 0
        self.reset()

    def reset(self):
        self.ddram = [bytearray(b" " * LINE) for _ in range(2)]
        self.cgram = bytearray(64)
        self.ac, self.cgram_mode, self.eight_bit, self.two_lines = 0, False, True, False
        self.display_on = self.cursor = self.blink = False
        self.increment, self.shift_on_write, self.shift = True, False, 0
        self.high, self.en, self.busy_until = None, False, 0

    def pins(self, rs: bool, en: bool, db: int, now_us: int):
        """Pin levels at `now_us`; data latches on E's falling edge. db = DB7..DB4 as bits 3..0."""
        falling = self.en and not en
        self.en = en
        if not falling:
            return
        if self.skip_edges:
            self.skip_edges -= 1
            return
        busy = None
        if self.high is None:
            slack = now_us - self.busy_until
            self.min_slack = slack if self.min_slack is None else min(self.min_slack, slack)
            busy = -slack if slack < 0 else None
        if self.eight_bit:
            self.transfer(rs, db << 4 | 0x0F, busy, now_us)  # DB3..DB0 float high (pull-ups)
        elif self.high is not None:
            high, busy = self.high
            self.high = None
            self.transfer(rs, high << 4 | db, busy, now_us)
        else:
            self.high = (db, busy)

    def transfer(self, rs, v, busy, now_us):
        if self.skip_transfers:
            self.skip_transfers -= 1
            return
        if busy is not None:
            self.busy_drops += 1
            self.last_drop = (rs, v, busy)
            return
        if rs:
            self.write(v)
            ex = EXEC_DATA_US
        else:
            ex = self.instruction(v)
        self.busy_until = now_us + ex * self.slow

    def instruction(self, v):
        if v >= 0x80:
            self.cgram_mode, self.ac = False, v & 0x7F
        elif v >= 0x40:
            self.cgram_mode, self.ac = True, v & 0x3F
        elif v >= 0x20:
            self.eight_bit, self.two_lines, self.high = bool(v & 0x10), bool(v & 0x08), None
        elif v >= 0x10:
            right = bool(v & 0x04)
            if v & 0x08:
                self.shift = (self.shift + (LINE - 1 if right else 1)) % LINE
            else:
                self.step(right)
        elif v >= 0x08:
            self.display_on, self.cursor, self.blink = bool(v & 4), bool(v & 2), bool(v & 1)
        elif v >= 0x04:
            self.increment, self.shift_on_write = bool(v & 2), bool(v & 1)
        elif v >= 0x02:
            self.home()
            return EXEC_CLEAR_US
        elif v == 0x01:
            self.ddram = [bytearray(b" " * LINE) for _ in range(2)]
            self.increment = True
            self.home()
            return EXEC_CLEAR_US
        return EXEC_US

    def home(self):
        self.ac, self.cgram_mode, self.shift = 0, False, 0

    def cell(self, ac):
        line, col = (int(ac >= 0x40), ac & 0x3F) if self.two_lines else (0, ac)
        return (line, col) if col < LINE else None

    def step(self, forward):
        if self.cgram_mode:
            self.ac = (self.ac + (1 if forward else -1)) & 0x3F
            return
        wrap = {(True, True, 0x27): 0x40, (True, True, 0x67): 0x00, (True, False, 0x40): 0x27,
                (True, False, 0x00): 0x67, (False, True, 0x4F): 0x00, (False, False, 0x00): 0x4F}
        nxt = wrap.get((self.two_lines, forward, self.ac))
        self.ac = nxt if nxt is not None else (self.ac + (1 if forward else -1)) & 0x7F

    def write(self, v):
        if self.cgram_mode:
            self.cgram[self.ac & 0x3F] = v & 0x1F
            self.ac = (self.ac + 1) & 0x3F
            return
        c = self.cell(self.ac)
        if c:
            self.ddram[c[0]][c[1]] = v
        self.step(self.increment)
        if self.shift_on_write:
            self.shift = (self.shift + (1 if self.increment else LINE - 1)) % LINE

    def visible(self) -> list:
        """The 16×2 character codes shown (blank while the display is off)."""
        out = [bytearray(b" " * 16) for _ in range(2)]
        if self.display_on:
            for row in range(2 if self.two_lines else 1):
                for col in range(16):
                    out[row][col] = self.ddram[row][(col + self.shift) % LINE]
        return out

    def cursor_pos(self):
        if not (self.display_on and (self.cursor or self.blink)) or self.cgram_mode:
            return None
        c = self.cell(self.ac)
        if not c:
            return None
        col = (c[1] + LINE - self.shift) % LINE
        return (c[0], col) if col < 16 else None


class Chain595:
    """74HC595 chain on data/clock/latch: the first bit clocked in ends up in the last
    register; `latched` holds the outputs (bit 0..7 = first register's Q0..Q7, …)."""

    def __init__(self, n=2):
        self.n, self.sr, self.latched, self.data = n, 0, 0, False

    def clock(self):
        self.sr = (self.sr >> 1) | (int(self.data) << (8 * self.n - 1))

    def latch(self):
        self.latched = self.sr

    def reg(self, i):
        """Register i in the order the firmware's AdvancedShiftRegister numbers them."""
        return self.latched >> (8 * (self.n - 1 - i)) & 0xFF


# ── SSD1309 OLED (v4) ───────────────────────────────────────────────────────────────────────

class SSD1309:
    """Page addressing (what the firmware's driver uses) into a 1 KB GRAM."""

    def __init__(self):
        self.gram, self.page, self.col, self.dirty = bytearray(1024), 0, 0, True

    def write(self, b: bytes):
        if len(b) == 4 and b[0] == 0x00 and 0xB0 <= b[1] <= 0xB7 and b[2] <= 0x0F and 0x10 <= b[3] <= 0x17:
            self.page, self.col = b[1] & 7, b[2] | (b[3] & 0x0F) << 4
        elif b[:1] == b"\x40":
            for x in b[1:]:
                if self.col < 128:
                    self.gram[self.page * 128 + self.col] = x
                    self.col += 1
            self.dirty = True

    def pixel(self, x, y):
        return self.gram[y // 8 * 128 + x] >> (y % 8) & 1


# ── BQ27441 fuel gauge (v4) ─────────────────────────────────────────────────────────────────

class BQ27441:
    def __init__(self):
        self.percent, self.ptr, self.control = 100, 0, 0

    def mv(self):
        return 3350 + self.percent * 7.5

    def write(self, b: bytes):
        self.ptr = b[0]
        if b[0] == 0x00 and len(b) >= 3:  # Control() subcommand
            sub = b[1] | b[2] << 8
            self.control = {0x0001: 0x0421}.get(sub, 0)  # DEVICE_TYPE

    def read(self, n: int) -> bytes:
        words = {0x00: self.control, 0x02: 2982, 0x04: int(self.mv()), 0x06: 0, 0x10: 0,
                 0x1C: self.percent, 0x1E: self.percent}
        out = bytearray()
        for i in range(n):
            a = self.ptr + i
            out.append(words.get(a & ~1, 0) >> (8 * (a & 1)) & 0xFF)
        return bytes(out)


# ── MFRC522 reader + MIFARE Classic 1K cards ────────────────────────────────────────────────

def crc_a(data) -> bytes:
    crc = 0x6363
    for b in data:
        b ^= crc & 0xFF
        b = (b ^ (b << 4)) & 0xFF
        crc = (crc >> 8) ^ (b << 8) ^ (b << 3) ^ (b >> 4)
    return bytes([crc & 0xFF, crc >> 8 & 0xFF])


class Card:
    def __init__(self, uid: int):
        self.uid = uid.to_bytes(16, "little")[:4]  # esp-hal-mfrc522's to_u128: little endian
        self.blocks = [bytearray(16) for _ in range(64)]
        self.blocks[0][:4] = self.uid
        for t in range(3, 64, 4):
            self.blocks[t][:] = b"\xff" * 6 + b"\xff\x07\x80\x69" + b"\xff" * 6
        self.state = "idle"  # idle, ready, active, halt
        self.pending_write = None

    def bcc(self):
        return self.uid[0] ^ self.uid[1] ^ self.uid[2] ^ self.uid[3]


COMMAND, COMIRQ, DIVIRQ, ERROR, STATUS2, FIFODATA, FIFOLEVEL, CONTROL, BITFRAMING, COLL = \
    0x01, 0x04, 0x05, 0x06, 0x08, 0x09, 0x0A, 0x0C, 0x0D, 0x0E
CRC_H, CRC_L, VERSION, TXCONTROL = 0x21, 0x22, 0x37, 0x14
IDLE, CALCCRC, TRANSCEIVE, MFAUTHENT, SOFTRESET = 0x0, 0x3, 0xC, 0xE, 0xF


class MFRC522:
    """Register-level model answering the real esp-hal-mfrc522 driver (SPI on v3, I2C 0x28 on
    v4). The chip does Crypto1 itself, so MFAuthent only checks the key against the card's
    sector trailer. `card` is the card in the field (or None); `rf_errors` corrupts the next
    transceives."""

    def __init__(self):
        self.card, self.rf_errors = None, 0
        self.soft_reset()
        self.spi_first, self.spi_addr, self.ptr = True, 0, 0

    def soft_reset(self):
        self.regs = bytearray(64)
        self.regs[TXCONTROL], self.regs[VERSION], self.regs[0x2A] = 0x80, 0x92, 0
        self.fifo = bytearray()

    def read(self, reg: int) -> int:
        if reg == FIFODATA:
            return self.fifo.pop(0) if self.fifo else 0
        if reg == FIFOLEVEL:
            return len(self.fifo)
        return self.regs[reg]

    def write(self, reg: int, v: int):
        if reg == FIFODATA:
            if len(self.fifo) < 64:
                self.fifo.append(v)
        elif reg == FIFOLEVEL:
            if v & 0x80:
                self.fifo.clear()
        elif reg in (COMIRQ, DIVIRQ):
            mask = v & 0x7F
            self.regs[reg] = self.regs[reg] | mask if v & 0x80 else self.regs[reg] & ~mask
        elif reg == COMMAND:
            self.regs[COMMAND] = v & 0x3F
            self.command(v & 0x0F)
        elif reg == BITFRAMING:
            self.regs[reg] = v & 0x7F
            if v & 0x80 and self.regs[COMMAND] & 0x0F == TRANSCEIVE:
                self.transceive(v & 0x07)
        else:
            self.regs[reg] = v

    def command(self, cmd):
        if cmd == SOFTRESET:
            self.soft_reset()
        elif cmd == CALCCRC:
            c = crc_a(self.fifo)
            self.fifo.clear()
            self.regs[CRC_L], self.regs[CRC_H] = c[0], c[1]
            self.regs[DIVIRQ] |= 0x04
        elif cmd == MFAUTHENT:
            f, card = bytes(self.fifo), self.card
            self.fifo.clear()
            ok = (card is not None and card.state == "active" and len(f) == 12
                  and f[8:12] == card.uid and f[1] < 64)
            if ok:
                trailer = card.blocks[f[1] // 4 * 4 + 3]
                key = trailer[:6] if f[0] == 0x60 else trailer[10:16]
                ok = f[2:8] == key
            if ok:
                self.regs[STATUS2] |= 0x08
                self.regs[COMIRQ] |= 0x10
            else:
                self.regs[COMIRQ] |= 0x01  # timer: the card never answers

    def transceive(self, last_bits):
        frame = bytes(self.fifo)
        self.fifo.clear()
        self.regs[ERROR] = 0
        resp = self.picc(frame, last_bits)
        if resp is None:
            self.regs[COMIRQ] |= 0x01
            return
        data, valid_bits = resp
        if self.rf_errors:
            self.rf_errors -= 1
            self.regs[ERROR] = 0x02  # parity error
        self.fifo.extend(data)
        self.regs[CONTROL] = self.regs[CONTROL] & ~0x07 | valid_bits
        self.regs[COMIRQ] |= 0x30

    def picc(self, f: bytes, last_bits):
        """The card's answer (bytes, valid bits of the last byte) or None (no answer)."""
        c = self.card
        if c is None or not f:
            return None
        crypto = bool(self.regs[STATUS2] & 0x08)
        if c.pending_write is not None and crypto:
            block, c.pending_write = c.pending_write, None
            if len(f) == 18 and crc_a(f[:16]) == f[16:]:
                c.blocks[block][:] = f[:16]
                return b"\x0a", 4
            return None
        if last_bits == 7 and f[0] in (0x26, 0x52):  # REQA / WUPA
            if c.state == "idle" or (f[0] == 0x52 and c.state == "halt"):
                c.state = "ready"
                return b"\x04\x00", 0
            return None
        if f[0] == 0x93 and len(f) == 2 and f[1] == 0x20 and c.state == "ready":
            return c.uid + bytes([c.bcc()]), 0
        if f[0] == 0x93 and len(f) == 9 and f[1] == 0x70 and c.state == "ready":
            if f[2:6] == c.uid and crc_a(f[:7]) == f[7:9]:
                c.state = "active"
                return bytes([0x08]) + crc_a(b"\x08"), 0
            return None
        if f[0] == 0x50 and c.state == "active":  # HLTA
            c.state = "halt"
            self.regs[STATUS2] &= ~0x08
            return None
        if c.state != "active" or not crypto or len(f) != 4 or crc_a(f[:2]) != f[2:]:
            return None
        if f[0] == 0x30 and f[1] < 64:  # MF_READ
            data = bytes(c.blocks[f[1]])
            return data + crc_a(data), 0
        if f[0] == 0xA0 and f[1] < 64:  # MF_WRITE, data follows
            c.pending_write = f[1]
            return b"\x0a", 4
        return None

    # bus framing
    def i2c_write(self, b: bytes):
        for v in b[1:]:
            self.write(b[0] & 0x3F, v)
        self.ptr = b[0] & 0x3F

    def i2c_read(self, n):
        return bytes(self.read(self.ptr) for _ in range(n))

    def spi_select(self):
        self.spi_first = True

    def spi(self, mosi: bytes) -> bytes:
        """Bytes while CS is low: address byte (bit 7 = read), then data / next addresses."""
        miso = bytearray()
        for b in mosi:
            if self.spi_first:
                self.spi_first, self.spi_addr = False, b
                miso.append(0)
            elif self.spi_addr & 0x80:
                miso.append(self.read(self.spi_addr >> 1 & 0x3F))
                self.spi_addr = b
            else:
                self.write(self.spi_addr >> 1 & 0x3F, b)
                miso.append(0)
        return bytes(miso)


if __name__ == "__main__":
    # self-check: 4-bit init and text, a command while busy is lost, re-init resyncs
    lcd, t = HD44780(), [0]

    def nib(rs, n, gap):
        t[0] += gap
        lcd.pins(rs, True, n, t[0])
        lcd.pins(rs, False, n, t[0])

    def byte(rs, v, gap):
        nib(rs, v >> 4, gap)
        nib(rs, v & 15, 1)

    def init():
        for n in (3, 3, 3, 2):
            nib(False, n, 5000)
        for v in (0x28, 0x0C, 0x06, 0x01):
            byte(False, v, 5000)

    def text(addr, s, gap=500):
        byte(False, 0x80 | addr, 5000)
        for ch in s.encode():
            byte(True, ch, gap)

    init()
    text(0, "Hello")
    text(0x45, "World")
    assert lcd.visible() == [bytearray(b"Hello           "), bytearray(b"     World      ")]
    text(0, "0123456789abcdef")
    byte(False, 0xC0, 20)  # right after the last character: lost, the text goes off screen
    for ch in b"NEW TEXT":
        byte(True, ch, 500)
    assert lcd.busy_drops == 1 and lcd.visible()[1] == bytearray(b"     World      ")
    lcd.skip_edges = 1
    text(0, "abcd")
    assert lcd.visible()[0][:4] != bytearray(b"abcd")
    init()
    text(0, "abcd")
    assert lcd.visible()[0][:4] == bytearray(b"abcd")

    ch = Chain595(2)
    for bit in [1, 0, 0, 0, 0, 0, 0, 0] + [0, 0, 0, 0, 0, 0, 0, 1]:  # reg 1 then reg 0, LSB first
        ch.data = bool(bit)
        ch.clock()
    ch.latch()
    assert ch.reg(1) == 0x01 and ch.reg(0) == 0x80, (hex(ch.reg(0)), hex(ch.reg(1)))

    assert crc_a(b"\x50\x00") == bytes([0x57, 0xCD])
    rc = MFRC522()
    rc.card = Card(69420)
    rc.write(FIFODATA, 0x26)
    rc.write(COMMAND, TRANSCEIVE)
    rc.write(BITFRAMING, 0x87)
    assert rc.regs[COMIRQ] & 0x30 and bytes(rc.fifo) == b"\x04\x00"
    print("devices self-check ok")
