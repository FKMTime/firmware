use super::{
    UninitBuf,
    ring_log::{self, CODE_LEN, STACKTRACE_HEADER_LEN, record_len},
};
use crate::{consts::NVS_ERROR_LOG, state::current_epoch};
use core::{cell::RefCell, fmt::Write};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::Instant;
use esp_hal_wifimanager::Nvs;
use portable_atomic::{AtomicBool, Ordering};

const CAPACITY: usize = 2 * 1024;
/// Same code is recorded at most once per this many seconds (a failing peripheral
/// reports in a tight loop).
const RATE_LIMIT_SECS: u64 = 60;
const RATE_LIMIT_SLOTS: usize = 8;
pub const MAX_STACKTRACE_ADDRS: usize = 16;


/// Records, oldest first, in the [`ring_log`] format (same as stored in NVS).
#[unsafe(link_section = ".dram2_uninit")]
static BUF: UninitBuf<CAPACITY> = UninitBuf::new();

struct ErrorLog {
    initialized: bool,
    len: usize,
    recent: [(u8, u64); RATE_LIMIT_SLOTS],
    recent_next: usize,
}

static LOG: Mutex<CriticalSectionRawMutex, RefCell<ErrorLog>> =
    Mutex::new(RefCell::new(ErrorLog {
        initialized: false,
        len: 0,
        recent: [(0, 0); RATE_LIMIT_SLOTS],
        recent_next: 0,
    }));
static DIRTY: AtomicBool = AtomicBool::new(false);

/// Runs `f` with exclusive access to the log state and the (initialized) buffer.
fn with_log<R>(f: impl FnOnce(&mut ErrorLog, &mut [u8; CAPACITY]) -> R) -> R {
    LOG.lock(|log| {
        let mut log = log.borrow_mut();
        // SAFETY: the critical section serializes all access to BUF.
        let buf = unsafe { BUF.get(&mut log.initialized) };
        f(&mut log, buf)
    })
}

#[inline(always)]
pub fn is_save_ready() -> bool {
    DIRTY.load(Ordering::Relaxed)
}

#[inline(always)]
pub fn clear_save_ready() {
    DIRTY.store(false, Ordering::Relaxed);
}

#[allow(dead_code)]
pub mod codes {
    // RFID (1-9)
    pub const RFID_INIT_FAILED: u8 = 1;
    #[cfg(feature = "v3")]
    pub const RFID_SPI_CREATE_FAILED: u8 = 2;
    #[cfg(feature = "v3")]
    pub const RFID_SPI_BUS_INIT_FAILED: u8 = 3;
    #[cfg(feature = "v3")]
    pub const RFID_DMA_TX_INIT_FAILED: u8 = 4;
    #[cfg(feature = "v3")]
    pub const RFID_DMA_RX_INIT_FAILED: u8 = 5;
    pub const RFID_SOLVE_GROUP_MISSING: u8 = 6;

    // Battery (10-19)
    #[cfg(feature = "v4")]
    pub const BATTERY_INIT_FAILED: u8 = 10;
    #[cfg(feature = "v4")]
    pub const BATTERY_I2C_TIMEOUT: u8 = 11;

    // LCD / Display (20-29)
    #[cfg(feature = "v4")]
    pub const LCD_INIT_FAILED: u8 = 20;
    #[cfg(feature = "v4")]
    pub const LCD_FRAMEBUFFER_ALLOC_FAILED: u8 = 21;
    #[cfg(feature = "v4")]
    pub const LCD_FLUSH_TIMEOUT: u8 = 22;

    // Stackmat (30-39)
    pub const STACKMAT_UART_INIT_FAILED: u8 = 30;

    // Firmware / OTA (40-49)
    pub const WRONG_PARTITION_TABLE: u8 = 40;
    pub const OTA_MARK_VALID_FAILED: u8 = 41;
    pub const OTA_VERIFY_FAILED: u8 = 42;
    pub const WS_CONNECTION_LOST_DURING_OTA: u8 = 43;

    // BLE (50-59)
    pub const BLE_INIT_FAILED: u8 = 50;
    pub const BLE_MAC_READ_FAILED: u8 = 51;
    pub const BLE_BOND_ADD_FAILED: u8 = 52;
    pub const BLE_SCAN_START_FAILED: u8 = 53;
    pub const BLE_BONDABLE_FAILED: u8 = 54;
    pub const BLE_REQUEST_SECURITY_FAILED: u8 = 55;
    pub const BLE_PAIRING_FAILED: u8 = 56;
    pub const BLE_GATT_CLIENT_FAILED: u8 = 57;
    pub const BLE_SERVICE_NOT_FOUND: u8 = 58;
    pub const BLE_CHARACTERISTIC_NOT_FOUND: u8 = 59;

    // Wifi / mDNS / Websocket (60-69)
    pub const WIFI_MANAGER_FAILED: u8 = 60;
    pub const MDNS_WS_URL_PARSE_FAILED: u8 = 61;
    pub const WS_DNS_RESOLVE_EMPTY: u8 = 62;
    pub const WS_HTTP_UPGRADE_READ_FAILED: u8 = 63;
    pub const WS_PACKET_PARSE_FAILED: u8 = 64;
    pub const WS_PACKET_SERIALIZE_FAILED: u8 = 65;
    pub const WS_TAGGED_SUBSCRIBER_FAILED: u8 = 66;

    // NVS persistence (70-79)
    pub const NVS_SAVED_STATE_WRITE_FAILED: u8 = 70;
    pub const NVS_BONDING_KEY_WRITE_FAILED: u8 = 71;
    #[cfg(feature = "v4")]
    pub const NVS_BUZZER_VOLUME_WRITE_FAILED: u8 = 72;
    pub const ERROR_LOG_PARSE_FAILED: u8 = 73;
    pub const NVS_SAVED_STATE_DELETE_FAILED: u8 = 74;

    // Tasks / runtime (80-89)
    pub const TASK_SPAWN_FAILED: u8 = 80;
    pub const SHARED_I2C_TIMEOUT: u8 = 81;

    // Crash recovery (90-99)
    /// Several crashes / watchdog resets in a row (was "double panic recovery").
    pub const REPEATED_CRASHES: u8 = 90;
    pub const WATCHDOG_RESET: u8 = 92;
}

fn push_record(log: &mut ErrorLog, buf: &mut [u8; CAPACITY], record: &[u8]) {
    if ring_log::push(buf, &mut log.len, record) {
        DIRTY.store(true, Ordering::Relaxed);
    }
}

/// Records error `code` (rate limited per code).
pub fn add_error(code: u8) {
    let now = Instant::now().as_secs();
    let mut record = [0; CODE_LEN];
    record[0] = b'N';
    record[1..9].copy_from_slice(&current_epoch().to_be_bytes());
    record[9] = code;

    with_log(|log, buf| {
        let recently = log
            .recent
            .iter()
            .any(|&(c, at)| c == code && at != 0 && now.saturating_sub(at) < RATE_LIMIT_SECS);
        if recently {
            return;
        }

        let slot = log.recent_next;
        log.recent[slot] = (code, now.max(1));
        log.recent_next = (slot + 1) % RATE_LIMIT_SLOTS;
        push_record(log, buf, &record);
    });
}

/// Records a panic backtrace (at most [`MAX_STACKTRACE_ADDRS`] addresses).
pub fn add_stacktrace(addrs: &[u32], version: &str, timestamp: u64) {
    let addrs = &addrs[..addrs.len().min(MAX_STACKTRACE_ADDRS)];
    let mut record = [0; STACKTRACE_HEADER_LEN + MAX_STACKTRACE_ADDRS * 4];
    record[0] = b'S';
    record[1..9].copy_from_slice(&timestamp.to_be_bytes());
    let version_len = version.len().min(16);
    record[9..9 + version_len].copy_from_slice(&version.as_bytes()[..version_len]);
    record[STACKTRACE_HEADER_LEN - 1] = addrs.len() as u8;
    for (chunk, addr) in record[STACKTRACE_HEADER_LEN..].chunks_exact_mut(4).zip(addrs) {
        chunk.copy_from_slice(&addr.to_be_bytes());
    }

    let len = STACKTRACE_HEADER_LEN + addrs.len() * 4;
    with_log(|log, buf| push_record(log, buf, &record[..len]));
}

/// Calls `f` with the raw log (the NVS / crash log dump format).
pub fn with_error_log<R>(f: impl FnOnce(&[u8]) -> R) -> R {
    with_log(|log, buf| f(&buf[..log.len]))
}

/// Restores the log saved in NVS, keeping only the valid prefix.
pub async fn load_error_log(nvs: &Nvs) {
    let Ok(saved) = nvs.get::<alloc::vec::Vec<u8>>(&NVS_ERROR_LOG).await else {
        return;
    };

    let saved = &saved[..saved.len().min(CAPACITY)];
    let valid = ring_log::valid_prefix(saved);
    with_log(|log, buf| {
        buf[..valid].copy_from_slice(&saved[..valid]);
        log.len = valid;
    });
}

pub async fn save_error_log(nvs: &Nvs) {
    // shortcut: copies the log to the heap for the NVS write (rare, throttled by the caller)
    let bytes = with_error_log(|b| b.to_vec());
    if let Err(e) = nvs.set(&NVS_ERROR_LOG, bytes.as_slice()).await {
        log::error!("errorlog save error: {e:?}");
    }
}

/// Number of entries in the log.
pub fn entry_count() -> usize {
    with_error_log(|buf| Entries { buf }.count())
}

/// Entry `idx` (oldest first).
pub fn entry(idx: usize) -> Option<ErrorLogEntry> {
    with_error_log(|buf| Entries { buf }.nth(idx))
}

struct Entries<'a> {
    buf: &'a [u8],
}

impl Iterator for Entries<'_> {
    type Item = ErrorLogEntry;

    fn next(&mut self) -> Option<ErrorLogEntry> {
        let len = record_len(self.buf)?;
        let (record, rest) = self.buf.split_at(len);
        self.buf = rest;

        let timestamp = u64::from_be_bytes(record[1..9].try_into().ok()?);
        Some(match record[0] {
            b'N' => ErrorLogEntry::Code {
                timestamp,
                code: record[9],
            },
            _ => {
                let version = core::str::from_utf8(&record[9..25])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                ErrorLogEntry::Stacktrace {
                    timestamp,
                    version: heapless::String::try_from(version).unwrap_or_default(),
                    addrs: record[STACKTRACE_HEADER_LEN..]
                        .chunks_exact(4)
                        .take(MAX_STACKTRACE_ADDRS)
                        .filter_map(|c| c.try_into().ok().map(u32::from_be_bytes))
                        .collect(),
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum ErrorLogEntry {
    Code {
        timestamp: u64,
        code: u8,
    },
    Stacktrace {
        timestamp: u64,
        version: heapless::String<16>,
        addrs: heapless::Vec<u32, MAX_STACKTRACE_ADDRS>,
    },
}

pub type Label = heapless::String<24>;

impl ErrorLogEntry {
    pub fn list_label(&self) -> Label {
        let mut label = Label::new();
        _ = match self {
            ErrorLogEntry::Code { timestamp, code } => {
                write!(label, "E{code} {}", format_timestamp_compact(*timestamp))
            }
            ErrorLogEntry::Stacktrace { timestamp, .. } => {
                write!(label, "Panic {}", format_timestamp_compact(*timestamp))
            }
        };
        label
    }
}

pub fn format_timestamp_compact(timestamp: u64) -> Label {
    let (_year, month, day, hour, minute, _second) = epoch_to_ymdhms(timestamp);
    let mut s = Label::new();
    _ = write!(s, "{day:02}/{month:02} {hour:02}:{minute:02}");
    s
}

pub fn format_timestamp_full(timestamp: u64) -> Label {
    let (year, month, day, hour, minute, second) = epoch_to_ymdhms(timestamp);
    let mut s = Label::new();
    _ = write!(
        s,
        "{day:02}/{month:02}/{year:04} {hour:02}:{minute:02}:{second:02}"
    );
    s
}

fn epoch_to_ymdhms(timestamp: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (timestamp / 86_400) as i64;
    let sod = (timestamp % 86_400) as u32;

    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let second = sod % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = (yoe + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    year += if month <= 2 { 1 } else { 0 };

    (year, month, day, hour, minute, second)
}
