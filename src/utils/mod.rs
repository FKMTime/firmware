pub mod arc;
pub mod backtrace_store;
pub mod buttons;
pub mod lcd_abstract;
pub mod logger;
pub mod rolling_average;
pub mod signaled_mutex;
pub mod stackmat;

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

/// Sets cpu clock to 10mHz (not reversable)
pub fn deeper_sleep() {
    esp32c3_set_cpu_freq_10mhz();

    unsafe { crate::state::DEEPER_SLEEP = true };
}

#[allow(unused)]
#[inline(always)]
fn ets_update_cpu_frequency_rom(ticks_per_us: u32) {
    unsafe extern "C" {
        fn ets_update_cpu_frequency(ticks_per_us: u32);
    }

    unsafe { ets_update_cpu_frequency(ticks_per_us) };
}

fn esp32c3_set_cpu_freq_10mhz() {
    use esp32c3::{RTC_CNTL, SYSTEM};

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
        ets_update_cpu_frequency_rom(TARGET_FREQ_MHZ);

        let freq_value = (TARGET_FREQ_HZ >> 12) & 0xFFFF;
        rtc_cntl
            .store5()
            .write(|w| w.data().bits(freq_value | (freq_value << 16)));
    }
}
