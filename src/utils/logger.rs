use crate::state::{ota_state, sleep_state};
use alloc::vec::Vec;
use core::cell::{Cell, UnsafeCell};
use embassy_sync::blocking_mutex::{Mutex as BlockingMutex, raw::CriticalSectionRawMutex};

const LOGS_BUF_SIZE: usize = 8 * 1024;

/// The log bytes live in `.dram2_uninit` so the 8 KiB is not zeroed at boot.
/// Access is guarded by `LOGS` below; sound on the single core.
struct BufCell(UnsafeCell<core::mem::MaybeUninit<[u8; LOGS_BUF_SIZE]>>);
unsafe impl Sync for BufCell {}

#[unsafe(link_section = ".dram2_uninit")]
static LOGS_BUF: BufCell = BufCell(UnsafeCell::new(core::mem::MaybeUninit::uninit()));

/// Guards `LOGS_BUF` and holds the write position (kept out of `.dram2_uninit`,
/// which is not initialized at boot).
static LOGS: BlockingMutex<CriticalSectionRawMutex, Cell<usize>> = BlockingMutex::new(Cell::new(0));

/// Short-lived writer over the log buffer; used only inside `LOGS.lock(..)`.
struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Writer<'_> {
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

impl core::fmt::Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_raw(s.as_bytes());
        Ok(())
    }
}

/// Drain the buffered logs into an owned Vec (resetting the write position).
pub fn get_vec(current_time: Option<u64>) -> Vec<u8> {
    LOGS.lock(|pos_cell| {
        let buf = unsafe { &mut *(*LOGS_BUF.0.get()).as_mut_ptr() };
        if let Some(current_time) = current_time {
            buf[2..10].copy_from_slice(&current_time.to_be_bytes());
        }

        let tmp = buf[0..pos_cell.get()].to_vec();
        pos_cell.set(0);
        tmp
    })
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
        const MAGENTA: &str = "\u{001B}[35m";

        let color = match record.level() {
            log::Level::Error => RED,
            log::Level::Warn => YELLOW,
            log::Level::Info => GREEN,
            log::Level::Debug => BLUE,
            log::Level::Trace => MAGENTA,
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

            LOGS.lock(|pos_cell| {
                let buf = unsafe { &mut *(*LOGS_BUF.0.get()).as_mut_ptr() };
                let mut w = Writer {
                    buf,
                    pos: pos_cell.get(),
                };

                if w.pos == 0 {
                    w.buf[0] = b'L';
                    w.buf[1] = 0x00;
                    w.pos = 10;
                }

                if w.pos + 3 > w.buf.len() {
                    w.mark_truncated();
                    pos_cell.set(w.pos);
                    return;
                }

                let line_start = w.pos;
                w.pos += 2;
                w.write_raw(level.as_bytes());

                _ = core::fmt::write(&mut w, *record.args());
                let line_len = w.pos - line_start - 2;
                w.buf[line_start..line_start + 2].copy_from_slice(&(line_len as u16).to_be_bytes());

                pos_cell.set(w.pos);
            });
        }
    }

    fn flush(&self) {}
}
