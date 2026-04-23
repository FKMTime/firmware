use alloc::vec::Vec;
use esp_storage::FlashStorage;

const MAX_BACKTRACE_ADDRESSES: usize = 10;
const RA_OFFSET: usize = 4;
const SAVED_VERSION_LEN: usize = 16;
const SAVED_TIMESTAMP_LEN: usize = 8;

pub async fn read_saved_backtrace() {
    if let Some(nvs_part) = esp_hal_wifimanager::Nvs::read_nvs_partition_offset(unsafe {
        esp_hal::peripherals::FLASH::steal()
    }) {
        let mut flash = FlashStorage::new(unsafe { esp_hal::peripherals::FLASH::steal() });

        let mut buf = [0; 1024];
        let res = embedded_storage::ReadStorage::read(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2) as u32,
            &mut buf[..2],
        );

        if let Err(e) = res {
            log::error!("read_len_err: {e:?}");
            return;
        }

        let len = u16::from_be_bytes([buf[0], buf[1]]);
        if len == 0 || len > 1024 || len == 0xff {
            return;
        }

        let res = embedded_storage::ReadStorage::read(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2 - len as usize) as u32,
            &mut buf[..len as usize],
        );

        if let Err(e) = res {
            log::error!("read_msg_err: {e:?}");
            return;
        }

        const NEW_HEADER_LEN: usize = SAVED_VERSION_LEN + SAVED_TIMESTAMP_LEN;
        let (saved_version, saved_timestamp, addr_data) =
            if len as usize >= NEW_HEADER_LEN && (len as usize - NEW_HEADER_LEN).is_multiple_of(4)
            {
                let version_raw = &buf[..SAVED_VERSION_LEN];
                let version = if let Ok(version) = core::str::from_utf8(version_raw) {
                    version.trim_end_matches('\0')
                } else {
                    crate::version::VERSION
                };
                let timestamp = u64::from_be_bytes(
                    buf[SAVED_VERSION_LEN..NEW_HEADER_LEN]
                        .try_into()
                        .unwrap_or([0; 8]),
                );
                (version, timestamp, &buf[NEW_HEADER_LEN..len as usize])
            } else if len as usize >= SAVED_VERSION_LEN
                && (len as usize - SAVED_VERSION_LEN).is_multiple_of(4)
            {
                // Old format without timestamp
                let version_raw = &buf[..SAVED_VERSION_LEN];
                let version = if let Ok(version) = core::str::from_utf8(version_raw) {
                    version.trim_end_matches('\0')
                } else {
                    crate::version::VERSION
                };
                (
                    version,
                    crate::state::current_epoch(),
                    &buf[SAVED_VERSION_LEN..len as usize],
                )
            } else {
                (
                    crate::version::VERSION,
                    crate::state::current_epoch(),
                    &buf[..len as usize],
                )
            };

        log::error!("Last crash info:");
        let mut addrs = Vec::new();
        for addr in addr_data.chunks(4) {
            if let Ok(addr) = addr.try_into() {
                let addr: u32 = u32::from_be_bytes(addr);
                addrs.push(addr);
                log::error!("0x{:X}", addr);
            }
        }

        _ = embedded_storage::Storage::write(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2) as u32,
            &[0x00, 0x00],
        );

        crate::utils::error_log::add_stacktrace(&addrs, saved_version, saved_timestamp).await;
    }
}

pub fn backtrace() -> [Option<usize>; MAX_BACKTRACE_ADDRESSES] {
    let fp = unsafe {
        let mut _tmp: u32;
        core::arch::asm!("mv {0}, x8", out(reg) _tmp);
        _tmp
    };

    backtrace_internal(fp, 2)
}

pub fn backtrace_internal(fp: u32, suppress: i32) -> [Option<usize>; MAX_BACKTRACE_ADDRESSES] {
    let mut result = [None; 10];
    let mut index = 0;

    let mut fp = fp;
    let mut suppress = suppress;
    let mut old_address = 0;
    loop {
        unsafe {
            let address = (fp as *const u32).offset(-1).read_volatile(); // RA/PC
            fp = (fp as *const u32).offset(-2).read_volatile(); // next FP

            if old_address == address {
                break;
            }

            old_address = address;

            if address == 0 {
                break;
            }

            if !is_valid_ram_address(fp) {
                break;
            }

            if suppress == 0 {
                result[index] = Some(address as usize);
                index += 1;

                if index >= MAX_BACKTRACE_ADDRESSES {
                    break;
                }
            } else {
                suppress -= 1;
            }
        }
    }

    result
}

fn is_valid_ram_address(address: u32) -> bool {
    if (address & 0xF) != 0 {
        return false;
    }

    if !(0x3FC8_0000..=0x3FCE_0000).contains(&address) {
        return false;
    }

    true
}

#[unsafe(no_mangle)]
pub extern "Rust" fn custom_pre_backtrace() {
    let backtrace = backtrace();

    let mut tmp = Vec::new();
    let mut version = [0; SAVED_VERSION_LEN];
    let version_bytes = crate::version::VERSION.as_bytes();
    let version_len = core::cmp::min(version_bytes.len(), SAVED_VERSION_LEN);
    version[..version_len].copy_from_slice(&version_bytes[..version_len]);
    tmp.extend_from_slice(&version);
    tmp.extend_from_slice(&crate::state::current_epoch().to_be_bytes());
    for addr in backtrace.into_iter().flatten() {
        tmp.extend_from_slice(&((addr - RA_OFFSET) as u32).to_be_bytes());
    }

    if let Some(nvs_part) = esp_hal_wifimanager::Nvs::read_nvs_partition_offset(unsafe {
        esp_hal::peripherals::FLASH::steal()
    }) {
        let mut flash = FlashStorage::new(unsafe { esp_hal::peripherals::FLASH::steal() });
        _ = embedded_storage::Storage::write(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2) as u32,
            &(tmp.len() as u16).to_be_bytes(),
        );

        _ = embedded_storage::Storage::write(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2 - tmp.len()) as u32,
            &tmp,
        );
    }

    let delay = esp_hal::delay::Delay::new();
    delay.delay_millis(100);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn custom_halt() {
    esp_hal::system::software_reset();
}
