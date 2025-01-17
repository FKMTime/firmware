pub mod arc;
pub mod backtrace_store;
pub mod buttons;
pub mod lcd_abstract;
pub mod logger;
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
