#[cfg(feature = "sleep")]
pub const SLEEP_AFTER_MS: u64 = 60000 * 5;
#[cfg(feature = "sleep")]
pub const DEEPER_SLEEP_AFTER_MS: u64 = 60000 * 15;

#[cfg(not(feature = "sleep"))]
pub const SLEEP_AFTER_MS: u64 = 60000 * 9999;
#[cfg(not(feature = "sleep"))]
pub const DEEPER_SLEEP_AFTER_MS: u64 = 60000 * 99999;

pub const LOG_SEND_INTERVAL_MS: u64 = 5000;
pub const PRINT_HEAP_INTERVAL_MS: u64 = 30000;

pub const BATTERY_SEND_INTERVAL_MS: u64 = 60000;

pub const SCROLL_TICKER_INVERVAL_MS: u64 = 500;
pub const LCD_INSPECTION_FRAME_TIME: u64 = 1000 / 30;

pub const RFID_RETRY_INIT_MS: u64 = 1500;
pub const WS_RETRY_MS: u64 = 1000;

pub const MDNS_RESEND_INTERVAL: u64 = 500;

pub const INSPECTION_TIME_DNF: u64 = 17000;
pub const INSPECTION_TIME_PLUS2: u64 = 15000;
