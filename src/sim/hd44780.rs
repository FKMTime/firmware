//! HD44780 character LCD controller as seen from its pins (pure, host-tested): RS/E/DB7–DB4
//! edges in, DDRAM/CGRAM and the visible 16×2 window out. Models what makes real modules show
//! wrong text: the 8-bit power-on state, 4-bit nibble pairing, and instructions that arrive
//! while the controller is still busy (ignored, as the datasheet allows no writes while BF = 1).

/// Execution times (datasheet, fosc = 270 kHz).
const EXEC_US: u64 = 37;
const EXEC_DATA_US: u64 = 41;
const EXEC_CLEAR_US: u64 = 1520;

pub const COLS: usize = 16;
const LINE: usize = 40;

#[derive(Clone, Copy)]
pub struct Hd44780 {
    pub ddram: [[u8; LINE]; 2],
    pub cgram: [u8; 64],
    /// address counter (DDRAM: 0x00–0x27 / 0x40–0x67, CGRAM: 0x00–0x3F)
    pub ac: u8,
    pub cgram_mode: bool,
    pub eight_bit: bool,
    pub two_lines: bool,
    pub display_on: bool,
    pub cursor: bool,
    pub blink: bool,
    increment: bool,
    shift_on_write: bool,
    /// display shift (characters, 0..40)
    shift: usize,
    /// 4-bit mode: high nibble of the byte in progress, and how early (µs) it came if busy
    high: Option<(u8, Option<u64>)>,
    en: bool,
    busy_until: u64,
    /// instructions/data ignored because they arrived while busy
    pub busy_drops: u32,
    /// the last one ignored: (RS, byte, µs too early)
    pub last_drop: Option<(bool, u8, u64)>,
    /// least time between the controller getting ready and the next transfer (µs, < 0 = early)
    pub min_slack: i64,
    /// fault injection: E falling edges to ignore (desyncs the nibble pairing)
    pub skip_edges: u8,
    /// fault injection: complete transfers to ignore
    pub skip_transfers: u8,
    /// execution times × this (a slower controller, or a faster MCU than the emulated one)
    pub slow: u64,
}

impl Hd44780 {
    /// The state after the internal power-on reset: 8-bit, one line, display off, cleared.
    pub const fn new() -> Self {
        Self {
            ddram: [[b' '; LINE]; 2],
            cgram: [0; 64],
            ac: 0,
            cgram_mode: false,
            eight_bit: true,
            two_lines: false,
            display_on: false,
            cursor: false,
            blink: false,
            increment: true,
            shift_on_write: false,
            shift: 0,
            high: None,
            en: false,
            busy_until: 0,
            busy_drops: 0,
            last_drop: None,
            min_slack: i64::MAX,
            skip_edges: 0,
            skip_transfers: 0,
            slow: 1,
        }
    }

    /// Power-on (brownout) reset; the statistics survive.
    pub fn reset(&mut self) {
        *self = Self {
            busy_drops: self.busy_drops,
            last_drop: self.last_drop,
            min_slack: self.min_slack,
            slow: self.slow,
            ..Self::new()
        };
    }

    /// Pin levels at `now_us`; data is latched on E's falling edge. `db` = DB7..DB4 as bits 3..0.
    pub fn pins(&mut self, rs: bool, en: bool, db: u8, now_us: u64) {
        let falling = self.en && !en;
        self.en = en;
        if !falling {
            return;
        }
        if self.skip_edges > 0 {
            self.skip_edges -= 1;
            return;
        }
        let db = db & 0x0F;
        // a transfer (its first E pulse) that starts while busy is ignored: no writes while BF = 1
        let busy = if self.high.is_none() {
            self.min_slack = self.min_slack.min(now_us as i64 - self.busy_until as i64);
            self.busy_until
                .checked_sub(now_us)
                .filter(|&early| early > 0)
        } else {
            None
        };
        if self.eight_bit {
            // DB3..DB0 of a 4-bit module float high (internal pull-ups)
            self.transfer(rs, db << 4 | 0x0F, busy, now_us);
        } else if let Some((high, busy)) = self.high.take() {
            self.transfer(rs, high << 4 | db, busy, now_us);
        } else {
            self.high = Some((db, busy));
        }
    }

    fn transfer(&mut self, rs: bool, v: u8, busy: Option<u64>, now_us: u64) {
        if self.skip_transfers > 0 {
            self.skip_transfers -= 1;
            return;
        }
        if let Some(early) = busy {
            self.busy_drops = self.busy_drops.saturating_add(1);
            self.last_drop = Some((rs, v, early));
            return;
        }
        let exec = if rs {
            self.write(v);
            EXEC_DATA_US
        } else {
            self.instruction(v)
        };
        self.busy_until = now_us + exec * self.slow;
    }

    fn instruction(&mut self, v: u8) -> u64 {
        match v {
            0x80.. => {
                self.cgram_mode = false;
                self.ac = v & 0x7F;
            }
            0x40.. => {
                self.cgram_mode = true;
                self.ac = v & 0x3F;
            }
            0x20.. => {
                self.eight_bit = v & 0x10 != 0;
                self.two_lines = v & 0x08 != 0;
                self.high = None;
            }
            0x10.. => {
                let right = v & 0x04 != 0;
                if v & 0x08 != 0 {
                    self.shift = (self.shift + if right { LINE - 1 } else { 1 }) % LINE;
                } else {
                    self.step(right);
                }
            }
            0x08.. => {
                self.display_on = v & 0x04 != 0;
                self.cursor = v & 0x02 != 0;
                self.blink = v & 0x01 != 0;
            }
            0x04.. => {
                self.increment = v & 0x02 != 0;
                self.shift_on_write = v & 0x01 != 0;
            }
            0x02.. => {
                self.home();
                return EXEC_CLEAR_US;
            }
            0x01 => {
                self.ddram = [[b' '; LINE]; 2];
                self.increment = true;
                self.home();
                return EXEC_CLEAR_US;
            }
            0x00 => {}
        }
        EXEC_US
    }

    fn home(&mut self) {
        self.ac = 0;
        self.cgram_mode = false;
        self.shift = 0;
    }

    fn write(&mut self, v: u8) {
        if self.cgram_mode {
            self.cgram[usize::from(self.ac & 0x3F)] = v & 0x1F;
            self.ac = (self.ac + 1) & 0x3F;
            return;
        }
        if let Some((line, col)) = self.cell(self.ac) {
            self.ddram[line][col] = v;
        }
        self.step(self.increment);
        if self.shift_on_write {
            self.shift = (self.shift + if self.increment { 1 } else { LINE - 1 }) % LINE;
        }
    }

    /// DDRAM address → (line, column); unused addresses (0x28–0x3F, 0x68–0x7F) hold nothing.
    fn cell(&self, ac: u8) -> Option<(usize, usize)> {
        let (line, col) = if self.two_lines {
            (usize::from(ac >= 0x40), usize::from(ac & 0x3F))
        } else {
            (0, usize::from(ac))
        };
        (col < LINE).then_some((line, col))
    }

    /// Moves the address counter like the controller does (wrapping between the lines).
    fn step(&mut self, forward: bool) {
        if self.cgram_mode {
            self.ac = if forward {
                self.ac + 1
            } else {
                self.ac.wrapping_sub(1)
            } & 0x3F;
            return;
        }
        self.ac = match (self.two_lines, forward, self.ac) {
            (true, true, 0x27) => 0x40,
            (true, true, 0x67) => 0x00,
            (true, false, 0x40) => 0x27,
            (true, false, 0x00) => 0x67,
            (false, true, 0x4F) => 0x00,
            (false, false, 0x00) => 0x4F,
            (_, true, ac) => ac.wrapping_add(1) & 0x7F,
            (_, false, ac) => ac.wrapping_sub(1) & 0x7F,
        };
    }

    /// The visible characters (codes, not glyphs); blank while the display is off.
    pub fn visible(&self) -> [[u8; COLS]; 2] {
        let mut out = [[b' '; COLS]; 2];
        if !self.display_on {
            return out;
        }
        for (row, line) in out.iter_mut().enumerate() {
            if row == 1 && !self.two_lines {
                break;
            }
            for (col, c) in line.iter_mut().enumerate() {
                *c = self.ddram[row][(col + self.shift) % LINE];
            }
        }
        out
    }

    /// Visible cursor position (row, col), if the cursor or blink is on.
    pub fn cursor_pos(&self) -> Option<(usize, usize)> {
        if !(self.display_on && (self.cursor || self.blink)) || self.cgram_mode {
            return None;
        }
        let (line, col) = self.cell(self.ac)?;
        let col = (col + LINE - self.shift) % LINE;
        (col < COLS).then_some((line, col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the pins like ag-lcd-async's 4-bit `send`, one E pulse per nibble, `gap` µs apart.
    struct Bus {
        lcd: Hd44780,
        t: u64,
    }

    impl Bus {
        fn nibble(&mut self, rs: bool, n: u8, gap: u64) {
            self.t += gap;
            self.lcd.pins(rs, true, n, self.t);
            self.lcd.pins(rs, false, n, self.t);
        }
        fn byte(&mut self, rs: bool, v: u8, gap: u64) {
            self.nibble(rs, v >> 4, gap);
            self.nibble(rs, v & 0x0F, 1);
        }
        fn init(&mut self) {
            for n in [3, 3, 3, 2] {
                self.nibble(false, n, 5000);
            }
            for v in [0x28, 0x0C, 0x06, 0x01] {
                self.byte(false, v, 5000);
            }
        }
        fn print(&mut self, addr: u8, s: &str, gap: u64) {
            self.byte(false, 0x80 | addr, 5000);
            for b in s.bytes() {
                self.byte(true, b, gap);
            }
        }
        fn row(&self, r: usize) -> String {
            String::from_utf8(self.lcd.visible()[r].to_vec()).unwrap()
        }
    }

    fn bus() -> Bus {
        let mut b = Bus {
            lcd: Hd44780::new(),
            t: 0,
        };
        b.init();
        b
    }

    #[test]
    fn four_bit_init_and_text() {
        let mut b = bus();
        b.print(0x00, "Hello", 500);
        b.print(0x45, "World", 500);
        assert_eq!(b.row(0), "Hello           ");
        assert_eq!(b.row(1), "     World      ");
        assert_eq!(b.lcd.busy_drops, 0);
    }

    #[test]
    fn power_on_state_is_blank_and_eight_bit() {
        let mut b = Bus {
            lcd: Hd44780::new(),
            t: 0,
        };
        b.print(0x00, "x", 500);
        assert_eq!(b.row(0), " ".repeat(16));
        assert!(b.lcd.eight_bit);
    }

    #[test]
    fn command_while_busy_is_ignored() {
        let mut b = bus();
        b.print(0x00, "0123456789abcdef", 500);
        // the row-1 address right after the last character: the text lands off screen
        b.byte(false, 0xC0, 20);
        for c in b"NEW TEXT        " {
            b.byte(true, *c, 500);
        }
        assert_eq!(b.lcd.busy_drops, 1);
        assert_eq!(b.row(1), " ".repeat(16));
        assert_eq!(b.lcd.ddram[0][16..24], *b"NEW TEXT");
    }

    #[test]
    fn lost_nibble_desyncs_until_reinit() {
        let mut b = bus();
        b.lcd.skip_edges = 1;
        b.print(0x00, "abcd", 500);
        assert_ne!(b.row(0), "abcd            ");
        b.init();
        b.print(0x00, "abcd", 500);
        assert_eq!(b.row(0), "abcd            ");
    }

    #[test]
    fn reinit_without_clear_recovers_from_any_nibble_phase() {
        for skip in [0, 1] {
            let mut b = bus();
            b.print(0x00, "keep", 500);
            b.lcd.skip_edges = skip;
            b.nibble(false, 0x0, 500); // leaves a half byte pending for skip = 0
            for n in [3, 3, 3, 2] {
                b.nibble(false, n, 5000);
            }
            for v in [0x28, 0x0C, 0x06, 0x02] {
                b.byte(false, v, 5000);
            }
            b.print(0x40, "ok", 500);
            assert_eq!(b.row(0), "keep            ", "skip {skip}");
            assert_eq!(b.row(1), "ok              ", "skip {skip}");
        }
    }

    #[test]
    fn brownout_blanks_and_eight_bit_garbles() {
        let mut b = bus();
        b.print(0x00, "before", 500);
        b.lcd.reset();
        b.print(0x00, "after", 500);
        assert!(!b.row(0).contains("after"));
    }

    #[test]
    fn address_wraps_between_lines() {
        let mut b = bus();
        b.print(0x26, "xyz", 500);
        assert_eq!(b.lcd.ddram[0][0x26..], *b"xy");
        assert_eq!(b.lcd.ddram[1][0], b'z');
    }
}
