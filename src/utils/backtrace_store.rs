//! Crash record kept in RTC fast RAM (survives software and watchdog resets, not power loss).
//!
//! The panic handler only does plain atomic stores here: no locks, no heap, no flash.
//! The record is turned into an error log entry on the next boot.

use portable_atomic::{AtomicU32, Ordering};

const MAX_BACKTRACE_ADDRESSES: usize = 10;
const RA_OFFSET: usize = 4;
const VERSION_WORDS: usize = 4;

const MAGIC: u32 = 0xF4C0_A5E1;
const KIND_NONE: u32 = 0;
const KIND_PANIC: u32 = 1;
const KIND_WATCHDOG: u32 = 2;

// word layout of `CRASH`
const W_MAGIC: usize = 0;
const W_KIND: usize = 1;
const W_CONSECUTIVE: usize = 2;
const W_TASK: usize = 3;
const W_EPOCH: usize = 4; // 2 words
const W_VERSION: usize = 6; // VERSION_WORDS words
const W_ADDR_COUNT: usize = W_VERSION + VERSION_WORDS;
const W_ADDRS: usize = W_ADDR_COUNT + 1;
const W_CHECKSUM: usize = W_ADDRS + MAX_BACKTRACE_ADDRESSES;
const WORDS: usize = W_CHECKSUM + 1;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU32 = AtomicU32::new(0);

// Any bit pattern is valid (power-on garbage fails the magic/checksum check).
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static CRASH: [AtomicU32; WORDS] = [ZERO; WORDS];

fn get(w: usize) -> u32 {
    CRASH[w].load(Ordering::Relaxed)
}

fn set(w: usize, v: u32) {
    CRASH[w].store(v, Ordering::Relaxed);
}

fn checksum() -> u32 {
    (W_KIND..W_CHECKSUM).fold(MAGIC, |acc, w| acc.rotate_left(5) ^ get(w))
}

fn is_valid() -> bool {
    get(W_MAGIC) == MAGIC && get(W_CHECKSUM) == checksum()
}

fn seal() {
    set(W_MAGIC, MAGIC);
    set(W_CHECKSUM, checksum());
}

/// Starts a new record (consecutive crash count carries over a valid previous one).
fn record(kind: u32, task: u32, addrs: &[usize]) {
    let consecutive = if is_valid() { get(W_CONSECUTIVE) } else { 0 };
    set(W_KIND, kind);
    set(W_CONSECUTIVE, consecutive.saturating_add(1));
    set(W_TASK, task);

    let epoch = crate::state::current_epoch();
    set(W_EPOCH, epoch as u32);
    set(W_EPOCH + 1, (epoch >> 32) as u32);

    let mut version = [0u8; VERSION_WORDS * 4];
    let bytes = crate::version::VERSION.as_bytes();
    let len = bytes.len().min(version.len());
    version[..len].copy_from_slice(&bytes[..len]);
    for (i, chunk) in version.as_chunks::<4>().0.iter().enumerate() {
        set(W_VERSION + i, u32::from_le_bytes(*chunk));
    }

    let count = addrs.len().min(MAX_BACKTRACE_ADDRESSES);
    set(W_ADDR_COUNT, count as u32);
    for (i, addr) in addrs.iter().take(count).enumerate() {
        set(W_ADDRS + i, addr.saturating_sub(RA_OFFSET) as u32);
    }

    seal();
}

/// Called by the task watchdog right before it lets the hardware watchdog reset the chip.
pub fn record_watchdog(task: u8) {
    record(KIND_WATCHDOG, task as u32, &[]);
}

/// Moves the crash record (if any) into the error log. Call after `load_error_log`.
pub fn report_saved_crash() {
    if !is_valid() {
        set(W_KIND, KIND_NONE);
        set(W_CONSECUTIVE, 0);
        seal();
        return;
    }

    let consecutive = get(W_CONSECUTIVE);
    match get(W_KIND) {
        KIND_PANIC => {
            let mut version = [0u8; VERSION_WORDS * 4];
            for (i, chunk) in version.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *chunk = get(W_VERSION + i).to_le_bytes();
            }
            let version = core::str::from_utf8(&version)
                .unwrap_or(crate::version::VERSION)
                .trim_end_matches('\0');
            let epoch = (get(W_EPOCH) as u64) | ((get(W_EPOCH + 1) as u64) << 32);

            let mut addrs = [0u32; MAX_BACKTRACE_ADDRESSES];
            let count = (get(W_ADDR_COUNT) as usize).min(MAX_BACKTRACE_ADDRESSES);
            for (i, addr) in addrs.iter_mut().take(count).enumerate() {
                *addr = get(W_ADDRS + i);
            }

            log::error!("Last crash info:");
            for addr in &addrs[..count] {
                log::error!("0x{addr:X}");
            }
            crate::utils::error_log::add_stacktrace(&addrs[..count], version, epoch);
        }
        KIND_WATCHDOG => {
            log::error!("Last reset: task watchdog (task {})", get(W_TASK));
            crate::utils::error_log::add_error(crate::utils::error_log::codes::WATCHDOG_RESET);
        }
        _ => {}
    }

    if consecutive >= 2 {
        log::error!("{consecutive} crashes in a row!");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::REPEATED_CRASHES);
    }

    set(W_KIND, KIND_NONE);
    seal();
}

/// Called once the firmware has run fine for a while: the next crash starts a new count.
pub fn mark_healthy() {
    if is_valid() && get(W_CONSECUTIVE) != 0 {
        set(W_CONSECUTIVE, 0);
        seal();
    }
}

fn backtrace() -> [Option<usize>; MAX_BACKTRACE_ADDRESSES] {
    let fp = unsafe {
        let mut _tmp: u32;
        core::arch::asm!("mv {0}, x8", out(reg) _tmp);
        _tmp
    };

    backtrace_internal(fp, 2)
}

fn backtrace_internal(fp: u32, suppress: i32) -> [Option<usize>; MAX_BACKTRACE_ADDRESSES] {
    let mut result = [None; MAX_BACKTRACE_ADDRESSES];
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
    let mut addrs = [0usize; MAX_BACKTRACE_ADDRESSES];
    let mut count = 0;
    for addr in backtrace.into_iter().flatten() {
        addrs[count] = addr;
        count += 1;
    }

    record(KIND_PANIC, 0, &addrs[..count]);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn custom_halt() {
    esp_hal::system::software_reset();
}
