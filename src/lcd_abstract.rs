use embedded_hal::delay::DelayNs;
use hd44780_driver::{
    bus::DataBus, charset::CharsetWithFallback, memory_map::DisplayMemoryMap, HD44780,
};

pub enum PrintAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug)]
pub enum LcdError {
    OutOfRange,
    Other,
}

pub struct LcdAbstract<
    const LINE_SIZE: usize,
    const X: usize,
    const Y: usize,
    const SCROLLER_WT: usize,
> {
    pub lines: [[u8; LINE_SIZE]; Y],
    pub sizes: [usize; Y],

    old_display: [[u8; X]; Y],
    scroll_wait_ticks: usize,
    current_scroll: usize,
    scroll_dir: i8,
}

impl<const LINE_SIZE: usize, const X: usize, const Y: usize, const SCROLLER_WT: usize>
    LcdAbstract<LINE_SIZE, X, Y, SCROLLER_WT>
{
    pub fn new() -> Self {
        Self {
            lines: [[b' '; LINE_SIZE]; Y],
            sizes: [0; Y],

            old_display: [[0; X]; Y],
            scroll_wait_ticks: 0,
            current_scroll: 0,
            scroll_dir: 0,
        }
    }

    pub fn display_data<'a>(&'a mut self) -> ([(&'a [u8], bool); Y], &'a mut [[u8; X]; Y]) {
        let mut tmp: [(&[u8], bool); Y] = [(&[], false); Y];

        for y in 0..Y {
            let scroll_max = self.sizes[y].saturating_sub(X);
            let scroll_offset = self.current_scroll.min(scroll_max);
            let scrolled_data = &self.lines[y][scroll_offset..X + scroll_offset];

            tmp[y] = (scrolled_data, scrolled_data != &self.old_display[y]);
        }

        (tmp, &mut self.old_display)
    }

    pub fn scroll_step(&mut self) -> Result<(), LcdError> {
        let max_size = *self.sizes.iter().max().ok_or_else(|| LcdError::Other)?;
        let max_scroll = max_size.saturating_sub(X);
        if max_scroll == 0 {
            return Ok(());
        }

        if self.scroll_wait_ticks > 0 {
            self.scroll_wait_ticks = self.scroll_wait_ticks.saturating_sub(1);
            return Ok(());
        }

        self.current_scroll = self
            .current_scroll
            .saturating_add_signed(self.scroll_dir as isize)
            .min(max_scroll);

        if self.current_scroll == 0 || self.current_scroll == max_scroll {
            self.scroll_wait_ticks = SCROLLER_WT - 1;

            self.scroll_dir = if self.current_scroll == 0 {
                1
            } else if self.current_scroll == max_scroll {
                -1
            } else {
                return Err(LcdError::Other);
            };
        }

        Ok(())
    }

    pub fn print(
        &mut self,
        line: usize,
        text: &str,
        align: PrintAlign,
        pad: bool,
    ) -> Result<(), LcdError> {
        if line > Y || text.len() > LINE_SIZE {
            return Err(LcdError::OutOfRange);
        }

        self.current_scroll = 0;
        self.scroll_wait_ticks = 0;

        let x_offset = if text.len() < X {
            match align {
                PrintAlign::Left => 0,
                PrintAlign::Center => (X - text.len()) / 2,
                PrintAlign::Right => X - text.len(),
            }
        } else {
            0
        };

        if pad && text.len() < X {
            let mut tmp_line = [b' '; X];
            let end_offset = (x_offset + text.len()).min(X);
            tmp_line[x_offset..end_offset]
                .copy_from_slice(&text.as_bytes()[..(end_offset - x_offset)]);

            self.lines[line][..X].copy_from_slice(&tmp_line);
            self.sizes[line] = X;
        } else {
            self.lines[line][x_offset..x_offset + text.len()].copy_from_slice(text.as_bytes());
            self.sizes[line] = text.len();
        }

        self.scroll_wait_ticks = SCROLLER_WT - 1;
        Ok(())
    }

    pub fn clear(&mut self, line: usize) -> Result<(), LcdError> {
        if line > Y {
            return Err(LcdError::OutOfRange);
        }

        self.lines[line][..X].fill(b' ');
        self.sizes[line] = 0;
        Ok(())
    }

    pub fn clear_all(&mut self) -> Result<(), LcdError> {
        for y in 0..Y {
            self.clear(y)?;
        }

        Ok(())
    }

    pub fn display_on_lcd<B: DataBus, M: DisplayMemoryMap, C: CharsetWithFallback, D: DelayNs>(
        &mut self,
        lcd: &mut HD44780<B, M, C>,
        delay: &mut D,
    ) -> Result<(), LcdError> {
        let display_data = self.display_data();
        for (y, line) in display_data.0.iter().enumerate() {
            if line.1 {
                lcd.set_cursor_xy((0, y as u8), delay)
                    .map_err(|_| LcdError::Other)?;

                lcd.write_bytes(line.0, delay)
                    .map_err(|_| LcdError::Other)?;

                display_data.1[y].copy_from_slice(line.0);
            }
        }

        Ok(())
    }
}
