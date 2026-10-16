use embassy_executor::{SpawnError, SpawnToken, Spawner};

pub mod backtrace_store;
pub mod battery;
pub mod bounded;
pub mod buttons;
pub mod error_log;
pub mod logger;
pub mod ring_log;
pub mod stackmat;
pub mod text;

/// A buffer for a `#[link_section = ".dram2_uninit"]` static (that region is not
/// zeroed at boot, so `StaticCell`/`Mutex` state can't live there). Callers must
/// serialize access themselves (a critical section) and call [`UninitBuf::get`]
/// only from inside it.
pub struct UninitBuf<const N: usize>(core::cell::UnsafeCell<core::mem::MaybeUninit<[u8; N]>>);

// SAFETY: every access goes through `get`, whose contract requires exclusive access.
unsafe impl<const N: usize> Sync for UninitBuf<N> {}

impl<const N: usize> UninitBuf<N> {
    pub const fn new() -> Self {
        Self(core::cell::UnsafeCell::new(core::mem::MaybeUninit::uninit()))
    }

    /// Zeroes the buffer on first use (`initialized` is the caller's flag).
    ///
    /// # Safety
    /// The caller must hold exclusive access (e.g. a critical section) for the whole
    /// lifetime of the returned reference, and use one `initialized` flag per buffer.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self, initialized: &mut bool) -> &mut [u8; N] {
        let ptr = self.0.get();
        // SAFETY: exclusive access is guaranteed by the caller.
        unsafe {
            if !*initialized {
                (*ptr).write([0; N]);
                *initialized = true;
            }
            (*ptr).assume_init_mut()
        }
    }
}

/// Records error `code` only the first time this call site runs (per boot).
#[macro_export]
macro_rules! log_once {
    ($code:expr) => {{
        static LOGGED: portable_atomic::AtomicBool = portable_atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, portable_atomic::Ordering::Relaxed) {
            $crate::utils::error_log::add_error($code);
        }
    }};
}

/// Spawns a task; the firmware can't work without any of its tasks, so failure resets.
pub fn spawn_task<T>(
    spawner: &Spawner,
    task_name: &str,
    token_res: Result<SpawnToken<T>, SpawnError>,
) {
    match token_res {
        Ok(token) => {
            spawner.spawn(token);
        }
        Err(e) => {
            log::error!("Cannot create task {task_name}: {e:?}, resetting");
            crate::utils::error_log::add_error(crate::utils::error_log::codes::TASK_SPAWN_FAILED);
            esp_hal::system::software_reset();
        }
    }
}

#[cfg(feature = "v3")]
pub mod lcd_text;

#[cfg(feature = "v4")]
pub mod shared_i2c;

pub mod screen;

#[cfg(feature = "v4")]
pub mod oled_ui;

#[cfg(feature = "v4")]
pub mod ssd1309;

pub fn set_brownout_detection(state: bool) {
    unsafe {
        let rtc_cntl = &*esp32c3::RTC_CNTL::ptr();
        rtc_cntl.int_ena().modify(|_, w| w.brown_out().bit(state));
    }
}

pub fn get_random_u64() -> u64 {
    let mut buf = [0; 8];
    _ = getrandom::getrandom(&mut buf);
    u64::from_be_bytes(buf)
}

/// This function returns value with maximum of signed integer
/// (2147483647) to easily store it in postgres db as integer
pub fn get_efuse_u32() -> u32 {
    let mut efuse = esp_hal_wifimanager::get_efuse_mac();
    efuse = (!efuse).wrapping_add(efuse << 18);
    efuse = efuse ^ (efuse >> 31);
    efuse = efuse.wrapping_mul(21);
    efuse = efuse ^ (efuse >> 11);
    efuse = efuse.wrapping_add(efuse << 6);
    efuse = efuse ^ (efuse >> 22);

    let mac = efuse & 0x000000007FFFFFFF;
    mac as u32
}

/// Sets the cpu clock to 10 MHz (only a reset brings it back).
#[cfg(feature = "v3")]
pub fn deeper_sleep() {
    use esp32c3::{RTC_CNTL, SYSTEM};
    unsafe extern "C" {
        fn ets_update_cpu_frequency(ticks_per_us: u32);
    }

    let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
    let system = unsafe { &*SYSTEM::ptr() };

    const TARGET_FREQ_MHZ: u32 = 10;
    const TARGET_FREQ_HZ: u32 = TARGET_FREQ_MHZ * 1_000_000;

    unsafe {
        let divider = 4u16;
        system.sysclk_conf().modify(|_, w| w.pre_div_cnt().bits(0));

        system
            .sysclk_conf()
            .modify(|_, w| w.pre_div_cnt().bits(divider - 1));

        system.sysclk_conf().modify(|_, w| w.soc_clk_sel().bits(0));
        ets_update_cpu_frequency(TARGET_FREQ_MHZ);

        let freq_value = (TARGET_FREQ_HZ >> 12) & 0xFFFF;
        rtc_cntl
            .store5()
            .write(|w| w.data().bits(freq_value | (freq_value << 16)));
    }
}
