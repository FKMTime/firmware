//! v3 16×2 character LCD text buffer (pure, host-tested): lines up to 80 chars that scroll
//! back and forth, and a copy of what the LCD shows so only changed characters are written.

pub const COLS: usize = 16;
const ROWS: usize = 2;
const LINE: usize = 80;
/// scroll ticks to wait at each end
const SCROLL_WAIT: usize = 3;

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct LcdText {
    lines: [[u8; LINE]; ROWS],
    sizes: [usize; ROWS],
    /// what the LCD shows (0xFF = unknown → rewritten)
    shown: [[u8; COLS]; ROWS],
    scroll: usize,
    wait: usize,
    dir: isize,
}

impl LcdText {
    pub const fn new() -> Self {
        Self {
            lines: [[b' '; LINE]; ROWS],
            sizes: [0; ROWS],
            shown: [[0xFF; COLS]; ROWS],
            scroll: 0,
            wait: 0,
            dir: 0,
        }
    }

    /// Blank both lines (the LCD is updated by the next `changes`).
    pub fn clear(&mut self) {
        self.lines = [[b' '; LINE]; ROWS];
        self.sizes = [0; ROWS];
    }

    /// Writes `text` to `row`. With `pad` the visible line is replaced, otherwise the text is
    /// laid over it. Non-ASCII chars become `?` (the HD44780 has no glyphs for them), text
    /// longer than a line is cut.
    pub fn print(&mut self, row: usize, text: &str, align: Align, pad: bool) {
        let Some(line) = self.lines.get_mut(row) else {
            return;
        };
        let mut chars = [b' '; LINE];
        let mut len = 0;
        for (dst, c) in chars.iter_mut().zip(text.chars()) {
            *dst = if c.is_ascii() && !c.is_ascii_control() {
                c as u8
            } else {
                b'?'
            };
            len += 1;
        }

        let x = match align {
            _ if len >= COLS => 0,
            Align::Left => 0,
            Align::Center => (COLS - len) / 2,
            Align::Right => COLS - len,
        };
        if pad && len < COLS {
            line[..COLS].fill(b' ');
            self.sizes[row] = COLS;
        } else {
            self.sizes[row] = len;
        }
        line[x..x + len].copy_from_slice(&chars[..len]);

        self.scroll = 0;
        self.wait = SCROLL_WAIT - 1;
    }

    /// One scroll tick (every 500 ms); `true` if the visible text moved.
    pub fn scroll_step(&mut self) -> bool {
        let max_scroll = self
            .sizes
            .iter()
            .max()
            .map_or(0, |s| s.saturating_sub(COLS));
        if max_scroll == 0 {
            return false;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return false;
        }

        self.scroll = self.scroll.saturating_add_signed(self.dir).min(max_scroll);
        if self.scroll == 0 || self.scroll == max_scroll {
            self.wait = SCROLL_WAIT - 1;
            self.dir = if self.scroll == 0 { 1 } else { -1 };
        }
        true
    }

    /// Forget what the LCD shows: the next `changes` rewrites everything.
    pub fn invalidate(&mut self) {
        self.shown = [[0xFF; COLS]; ROWS];
    }

    /// Calls `write(row, col, text)` for the changed run of each row and marks it shown.
    pub fn changes(&mut self, mut write: impl FnMut(u8, u8, &str)) {
        for row in 0..ROWS {
            let offset = self.scroll.min(self.sizes[row].saturating_sub(COLS));
            let visible = &self.lines[row][offset..offset + COLS];
            let shown = &mut self.shown[row];

            let Some(start) = visible.iter().zip(shown.iter()).position(|(a, b)| a != b) else {
                continue;
            };
            let end = COLS
                - visible
                    .iter()
                    .zip(shown.iter())
                    .rev()
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
            // only ASCII is ever stored, so this can't fail
            if let Ok(text) = core::str::from_utf8(&visible[start..end]) {
                write(row as u8, start as u8, text);
            }
            shown[start..end].copy_from_slice(&visible[start..end]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{string::String, vec::Vec};

    fn flush(t: &mut LcdText) -> Vec<(u8, u8, String)> {
        let mut v = Vec::new();
        t.changes(|r, c, s| v.push((r, c, s.into())));
        v
    }

    #[test]
    fn writes_only_changed_runs() {
        let mut t = LcdText::new();
        t.print(0, "12.34", Align::Center, true);
        t.print(1, "Inspection", Align::Center, true);
        assert_eq!(
            flush(&mut t),
            [
                (0, 0, "     12.34      ".into()),
                (1, 0, "   Inspection   ".into())
            ]
        );
        assert!(flush(&mut t).is_empty());

        t.print(0, "12.35", Align::Center, true);
        assert_eq!(flush(&mut t), [(0, 9, "5".into())]);

        // overlay without pad keeps the rest of the line
        t.print(0, ">", Align::Right, false);
        assert_eq!(flush(&mut t), [(0, 15, ">".into())]);
    }

    #[test]
    fn sanitizes_and_cuts() {
        let mut t = LcdText::new();
        t.print(0, "zażółć", Align::Left, true);
        t.print(5, "out of range row", Align::Left, true);
        t.print(1, &"x".repeat(200), Align::Left, true);
        let out = flush(&mut t);
        assert_eq!(out[0], (0, 0, "za????          ".into()));
        assert_eq!(out[1], (1, 0, "x".repeat(16)));
    }

    #[test]
    fn long_lines_scroll_back_and_forth() {
        let mut t = LcdText::new();
        t.print(0, "Signing | Submit To Exit", Align::Left, true); // 24 chars
        flush(&mut t);
        let mut offsets = Vec::new();
        for _ in 0..30 {
            if t.scroll_step() {
                offsets.push(t.scroll);
            }
        }
        assert_eq!(&offsets[..10], [0, 1, 2, 3, 4, 5, 6, 7, 8, 7]);
        assert!(!flush(&mut t).is_empty());
    }

    #[test]
    fn invalidate_rewrites_everything() {
        let mut t = LcdText::new();
        t.print(0, "abc", Align::Left, true);
        flush(&mut t);
        t.invalidate();
        assert_eq!(
            flush(&mut t),
            [(0, 0, "abc             ".into()), (1, 0, " ".repeat(16))]
        );
    }
}
