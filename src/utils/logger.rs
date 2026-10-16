use super::UninitBuf;
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use portable_atomic::{AtomicBool, Ordering};

const HALF: usize = 4 * 1024;
/// 'L' + truncated flag + u64 time + u64 inspection + u64 competitor + 20 B group id
/// + 36 B session id + padding
const HEADER_LEN: usize = 100;

/// Two halves: the logger appends to the active one, the consumer sends the other.
#[unsafe(link_section = ".dram2_uninit")]
static LOGS_BUF: UninitBuf<{ 2 * HALF }> = UninitBuf::new();

struct LogsState {
    initialized: bool,
    active: usize,
    pos: usize,
}

static LOGS: Mutex<CriticalSectionRawMutex, RefCell<LogsState>> =
    Mutex::new(RefCell::new(LogsState {
        initialized: false,
        active: 0,
        pos: 0,
    }));
static READY_TAKEN: AtomicBool = AtomicBool::new(false);

/// Live values written into the header of a log batch.
pub struct LogsHeader<'a> {
    pub current_time: Option<u64>,
    pub inspection_time: Option<u64>,
    pub competitor: Option<u64>,
    pub group_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
}

/// A finished log batch; the half stays reserved until this is dropped.
pub struct ReadyLogs(&'static [u8]);

impl core::ops::Deref for ReadyLogs {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

impl Drop for ReadyLogs {
    fn drop(&mut self) {
        READY_TAKEN.store(false, Ordering::Release);
    }
}

/// Swaps the halves and returns the finished batch (with `header` filled in), or
/// `None` if nothing was logged or the previous batch is still held.
pub fn take_ready(header: &LogsHeader) -> Option<ReadyLogs> {
    if READY_TAKEN.swap(true, Ordering::Acquire) {
        return None;
    }

    let ready = LOGS.lock(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        if state.pos == 0 {
            return None;
        }

        let (half, len) = (state.active, state.pos);
        state.active ^= 1;
        state.pos = 0;

        // SAFETY: inside the critical section; the returned half is only written again
        // after the next swap, which needs READY_TAKEN (held by the ReadyLogs).
        let buf = unsafe { LOGS_BUF.get(&mut state.initialized) };
        let batch = &mut buf[half * HALF..half * HALF + len];
        write_header(batch, header);

        let ptr = batch.as_ptr();
        // SAFETY: points into a 'static buffer; exclusively ours until ReadyLogs drops.
        Some(unsafe { core::slice::from_raw_parts(ptr, len) })
    });

    match ready {
        Some(bytes) => Some(ReadyLogs(bytes)),
        None => {
            READY_TAKEN.store(false, Ordering::Release);
            None
        }
    }
}

fn write_header(batch: &mut [u8], header: &LogsHeader) {
    batch[2..10].copy_from_slice(&header.current_time.unwrap_or(u64::MAX).to_be_bytes());
    batch[10..18].copy_from_slice(&header.inspection_time.unwrap_or(u64::MAX).to_be_bytes());
    batch[18..26].copy_from_slice(&header.competitor.unwrap_or(u64::MAX).to_be_bytes());
    batch[26..82].fill(0x00);

    if let Some(group_id) = header.group_id.filter(|g| g.len() <= 20) {
        batch[26..26 + group_id.len()].copy_from_slice(group_id.as_bytes());
    }

    if let Some(session_id) = header.session_id.filter(|s| s.len() <= 36) {
        batch[46..46 + session_id.len()].copy_from_slice(session_id.as_bytes());
    }
}

/// Appends one line record (u16 len + level + text) to the active half.
fn append_line(level: &str, args: &core::fmt::Arguments) {
    LOGS.lock(|state| {
        // a log from inside the formatting below (or an ISR) is dropped instead of tearing
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let state = &mut *state;
        // SAFETY: inside the critical section, only the active half is written.
        let buf = unsafe { LOGS_BUF.get(&mut state.initialized) };
        let half = &mut buf[state.active * HALF..(state.active + 1) * HALF];

        if state.pos == 0 {
            half[0] = b'L';
            half[1] = 0x00;
            state.pos = HEADER_LEN;
        }

        if state.pos + 3 > HALF {
            half[1] = 0x01; // truncated
            return;
        }

        let mut w = HalfWriter {
            buf: half,
            pos: state.pos + 2,
        };
        w.write_raw(level.as_bytes());
        _ = core::fmt::write(&mut w, *args);
        let end = w.pos;

        let line_len = (end - state.pos - 2) as u16;
        half[state.pos..state.pos + 2].copy_from_slice(&line_len.to_be_bytes());
        state.pos = end;
    });
}

struct HalfWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl HalfWriter<'_> {
    fn write_raw(&mut self, bytes: &[u8]) {
        let len = bytes.len().min(self.buf.len() - self.pos);
        self.buf[self.pos..self.pos + len].copy_from_slice(&bytes[..len]);
        self.pos += len;
    }
}

impl core::fmt::Write for HalfWriter<'_> {
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
        metadata.level() <= FILTER_MAX
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

        #[cfg(not(feature = "sim"))]
        esp_println::println!("{}{} - {}{}", color, record.level(), record.args(), RESET);
        #[cfg(feature = "sim")]
        crate::sim::print(format_args!(
            "{}{} - {}{}",
            color,
            record.level(),
            record.args(),
            RESET
        ));

        // bat_dev_lcd/qa keep logs on the serial port only
        if !cfg!(any(feature = "bat_dev_lcd", feature = "qa"))
            && !crate::ws::ota_active()
            && !crate::power::is_sleeping()
        {
            let level: &str = match record.level() {
                log::Level::Error => "E ",
                log::Level::Warn => "W ",
                log::Level::Info => "I ",
                log::Level::Debug => "D ",
                log::Level::Trace => "T ",
            };

            append_line(level, record.args());
        }
    }

    fn flush(&self) {}
}
