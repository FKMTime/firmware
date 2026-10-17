//! esp-emu simulation build (feature `sim`, see sim/README.md).
//!
//! The emulator has no I2C devices, no GPIO input and prints only UART0, so this module stands
//! in for them: logs go to UART0, the OLED is a fake I2C SSD1309 printed as text, and stdin
//! (forwarded to UART0 RX) drives the buttons, cards and battery.

#[cfg(any(feature = "v3", feature = "timer-func"))]
compile_error!("feature `sim` targets v4 with the UART stackmat (--no-default-features)");

use crate::state::GlobalState;
use core::{cell::RefCell, fmt::Write};
use embassy_sync::blocking_mutex::{CriticalSectionMutex, raw::CriticalSectionRawMutex};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use embedded_hal::i2c::Operation;
use esp_hal::{
    Async, Blocking,
    i2c::master::{AcknowledgeCheckFailedReason, Error as I2cError},
    uart::{Config, Uart, UartRx, UartTx},
};

/// Scanned by the fake RFID reader (`card <uid>`).
pub static CARD: Signal<CriticalSectionRawMutex, u128> = Signal::new();
/// `wifi off|on`: `true` stops the radio (driver torn down), `false` restarts it
pub static WIFI: Signal<CriticalSectionRawMutex, bool> = Signal::new();
/// `oled raw`: frames as `@oled <hex>` every 20 ms (sim/web.py) instead of text art every 200 ms
static OLED_RAW: portable_atomic::AtomicBool = portable_atomic::AtomicBool::new(false);

static TX: CriticalSectionMutex<RefCell<Option<UartTx<'static, Blocking>>>> =
    CriticalSectionMutex::new(RefCell::new(None));
/// Release instant (µs) of each held button.
static BUTTONS: CriticalSectionMutex<RefCell<[u64; 4]>> =
    CriticalSectionMutex::new(RefCell::new([0; 4]));
static OLED: CriticalSectionMutex<RefCell<Oled>> = CriticalSectionMutex::new(RefCell::new(Oled {
    gram: [0; 1024],
    page: 0,
    col: 0,
    dirty: false,
}));

/// Takes UART0 for logs (TX) and commands (RX). Call before the first log line.
pub fn init() -> Option<UartRx<'static, Async>> {
    // SAFETY: nothing else in the firmware uses UART0 (esp-println isn't used in sim builds)
    let uart0 = unsafe { esp_hal::peripherals::UART0::steal() };
    let (rx, tx) = Uart::new(uart0, Config::default()).ok()?.split();
    TX.lock(|t| *t.borrow_mut() = Some(tx));
    Some(rx.into_async())
}

pub fn print(args: core::fmt::Arguments) {
    struct W<'a>(&'a mut UartTx<'static, Blocking>);
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let mut b = s.as_bytes();
            while !b.is_empty() {
                let n = self.0.write(b).map_err(|_| core::fmt::Error)?;
                b = &b[n..];
            }
            Ok(())
        }
    }
    TX.lock(|t| {
        if let Some(tx) = t.borrow_mut().as_mut() {
            _ = W(tx).write_fmt(args);
            _ = W(tx).write_str("\r\n");
        }
    });
}

/// Mask of the buttons currently held by `btn` commands (bit 0 = first button).
pub fn button_mask() -> u8 {
    let now = Instant::now().as_micros();
    BUTTONS.lock(|b| {
        b.borrow()
            .iter()
            .enumerate()
            .filter(|(_, until)| **until > now)
            .fold(0, |m, (i, _)| m | 1 << i)
    })
}

/// Seeds the emulator's Wi-Fi (`myssid`/`mypassword`) and server url into NVS, so the
/// wifimanager connects right away instead of starting AP/BLE provisioning
/// (`SIM_PROVISION=1` at build time skips this to test provisioning).
pub async fn seed_wifi(nvs: &esp_hal_wifimanager::Nvs) {
    if option_env!("SIM_PROVISION").is_some()
        || nvs
            .get::<alloc::string::String>(&esp_hal_wifimanager::WIFI_NVS_KEY)
            .await
            .is_ok()
    {
        return;
    }
    // SIM_WS_URL=ws://192.168.4.1:8080 at build time skips mdns (192.168.4.1 = host localhost)
    let mut setup = heapless::String::<320>::new();
    _ = match option_env!("SIM_WS_URL") {
        Some(url) => write!(
            setup,
            r#"{{"ssid":"myssid","psk":"mypassword","data":{{"mdns":false,"ws_url":"{url}"}}}}"#
        ),
        None => write!(
            setup,
            r#"{{"ssid":"myssid","psk":"mypassword","data":{{"mdns":true,"ws_url":null}}}}"#
        ),
    };
    if let Err(e) = nvs
        .set(&esp_hal_wifimanager::WIFI_NVS_KEY, setup.as_str())
        .await
    {
        log::error!("[sim] wifi seed failed: {e:?}");
    }
}

/// The emulated I2C bus: only the SSD1309 (0x3C) answers.
pub fn i2c(address: u8, operations: &mut [Operation<'_>]) -> Result<(), I2cError> {
    if address != 0x3C {
        return Err(I2cError::AcknowledgeCheckFailed(
            AcknowledgeCheckFailedReason::Address,
        ));
    }
    OLED.lock(|o| {
        let mut o = o.borrow_mut();
        for op in operations {
            match op {
                Operation::Write(bytes) => o.write(bytes),
                Operation::Read(buf) => buf.fill(0),
            }
        }
    });
    Ok(())
}

/// Fake SSD1309: page addressing (what the driver uses) into a 1 KB GRAM.
struct Oled {
    gram: [u8; 1024],
    page: usize,
    col: usize,
    dirty: bool,
}

impl Oled {
    fn write(&mut self, bytes: &[u8]) {
        match bytes {
            // set page + column (the driver sends exactly these before each page's data)
            [0x00, page @ 0xB0..=0xB7, lo @ 0x00..=0x0F, hi @ 0x10..=0x17] => {
                self.page = usize::from(page & 0x07);
                self.col = usize::from(lo | (hi & 0x0F) << 4);
            }
            [0x40, data @ ..] => {
                for &b in data {
                    if self.col < 128 {
                        self.gram[self.page * 128 + self.col] = b;
                        self.col += 1;
                    }
                }
                self.dirty = true;
            }
            _ => {} // other commands (init, contrast, …) don't change the picture
        }
    }

    fn pixel(&self, x: usize, y: usize) -> bool {
        self.gram[y / 8 * 128 + x] & 1 << (y % 8) != 0
    }
}

/// Prints the OLED (1×2 pixels per character) if it changed since the last print.
fn print_oled() {
    let Some(gram) = OLED.lock(|o| {
        let mut o = o.borrow_mut();
        core::mem::take(&mut o.dirty).then(|| Oled { ..*o })
    }) else {
        return;
    };

    if OLED_RAW.load(portable_atomic::Ordering::Relaxed) {
        let mut line = heapless::String::<{ 6 + 2048 }>::new();
        _ = line.push_str("@oled ");
        for b in gram.gram {
            _ = write!(line, "{b:02x}");
        }
        print(format_args!("{line}"));
        return;
    }

    print(format_args!("┌{:─<128}┐", ""));
    for row in 0..32 {
        let mut line = heapless::String::<{ 128 * 3 + 8 }>::new();
        _ = line.push('│');
        for x in 0..128 {
            _ = line.push(match (gram.pixel(x, row * 2), gram.pixel(x, row * 2 + 1)) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            });
        }
        _ = line.push('│');
        print(format_args!("{line}"));
    }
    print(format_args!("└{:─<128}┘", ""));
}

const HELP: &str = "sim commands: btn <1-4> [ms] | card <uid> | bat <0-100> | screen | state | wifi off|on | help (btn 3 = submit)";

/// One machine-readable line for sim/monkey.py: `@state key=value ...` (`-` = none).
fn print_state(state: GlobalState) {
    let mut line = heapless::String::<512>::new();
    let opt = |v: Option<u64>| v.map_or(-1, |v| v as i64);
    state.solve.read(|s| {
        _ = write!(
            line,
            "@state scene={:?} comp={} judge={} time={} pen={} insp={}{} conf={} group={} groups={} sel={} dhold={}",
            s.scene,
            opt(s.current_competitor),
            opt(s.current_judge),
            opt(s.solve_time),
            s.penalty.map_or(-99, i64::from),
            u8::from(s.inspection_start.is_some()),
            u8::from(s.inspection_end.is_some()),
            u8::from(s.time_confirmed),
            s.solve_group.as_ref().map_or("-", |g| g.group_id.as_str()),
            s.possible_groups.len(),
            s.group_selected_idx,
            s.delegate_hold.map_or(-1, i64::from),
        );
    });
    state.ui.read(|u| {
        _ = write!(
            line,
            " menu={:?} cfg={} err={} msg={} el={}/{} eo={}",
            u.menu_scene,
            u.selected_config_menu.map_or(-1, |v| v as i64),
            u8::from(u.error_text.is_some()),
            u8::from(u.custom_message.is_some()),
            u.selected_error_log_item,
            crate::utils::error_log::entry_count() + 1,
            u8::from(u.selected_error_log_entry.is_some()),
        );
    });
    state.status.read(|s| {
        _ = write!(
            line,
            " wifi={:?} srv={:?} stk={:?} added={:?}",
            s.wifi_connected, s.server_connected, s.stackmat_connected, s.device_added
        );
    });
    _ = write!(line, " errs=");
    for i in 0..crate::utils::error_log::entry_count() {
        match crate::utils::error_log::entry(i) {
            Some(crate::utils::error_log::ErrorLogEntry::Code { code, .. }) => {
                _ = write!(line, "{code},")
            }
            Some(_) => _ = write!(line, "S,"),
            None => _ = write!(line, "?,"),
        }
    }
    _ = write!(
        line,
        " timer={:?} held={} up={}",
        state.timer.try_get(),
        button_mask(),
        Instant::now().as_millis()
    );
    print(format_args!("{line}"));
}

/// Reads stdin commands and prints the OLED when it changes (every 200 ms, 20 ms in raw mode).
#[embassy_executor::task]
pub async fn command_task(mut rx: UartRx<'static, Async>, state: GlobalState) {
    use embassy_futures::select::{Either, select};

    set_battery(state, 100);
    print(format_args!("{HELP}"));

    let mut line = heapless::Vec::<u8, 64>::new();
    let mut buf = [0u8; 32];
    loop {
        match select(
            rx.read_async(&mut buf),
            Timer::after(Duration::from_millis(
                if OLED_RAW.load(portable_atomic::Ordering::Relaxed) { 20 } else { 200 },
            )),
        )
        .await
        {
            Either::First(Ok(n)) => {
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            command(core::str::from_utf8(&line).unwrap_or(""), state);
                        }
                        line.clear();
                    } else if line.push(b).is_err() {
                        line.clear(); // over-long garbage: drop the line
                    }
                }
            }
            Either::First(Err(_)) => line.clear(),
            Either::Second(()) => print_oled(),
        }
    }
}

fn command(line: &str, state: GlobalState) {
    let mut words = line.split_whitespace();
    let (cmd, a, b) = (words.next(), words.next(), words.next());
    let num = |s: Option<&str>| s.and_then(|s| s.parse::<u64>().ok());

    match (cmd, num(a)) {
        (Some("btn"), Some(n @ 1..=4)) => {
            let until = Instant::now().as_micros() + num(b).unwrap_or(150) * 1000;
            BUTTONS.lock(|bt| bt.borrow_mut()[n as usize - 1] = until);
        }
        (Some("card"), Some(uid)) => CARD.signal(u128::from(uid)),
        (Some("bat"), Some(pct @ 0..=100)) => set_battery(state, pct as u8),
        (Some("screen"), _) => {
            OLED.lock(|o| o.borrow_mut().dirty = true);
            print_oled();
        }
        (Some("state"), _) => print_state(state),
        (Some("oled"), _) if a == Some("raw") => {
            OLED_RAW.store(true, portable_atomic::Ordering::Relaxed);
            OLED.lock(|o| o.borrow_mut().dirty = true);
        }
        (Some("wifi"), _) if a == Some("off") => WIFI.signal(true),
        (Some("wifi"), _) if a == Some("on") => WIFI.signal(false),
        _ => print(format_args!("? {line}\r\n{HELP}")),
    }
}

/// The battery gauge doesn't exist in the emulator; the first value also unblocks the boot screen.
fn set_battery(state: GlobalState, percent: u8) {
    state.battery.write(|b| b.percent = percent);
    state.battery_ready.signal(());
}
