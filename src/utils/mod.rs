pub mod arc;
pub mod backtrace_store;
pub mod buttons;
pub mod lcd_abstract;
pub mod logger;
pub mod rolling_average;
pub mod signaled_mutex;
pub mod stackmat;

pub fn set_brownout_detection(state: bool) {
    #[cfg(feature = "esp32c3")]
    unsafe {
        let rtc_cntl = &*esp32c3::RTC_CNTL::ptr();
        rtc_cntl.int_ena().modify(|_, w| w.brown_out().bit(state));
    }

    #[cfg(feature = "esp32")]
    unsafe {
        let rtc_cntl = &*esp32::RTC_CNTL::ptr();
        rtc_cntl.int_ena().modify(|_, w| w.brown_out().bit(state));
    }
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

#[cfg(not(feature = "esp32c3"))]
pub fn deeper_sleep() {}

/// Sets cpu clock to 10mHz (not reversable)
#[cfg(feature = "esp32c3")]
pub fn deeper_sleep() {
    esp32c3_rtc_update_to_xtal();
    esp32c3_rtc_apb_freq_update();

    unsafe { crate::state::DEEPER_SLEEP = true };
}

#[cfg(feature = "esp32c3")]
#[allow(unused)]
#[inline(always)]
fn ets_update_cpu_frequency_rom(ticks_per_us: u32) {
    extern "C" {
        fn ets_update_cpu_frequency(ticks_per_us: u32);
    }

    unsafe { ets_update_cpu_frequency(ticks_per_us) };
}

#[cfg(feature = "esp32c3")]
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

#[cfg(feature = "esp32c3")]
fn esp32c3_rtc_apb_freq_update() {
    let hz = 10000000;
    let rtc_cntl = unsafe { &*esp32c3::RTC_CNTL::ptr() };
    let value = ((hz >> 12) & u16::MAX as u32) | (((hz >> 12) & u16::MAX as u32) << 16);

    rtc_cntl
        .store5()
        .modify(|_, w| unsafe { w.scratch5().bits(value) });
}
