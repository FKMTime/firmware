use esp32c3::RTC_CNTL;

pub mod arc;
pub mod buttons;
pub mod lcd_abstract;
pub mod signaled_mutex;
pub mod stackmat;

pub fn set_brownout_detection(state: bool) {
    unsafe {
        let rtc_cntl = &*RTC_CNTL::ptr();
        rtc_cntl.int_ena().modify(|_, w| w.brown_out().bit(state));
    }
}
