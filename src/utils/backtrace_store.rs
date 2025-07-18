use esp_storage::FlashStorage;

const MAX_BACKTRACE_ADDRESSES: usize = 10;
const RA_OFFSET: usize = 4;

pub async fn read_saved_backtrace() {
    if let Some(nvs_part) = esp_hal_wifimanager::Nvs::read_nvs_partition_offset() {
        let mut flash = FlashStorage::new();

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

        let msg = core::str::from_utf8(&buf[..len as usize]);
        if let Ok(msg) = msg {
            log::error!("Last crash info:\n{msg}");
        }

        _ = embedded_storage::Storage::write(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2) as u32,
            &[0x00, 0x00],
        );
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

    let mut tmp = alloc::string::String::new();
    if backtrace.iter().filter(|e| e.is_some()).count() == 0 {
        tmp.push_str("No backtrace available - make sure to force frame-pointers. (see https://crates.io/crates/esp-backtrace)\n");
    }
    for addr in backtrace.into_iter().flatten() {
        tmp.push_str(&alloc::format!("0x{:x}\n", addr - RA_OFFSET));
    }

    if let Some(nvs_part) = esp_hal_wifimanager::Nvs::read_nvs_partition_offset() {
        let mut flash = FlashStorage::new();
        _ = embedded_storage::Storage::write(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2) as u32,
            &(tmp.len() as u16).to_be_bytes(),
        );

        let tmp = tmp.as_bytes();
        _ = embedded_storage::Storage::write(
            &mut flash,
            (nvs_part.0 + nvs_part.1 - 2 - tmp.len()) as u32,
            tmp,
        );
    }

    let delay = esp_hal::delay::Delay::new();
    delay.delay_millis(100);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn custom_halt() {
    esp_hal::system::software_reset();
}
