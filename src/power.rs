//! Sleep state machine: Awake → Sleeping (radio off) → DeepSleep (chip off, button wakes it).
//!
//! Display/input code only calls [`request_sleep`] / [`wake`] / [`enter_deep_sleep`];
//! [`Supervisor::tick`] (main loop) does the radio, websocket and BLE side of each transition.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant};
use portable_atomic::{AtomicBool, Ordering};
use esp_hal_wifimanager::WifiManager;

pub type SleepSignal = Signal<CriticalSectionRawMutex, bool>;

/// Time between the websocket close frame and the radio going off; also debounces quick wakes.
const SLEEP_SETTLE: Duration = Duration::from_secs(15);

static SLEEPING: AtomicBool = AtomicBool::new(false);
/// v3 only: the cpu runs at 10 MHz until the next reset.
static DEEPER: AtomicBool = AtomicBool::new(false);

pub fn is_sleeping() -> bool {
    SLEEPING.load(Ordering::Relaxed)
}

#[cfg_attr(not(feature = "v3"), allow(dead_code))]
pub fn is_deeper_sleeping() -> bool {
    DEEPER.load(Ordering::Relaxed)
}

#[cfg_attr(any(feature = "e2e", feature = "qa"), allow(dead_code))] // sleep is off in e2e/qa
pub fn request_sleep() {
    SLEEPING.store(true, Ordering::Relaxed);
}

pub fn wake() {
    SLEEPING.store(false, Ordering::Relaxed);
}

/// Powers the chip down until one of the four buttons is pressed (wake = reset).
#[cfg(feature = "v4")]
#[cfg_attr(any(feature = "e2e", feature = "qa"), allow(dead_code))]
pub fn enter_deep_sleep() -> ! {
    use esp_hal::gpio::{Event, Input, InputConfig, Pin, Pull, WakeupConfig};
    use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};

    // SAFETY: called right before deep sleep, after which nothing else uses these pins.
    let (pins, lpwr) = unsafe {
        (
            [
                esp_hal::peripherals::GPIO0::steal().degrade(),
                esp_hal::peripherals::GPIO1::steal().degrade(),
                esp_hal::peripherals::GPIO2::steal().degrade(),
                esp_hal::peripherals::GPIO3::steal().degrade(),
            ],
            esp_hal::peripherals::LPWR::steal(),
        )
    };

    // pull-down like the old RtcioWakeupSource did for a high-level wake
    let input_config = InputConfig::default().with_pull(Pull::Down);
    let wakeup_config = WakeupConfig::default().with_low_power_path(true);
    let mut wakeup_pins = pins.map(|pin| Input::new(pin, input_config));
    for pin in &mut wakeup_pins {
        if let Err(e) = pin.apply_wakeup_config(&wakeup_config) {
            log::error!("Wakeup pin config failed: {e:?}");
        }
        pin.listen(Event::HighLevel);
    }

    LowPower::new(lpwr).sleep_deep(RtcSleepConfig::deep())
}

/// v3 has no wakeable deep sleep: the cpu clock drops to 10 MHz until the next reset.
#[cfg(feature = "v3")]
#[cfg_attr(any(feature = "e2e", feature = "qa"), allow(dead_code))]
pub fn enter_deep_sleep() {
    crate::utils::deeper_sleep();
    DEEPER.store(true, Ordering::Relaxed);
}

/// Applies sleep transitions to the radio, websocket and BLE tasks.
pub struct Supervisor<'a> {
    wifi: &'a WifiManager,
    sleeping: bool,
    /// Set while waiting [`SLEEP_SETTLE`] before turning the radio off.
    settling_since: Option<Instant>,
}

impl<'a> Supervisor<'a> {
    pub fn new(wifi: &'a WifiManager) -> Self {
        Self {
            wifi,
            sleeping: false,
            settling_since: None,
        }
    }

    /// Call periodically (never blocks for long).
    pub async fn tick(&mut self) {
        let sleeping = is_sleeping();

        if let Some(since) = self.settling_since {
            if !sleeping {
                // woken while settling: reset the ws state, the next tick restarts everything
                self.settling_since = None;
                crate::ws::SLEEP.signal(true);
            } else if since.elapsed() >= SLEEP_SETTLE {
                self.settling_since = None;
                crate::ws::SLEEP.signal(true);
                self.wifi.stop_radio();
            }
            return;
        }

        if sleeping == self.sleeping {
            return;
        }

        self.sleeping = sleeping;
        crate::bluetooth::SLEEP.signal(sleeping);
        if sleeping {
            log::warn!("Entering sleep mode!");
            crate::ws::close(4000, "Going into sleep mode");
            self.settling_since = Some(Instant::now());
        } else {
            crate::ws::SLEEP.signal(false);
            self.wifi.restart_radio();
        }
    }
}
