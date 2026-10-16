/// The device never sleeps without the `sleep` feature or in e2e/qa/sim builds.
pub const SLEEP_ENABLED: bool = cfg!(all(
    feature = "sleep",
    not(any(feature = "e2e", feature = "qa", feature = "sim"))
));
pub const SLEEP_AFTER_MS: u64 = 60000 * 5;
pub const DEEPER_SLEEP_AFTER_MS: u64 = 60000 * 15;

pub const PRINT_HEAP_INTERVAL_MS: u64 = 30000;
pub const LOG_SEND_INTERVAL_MS: u64 = 1000;

pub const BATTERY_SEND_INTERVAL_MS: u64 = 60000;

/// v3 long text scrolls one step per tick.
#[cfg(feature = "v3")]
pub const SCROLL_TICK_MS: u64 = 500;
pub const LCD_INSPECTION_FRAME_TIME: u64 = 1000 / 30;

pub const RFID_RETRY_INIT_MS: u64 = 1500;
pub const WS_RETRY_MS: u64 = 1000;
/// Total time for a request (send + tagged response); was 5 s send + 5 s response.
pub const WS_REQUEST_TIMEOUT_MS: u64 = 10_000;
/// The delegate answers on the web panel, so wait long (the buttons task is blocked).
#[cfg_attr(feature = "qa", allow(dead_code))]
pub const DELEGATE_TIMEOUT_MS: u64 = 5 * 60_000;

/// v3 buttons are shift-register scanned with no hardware debounce / strong
/// pull-down filtering; require this long of a stable sample before edges fire.
#[cfg(feature = "v3")]
pub const BUTTON_DEBOUNCE_MS: u64 = 30;

pub const MDNS_RESEND_INTERVAL: u64 = 500;

pub const INSPECTION_TIME_DNF: u64 = 17000;
pub const INSPECTION_TIME_PLUS2: u64 = 15000;

#[cfg(feature = "v4")]
pub const NVS_BUZZER_VOLUME: esp_hal_wifimanager::Key =
    esp_hal_wifimanager::Key::from_str("BUZZER_VOLUME");
#[cfg(feature = "v4")]
pub const BUZZER_VOLUME_MIN: u8 = 0;
#[cfg(feature = "v4")]
pub const BUZZER_VOLUME_MAX: u8 = 25;
#[cfg(feature = "v4")]
pub const BUZZER_VOLUME_DEFAULT: u8 = 5;

/// Both hands must rest on the pads this long before the timer arms (like a stackmat).
#[cfg(feature = "timer-func")]
pub const PADS_ARM_MS: u64 = 550;

pub const NVS_BONDING_KEY: esp_hal_wifimanager::Key =
    esp_hal_wifimanager::Key::from_str("BONDING_KEY");
pub const NVS_SIGN_KEY: esp_hal_wifimanager::Key = esp_hal_wifimanager::Key::from_str("SIGN_KEY");
pub const NVS_SAVED_STATE: esp_hal_wifimanager::Key =
    esp_hal_wifimanager::Key::from_str("SAVED_STATE");
pub const NVS_ERROR_LOG: esp_hal_wifimanager::Key = esp_hal_wifimanager::Key::from_str("ERROR_LOG");
