use crate::{consts::NVS_ERROR_LOG, state::current_epoch};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use anyhow::Result;
use esp_hal_wifimanager::Nvs;

#[unsafe(link_section = ".dram2_uninit")]
static mut ERROR_LOG_BUF: core::mem::MaybeUninit<[u8; 2 * 1024]> = core::mem::MaybeUninit::uninit();
static mut OFFSET: usize = 0;
static mut SAVE_READY: bool = false;

#[inline(always)]
pub fn is_save_ready() -> bool {
    unsafe { SAVE_READY }
}

#[inline(always)]
pub fn clear_save_ready() {
    unsafe { SAVE_READY = false };
}

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

    // LCD / Display (20-29)
    #[cfg(feature = "v4")]
    pub const LCD_INIT_FAILED: u8 = 20;
    #[cfg(feature = "v4")]
    pub const LCD_FRAMEBUFFER_ALLOC_FAILED: u8 = 21;

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

    // Crash recovery (90-99)
    #[cfg(feature = "release_build")]
    pub const DOUBLE_PANIC_RECOVERY: u8 = 90;
    pub const BACKTRACE_READ_FAILED: u8 = 91;
}

pub async fn add_error(code: u8) {
    unsafe {
        #[allow(static_mut_refs)]
        let error_log_buf = &mut (*ERROR_LOG_BUF.as_mut_ptr());

        error_log_buf[OFFSET] = b'N';
        error_log_buf[OFFSET + 1..OFFSET + 1 + 8].copy_from_slice(&current_epoch().to_be_bytes());
        error_log_buf[OFFSET + 1 + 8] = code;

        OFFSET += 1 + 8 + 1;
        SAVE_READY = true;
    }
}

pub async fn add_stacktrace(addrs: &[u32], version: &str, timestamp: u64) {
    unsafe {
        #[allow(static_mut_refs)]
        let error_log_buf = &mut (*ERROR_LOG_BUF.as_mut_ptr());

        let mut version_buf = [0; 16];
        let version_bytes = version.as_bytes();
        let version_len = core::cmp::min(version_bytes.len(), version_buf.len());
        version_buf[..version_len].copy_from_slice(&version_bytes[..version_len]);

        error_log_buf[OFFSET] = b'S';
        error_log_buf[OFFSET + 1..OFFSET + 1 + 8].copy_from_slice(&timestamp.to_be_bytes());
        error_log_buf[OFFSET + 1 + 8..OFFSET + 1 + 8 + 16].copy_from_slice(&version_buf);
        error_log_buf[OFFSET + 1 + 8 + 16] = addrs.len() as u8;

        for (i, &addr) in addrs.iter().enumerate() {
            let start = OFFSET + 1 + 8 + 16 + 1 + i * 4;
            let end = start + 4;
            error_log_buf[start..end].copy_from_slice(&addr.to_be_bytes());
        }

        OFFSET += 1 + 8 + 16 + 1 + addrs.len() * 4;
        SAVE_READY = true;
    }
}

pub async fn load_error_log(nvs: &Nvs) {
    let Ok(buf) = nvs.get::<Vec<u8>>(NVS_ERROR_LOG).await else {
        return;
    };

    let loaded_len = buf.len();

    #[allow(static_mut_refs)]
    let error_log_buf = unsafe { &mut (*ERROR_LOG_BUF.as_mut_ptr()) };
    error_log_buf[..buf.len()].copy_from_slice(&buf);
    drop(buf);

    let mut offset = 0;
    while offset < loaded_len {
        let log_type = error_log_buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                offset += 1 + 8 + 1;
            }
            b'S' => {
                // u64 + 16 * u8 + u8 (size) + size * u32
                let size = error_log_buf[offset + 1 + 8 + 16];
                offset += 1 + 8 + 16 + 1 + size as usize * 4;
            }
            _ => {
                break;
            }
        }
    }

    unsafe {
        OFFSET = offset;
    }
}

pub async fn save_error_log(nvs: &Nvs) {
    #[allow(static_mut_refs)]
    let error_log_buf = unsafe { &mut (*ERROR_LOG_BUF.as_mut_ptr()) };

    unsafe {
        _ = nvs.delete(NVS_ERROR_LOG).await;
        let res = nvs.set(NVS_ERROR_LOG, &error_log_buf[..OFFSET]).await;
        if let Err(e) = res {
            log::error!("errorlog save error: {e:?}");
        }
    }
}

pub fn parse_error_log_entries() -> Result<Vec<ErrorLogEntry>> {
    let mut tmp = Vec::new();

    #[allow(static_mut_refs)]
    let error_log_buf = unsafe { &mut (*ERROR_LOG_BUF.as_mut_ptr()) };
    let max_offset = unsafe { OFFSET };

    log::warn!("ERROR LOG:");
    let mut offset = 0;
    while offset < max_offset {
        let log_type = error_log_buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                let entry = ErrorLogEntry::Code {
                    timestamp: u64::from_be_bytes(
                        error_log_buf[offset + 1..offset + 1 + 8].try_into()?,
                    ),
                    code: error_log_buf[offset + 1 + 8],
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

                let version_str =
                    core::str::from_utf8(&error_log_buf[offset + 1 + 8..offset + 1 + 8 + 16])?;
                let version_str = version_str.trim_end_matches('\0');

                let size = error_log_buf[offset + 1 + 8 + 16];
                let mut tmp_addrs = Vec::new();
                for addr in (error_log_buf
                    [offset + 1 + 8 + 16 + 1..offset + 1 + 8 + 16 + 1 + size as usize * 4])
                    .chunks(4)
                {
                    tmp_addrs.push(u32::from_be_bytes(addr.try_into()?));
                }
                let entry = ErrorLogEntry::Stacktrace {
                    timestamp: u64::from_be_bytes(
                        error_log_buf[offset + 1..offset + 1 + 8].try_into()?,
                    ),
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
