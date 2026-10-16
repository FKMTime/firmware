//! Minimal async SSD1309 (128×64, I2C) driver: sends only the changed span of each page.
//! Init sequence and rotation are the ones oled_async 0.2.1 used (Ssd1309_128_64, Rotate180).

use super::oled_ui::Frame;
use embassy_time::Timer;
use embedded_hal::digital::OutputPin;
use embedded_hal_async::i2c::I2c;

const ADDR: u8 = 0x3C;
const CMD: u8 = 0x00;
const DATA: u8 = 0x40;

const INIT: &[u8] = &[
    CMD, 0xAE, // display off
    0xD5, 0x80, // clock div
    0xA8, 0x3F, // multiplex 64
    0x40, // start line 0
    0xAD, 0x8B, // charge pump
    0x81, 0x80, // contrast
    0xD9, 0xF1, // pre-charge
    0xDB, 0x40, // vcomh auto
    0xA4, // resume from RAM
    0xA6, // normal (not inverted)
    0xAF, // display on
    0xD3, 0x00, // display offset 0
];
const INIT2: &[u8] = &[
    CMD, 0xDA, 0x12, // alternative COM pins
    0xA0, 0xC0, // Rotate180: segment remap off, COM scan normal
];

pub struct Ssd1309<I> {
    i2c: I,
    pub frame: Frame,
}

impl<I: I2c> Ssd1309<I> {
    pub const fn new(i2c: I) -> Self {
        Self {
            i2c,
            frame: Frame::new(),
        }
    }

    pub async fn init(&mut self, rst: &mut impl OutputPin) -> Result<(), I::Error> {
        _ = rst.set_high();
        Timer::after_millis(1).await;
        _ = rst.set_low();
        Timer::after_millis(10).await;
        _ = rst.set_high();

        self.i2c.write(ADDR, INIT).await?;
        self.i2c.write(ADDR, INIT2).await?;
        self.frame.invalidate();
        Ok(())
    }

    /// Sends what changed since the last flush (nothing if the frame is unchanged).
    pub async fn flush(&mut self) -> Result<(), I::Error> {
        let mut data = [0u8; 129];
        data[0] = DATA;

        for page in 0..8 {
            let Some((col, bytes)) = self.frame.dirty(page) else {
                continue;
            };
            let n = bytes.len();
            data[1..=n].copy_from_slice(bytes);

            let addr = [CMD, 0xB0 | page as u8, col & 0x0F, 0x10 | (col >> 4)];
            let res = match self.i2c.write(ADDR, &addr).await {
                Ok(()) => self.i2c.write(ADDR, &data[..=n]).await,
                err => err,
            };
            if let Err(e) = res {
                self.frame.invalidate(); // panel content unknown now
                return Err(e);
            }
            self.frame.sent(page);
        }

        self.frame.mark_synced();
        Ok(())
    }
}
