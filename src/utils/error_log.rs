use crate::{consts::NVS_ERROR_LOG, state::current_epoch};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use anyhow::Result;
use core::cell::{Cell, UnsafeCell};
use embassy_sync::blocking_mutex::{Mutex as BlockingMutex, raw::CriticalSectionRawMutex};
use esp_hal_wifimanager::Nvs;

const ERROR_LOG_BUF_SIZE: usize = 2 * 1024;

/// The log bytes live in `.dram2_uninit` so the 2 KiB is not zeroed at boot.
/// Every access happens inside the `ERROR_LOG` critical-section lock below, so
/// this `Sync` impl is sound on the single core.
struct BufCell(UnsafeCell<core::mem::MaybeUninit<[u8; ERROR_LOG_BUF_SIZE]>>);
unsafe impl Sync for BufCell {}

#[unsafe(link_section = ".dram2_uninit")]
static ERROR_LOG_BUF: BufCell = BufCell(UnsafeCell::new(core::mem::MaybeUninit::uninit()));

#[derive(Clone, Copy)]
struct Meta {
    offset: usize,
    save_ready: bool,
}

/// Guards `ERROR_LOG_BUF` and holds `offset`/`save_ready` (these must stay out
/// of `.dram2_uninit`, which is not initialized at boot).
static ERROR_LOG: BlockingMutex<CriticalSectionRawMutex, Cell<Meta>> =
    BlockingMutex::new(Cell::new(Meta {
        offset: 0,
        save_ready: false,
    }));

#[inline(always)]
pub fn is_save_ready() -> bool {
    ERROR_LOG.lock(|m| m.get().save_ready)
}

#[inline(always)]
pub fn clear_save_ready() {
    ERROR_LOG.lock(|m| {
        let mut meta = m.get();
        meta.save_ready = false;
        m.set(meta);
    });
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
    #[cfg(feature = "release_build")]
    pub const DOUBLE_PANIC_RECOVERY: u8 = 90;
    pub const BACKTRACE_READ_FAILED: u8 = 91;
}

/// Size of the log entry at `offset` in `buf`, or None if malformed/truncated.
fn entry_size(buf: &[u8], offset: usize) -> Option<usize> {
    match *buf.get(offset)? {
        b'N' => Some(1 + 8 + 1),
        b'S' => {
            let size = *buf.get(offset + 1 + 8 + 16)? as usize;
            Some(1 + 8 + 16 + 1 + size * 4)
        }
        _ => None,
    }
}

/// Ensures there is room for `needed` bytes at `offset`, dropping the oldest
/// entries when the buffer is full. Returns false if the entry can never fit.
fn ensure_space(buf: &mut [u8], offset: &mut usize, needed: usize) -> bool {
    while *offset + needed > buf.len() {
        match entry_size(buf, 0) {
            Some(size) if size <= *offset => {
                buf.copy_within(size..*offset, 0);
                *offset -= size;
            }
            _ => return false,
        }
    }

    true
}

pub async fn add_error(code: u8) {
    let epoch = current_epoch();
    ERROR_LOG.lock(|cell| {
        let mut meta = cell.get();
        let buf = unsafe { &mut *(*ERROR_LOG_BUF.0.get()).as_mut_ptr() };

        if !ensure_space(buf, &mut meta.offset, 1 + 8 + 1) {
            return;
        }

        let o = meta.offset;
        buf[o] = b'N';
        buf[o + 1..o + 1 + 8].copy_from_slice(&epoch.to_be_bytes());
        buf[o + 1 + 8] = code;

        meta.offset = o + 1 + 8 + 1;
        meta.save_ready = true;
        cell.set(meta);
    });
}

/// Log `code` at most once, guarded by `flag`.
pub async fn report_once(flag: &core::sync::atomic::AtomicBool, code: u8) {
    if !flag.load(core::sync::atomic::Ordering::Relaxed) {
        add_error(code).await;
        flag.store(true, core::sync::atomic::Ordering::Relaxed);
    }
}

pub async fn add_stacktrace(addrs: &[u32], version: &str, timestamp: u64) {
    ERROR_LOG.lock(|cell| {
        let mut meta = cell.get();
        let buf = unsafe { &mut *(*ERROR_LOG_BUF.0.get()).as_mut_ptr() };

        if !ensure_space(buf, &mut meta.offset, 1 + 8 + 16 + 1 + addrs.len() * 4) {
            return;
        }

        let mut version_buf = [0; 16];
        let version_bytes = version.as_bytes();
        let version_len = core::cmp::min(version_bytes.len(), version_buf.len());
        version_buf[..version_len].copy_from_slice(&version_bytes[..version_len]);

        let o = meta.offset;
        buf[o] = b'S';
        buf[o + 1..o + 1 + 8].copy_from_slice(&timestamp.to_be_bytes());
        buf[o + 1 + 8..o + 1 + 8 + 16].copy_from_slice(&version_buf);
        buf[o + 1 + 8 + 16] = addrs.len() as u8;

        for (i, &addr) in addrs.iter().enumerate() {
            let start = o + 1 + 8 + 16 + 1 + i * 4;
            let end = start + 4;
            buf[start..end].copy_from_slice(&addr.to_be_bytes());
        }

        meta.offset = o + 1 + 8 + 16 + 1 + addrs.len() * 4;
        meta.save_ready = true;
        cell.set(meta);
    });
}

pub fn dump_error_log() -> Vec<u8> {
    ERROR_LOG.lock(|cell| {
        let meta = cell.get();
        let buf = unsafe { &*(*ERROR_LOG_BUF.0.get()).as_ptr() };
        buf[..meta.offset].to_vec()
    })
}

pub async fn load_error_log(nvs: &Nvs) {
    let Ok(buf) = nvs.get::<Vec<u8>>(NVS_ERROR_LOG).await else {
        return;
    };

    ERROR_LOG.lock(|cell| {
        let log_buf = unsafe { &mut *(*ERROR_LOG_BUF.0.get()).as_mut_ptr() };

        if buf.len() > log_buf.len() {
            // stored log is oversized/corrupt - start fresh instead of panicking
            let mut meta = cell.get();
            meta.offset = 0;
            cell.set(meta);
            return;
        }

        let loaded_len = buf.len();
        log_buf[..loaded_len].copy_from_slice(&buf);

        // validate structure, truncate at the first malformed entry
        let mut offset = 0;
        while offset < loaded_len {
            match entry_size(log_buf, offset) {
                Some(size) if offset + size <= loaded_len => {
                    offset += size;
                }
                _ => break,
            }
        }

        let mut meta = cell.get();
        meta.offset = offset;
        cell.set(meta);
    });
}

pub async fn save_error_log(nvs: &Nvs) {
    // Snapshot under the lock so a concurrent add_error can't shift the bytes mid-save.
    let snapshot: Vec<u8> = ERROR_LOG.lock(|cell| {
        let meta = cell.get();
        let buf = unsafe { &*(*ERROR_LOG_BUF.0.get()).as_ptr() };
        buf[..meta.offset].to_vec()
    });

    _ = nvs.delete(NVS_ERROR_LOG).await;
    let res = nvs.set(NVS_ERROR_LOG, snapshot.as_slice()).await;
    if let Err(e) = res {
        log::error!("errorlog save error: {e:?}");
    }
}

pub fn parse_error_log_entries() -> Result<Vec<ErrorLogEntry>> {
    // Snapshot under the lock, then parse/allocate outside it.
    let buf: Vec<u8> = ERROR_LOG.lock(|cell| {
        let meta = cell.get();
        let b = unsafe { &*(*ERROR_LOG_BUF.0.get()).as_ptr() };
        b[..meta.offset].to_vec()
    });

    let mut tmp = Vec::new();
    let max_offset = buf.len();

    log::warn!("ERROR LOG:");
    let mut offset = 0;
    while offset < max_offset {
        let log_type = buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                let entry = ErrorLogEntry::Code {
                    timestamp: u64::from_be_bytes(buf[offset + 1..offset + 1 + 8].try_into()?),
                    code: buf[offset + 1 + 8],
                };

                if let ErrorLogEntry::Code { timestamp, code } = entry {
                    log::warn!(
                        "Error {} at {}",
                        code,
                        crate::utils::error_log::format_timestamp_full(timestamp)
                    );
                }

                tmp.push(entry);
                offset += 1 + 8 + 1;
            }
            b'S' => {
                // u64 + 16 * u8 + u8 (size) + size * u32

                let version_str = core::str::from_utf8(&buf[offset + 1 + 8..offset + 1 + 8 + 16])?;
                let version_str = version_str.trim_end_matches('\0');

                let size = buf[offset + 1 + 8 + 16];
                let mut tmp_addrs = Vec::new();
                for addr in (buf
                    [offset + 1 + 8 + 16 + 1..offset + 1 + 8 + 16 + 1 + size as usize * 4])
                    .chunks(4)
                {
                    tmp_addrs.push(u32::from_be_bytes(addr.try_into()?));
                }
                let entry = ErrorLogEntry::Stacktrace {
                    timestamp: u64::from_be_bytes(buf[offset + 1..offset + 1 + 8].try_into()?),
                    version: version_str.to_string(),
                    addrs: tmp_addrs,
                };
                if let ErrorLogEntry::Stacktrace {
                    timestamp,
                    ref version,
                    ref addrs,
                } = entry
                {
                    log::warn!(
                        "Panic of ver {version} at {}. Addrs: {addrs:X?}",
                        crate::utils::error_log::format_timestamp_full(timestamp)
                    );
                }

                tmp.push(entry);
                offset += 1 + 8 + 16 + 1 + size as usize * 4;
            }
            _ => {
                break;
            }
        }
    }

    Ok(tmp)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorLogEntry {
    Code {
        timestamp: u64,
        code: u8,
    },
    Stacktrace {
        timestamp: u64,
        version: String,
        addrs: Vec<u32>,
    },
}

impl ErrorLogEntry {
    #[cfg(feature = "v3")]
    pub fn list_label_v3(&self) -> String {
        match self {
            ErrorLogEntry::Code { timestamp, code } => {
                format!("E{code} {}", format_timestamp_compact(*timestamp))
            }
            ErrorLogEntry::Stacktrace { timestamp, .. } => {
                format!("Panic {}", format_timestamp_compact(*timestamp))
            }
        }
    }

    #[cfg(feature = "v4")]
    pub fn list_label_v4(&self) -> String {
        match self {
            ErrorLogEntry::Code { timestamp, code } => {
                format!("E{code} {}", format_timestamp_compact(*timestamp))
            }
            ErrorLogEntry::Stacktrace { timestamp, .. } => {
                format!("Panic {}", format_timestamp_compact(*timestamp))
            }
        }
    }
}

pub fn format_timestamp_compact(timestamp: u64) -> String {
    let (_year, month, day, hour, minute, _second) = epoch_to_ymdhms(timestamp);
    format!("{day:02}/{month:02} {hour:02}:{minute:02}")
}

pub fn format_timestamp_full(timestamp: u64) -> String {
    let (year, month, day, hour, minute, second) = epoch_to_ymdhms(timestamp);
    format!("{day:02}/{month:02}/{year:04} {hour:02}:{minute:02}:{second:02}")
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
