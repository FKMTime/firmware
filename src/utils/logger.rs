use crate::state::{ota_state, sleep_state};
use alloc::vec::Vec;

#[unsafe(link_section = ".dram2_uninit")]
static mut LOGS_BUF: core::mem::MaybeUninit<[u8; 8 * 1024]> = core::mem::MaybeUninit::uninit();

pub struct LogsBufWriter {
    buf: &'static mut [u8],
    pos: usize,
}

#[allow(static_mut_refs)]
pub static mut LOGS_WRITER: LogsBufWriter = LogsBufWriter {
    buf: unsafe { &mut *LOGS_BUF.as_mut_ptr() },
    pos: 0,
};

impl LogsBufWriter {
    pub fn get_vec(&mut self, current_time: Option<u64>) -> Vec<u8> {
        if let Some(current_time) = current_time {
            self.buf[2..10].copy_from_slice(&current_time.to_be_bytes());
        }

        let tmp = self.buf[0..self.pos].to_vec();
        self.pos = 0;

        tmp
    }

    fn mark_truncated(&mut self) {
        self.buf[1] = 0x01;
    }

    fn write_raw(&mut self, bytes: &[u8]) {
        if self.pos >= self.buf.len() {
            return;
        }

        let len = bytes.len().min(self.buf.len() - self.pos);
        self.buf[self.pos..self.pos + len].copy_from_slice(&bytes[..len]);
        self.pos += len;
    }
}

impl core::fmt::Write for LogsBufWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_raw(s.as_bytes());
        Ok(())
    }
}

#[cfg(feature = "release_build")]
pub const FILTER_MAX: log::LevelFilter = log::LevelFilter::Info;

#[cfg(not(feature = "release_build"))]
pub const FILTER_MAX: log::LevelFilter = log::LevelFilter::Debug;

pub struct FkmLogger;

impl FkmLogger {
    pub fn set_logger() {
        unsafe {
            _ = log::set_logger_racy(&FkmLogger);
            log::set_max_level_racy(FILTER_MAX);
        }
    }
}

impl log::Log for FkmLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let level = metadata.level();
        //let target = metadata.target();

        if level <= FILTER_MAX {
            return true;
        }
        false
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        const RESET: &str = "\u{001B}[0m";
        const RED: &str = "\u{001B}[31m";
        const GREEN: &str = "\u{001B}[32m";
        const YELLOW: &str = "\u{001B}[33m";
        const BLUE: &str = "\u{001B}[34m";
        const CYAN: &str = "\u{001B}[35m";

        let color = match record.level() {
            log::Level::Error => RED,
            log::Level::Warn => YELLOW,
            log::Level::Info => GREEN,
            log::Level::Debug => BLUE,
            log::Level::Trace => CYAN,
        };
        let reset = RESET;

        esp_println::println!("{}{} - {}{}", color, record.level(), record.args(), reset);

        #[cfg(not(any(feature = "bat_dev_lcd", feature = "qa")))]
        if !ota_state() && !sleep_state() {
            let level: &str = match record.level() {
                log::Level::Error => "E ",
                log::Level::Warn => "W ",
                log::Level::Info => "I ",
                log::Level::Debug => "D ",
                log::Level::Trace => "T ",
            };

            unsafe {
                #[allow(clippy::deref_addrof)]
                let w = &mut *(&raw mut LOGS_WRITER);
                if w.pos == 0 {
                    w.buf[0] = b'L';
                    w.buf[1] = 0x00;
                    w.pos = 10;
                }

                if w.pos + 3 > w.buf.len() {
                    w.mark_truncated();
                    return;
                }

                let line_start = w.pos;
                w.pos += 2;
                w.write_raw(level.as_bytes());

                _ = core::fmt::write(w, *record.args());
                let line_len = w.pos - line_start - 2;
                w.buf[line_start..line_start + 2].copy_from_slice(&(line_len as u16).to_be_bytes());
            }
        }
    }

    fn flush(&self) {}
}
