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
    esp32c3_rtc_update_to_xtal();
    esp32c3_rtc_apb_freq_update();

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

fn esp32c3_rtc_update_to_xtal() {
    let _div = 1;
    ets_update_cpu_frequency_rom(10);

    let system_control = unsafe { &*esp32c3::SYSTEM::ptr() };
    unsafe {
        // Set divider from XTAL to APB clock. Need to set divider to 1 (reg. value 0)
        // first.
        system_control.sysclk_conf().modify(|_, w| {
            w.pre_div_cnt()
                .bits(0)
                .pre_div_cnt()
                .bits((_div - 1) as u16)
        });

        // No need to adjust the REF_TICK

        // Switch clock source
        system_control
            .sysclk_conf()
            .modify(|_, w| w.soc_clk_sel().bits(0));
    }
}

fn esp32c3_rtc_apb_freq_update() {
    let hz = 10000000;
    let rtc_cntl = unsafe { &*esp32c3::RTC_CNTL::ptr() };
    let value = ((hz >> 12) & u16::MAX as u32) | (((hz >> 12) & u16::MAX as u32) << 16);

    rtc_cntl
        .store5()
        .modify(|_, w| unsafe { w.data().bits(value) });
}
