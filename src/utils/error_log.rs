use crate::{consts::NVS_ERROR_LOG, state::current_epoch};
use alloc::vec::Vec;
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

pub async fn add_stacktrace(addrs: &[u32]) {
    unsafe {
        #[allow(static_mut_refs)]
        let error_log_buf = &mut (*ERROR_LOG_BUF.as_mut_ptr());

        error_log_buf[OFFSET] = b'S';
        error_log_buf[OFFSET + 1..OFFSET + 1 + 8].copy_from_slice(&current_epoch().to_be_bytes());
        error_log_buf[OFFSET + 1 + 8] = addrs.len() as u8;

        for (i, &addr) in addrs.iter().enumerate() {
            let start = OFFSET + 1 + 8 + 1 + i * 4;
            let end = start + 4;
            error_log_buf[start..end].copy_from_slice(&addr.to_be_bytes());
        }

        OFFSET += 1 + 8 + 1 + addrs.len() * 4;
        SAVE_READY = true;
    }
}

pub async fn load_error_log(nvs: &Nvs) {
    let Ok(buf) = nvs.get::<Vec<u8>>(NVS_ERROR_LOG).await else {
        return;
    };

    #[allow(static_mut_refs)]
    let error_log_buf = unsafe { &mut (*ERROR_LOG_BUF.as_mut_ptr()) };
    error_log_buf[..buf.len()].copy_from_slice(&buf);
    log::info!("{buf:?}");
    drop(buf);

    let mut offset = 0;
    while offset < error_log_buf.len() {
        let log_type = error_log_buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                offset += 1 + 8 + 1;
                log::warn!("NORMAL");
            }
            b'S' => {
                // u64 + u8 (size) + size * u32
                let size = error_log_buf[offset + 8];
                offset += 1 + 8 + 1 + size as usize * 4;
                log::warn!("STACKTRACE WITH SIZE: {size}");
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

    let mut offset = 0;
    while offset < error_log_buf.len() {
        let log_type = error_log_buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                tmp.push(ErrorLogEntry::Code {
                    timestamp: u64::from_be_bytes(
                        error_log_buf[offset + 1..offset + 1 + 8].try_into()?,
                    ),
                    code: error_log_buf[offset + 1 + 8],
                });
                offset += 1 + 8 + 1;
            }
            b'S' => {
                // u64 + u8 (size) + size * u32
                let size = error_log_buf[offset + 8];
                let mut tmp_addrs = Vec::new();
                for addr in
                    (error_log_buf[offset + 1 + 8..offset + 1 + 8 + size as usize * 4]).chunks(4)
                {
                    tmp_addrs.push(u32::from_be_bytes(addr.try_into()?));
                }

                tmp.push(ErrorLogEntry::Stacktrace {
                    timestamp: u64::from_be_bytes(
                        error_log_buf[offset + 1..offset + 1 + 8].try_into()?,
                    ),
                    addrs: tmp_addrs,
                });
                offset += 1 + 8 + 1 + size as usize * 4;
            }
            _ => {
                break;
            }
        }
    }

    Ok(tmp)
}

#[derive(Debug)]
pub enum ErrorLogEntry {
    Code { timestamp: u64, code: u8 },
    Stacktrace { timestamp: u64, addrs: Vec<u32> },
}
