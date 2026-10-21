//! esp-emu simulation build (feature `sim`, see sim/README.md).
//!
//! The firmware runs its real drivers: the sim/patches bus tap (so `sim` builds need the
//! patched esp-hal, see sim/run.sh) hands GPIO, I2C, SPI and ADC traffic to device models on
//! the host (sim/bus.py). This module only adds what the host can't see: a machine-readable
//! state line, heap statistics and a Wi-Fi radio switch, read from the tap's console input.

#[cfg(feature = "timer-func")]
compile_error!("feature `sim` needs the UART stackmat (--no-default-features)");

use crate::state::GlobalState;
use core::fmt::Write;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Instant, Timer};

/// `wifi off|on`: `true` stops the radio (driver torn down), `false` restarts it
pub static WIFI: Signal<CriticalSectionRawMutex, bool> = Signal::new();

/// v3: what lcd_v3 last wrote (its change cache), so the host can compare it with its LCD.
#[cfg(feature = "v3")]
static LCD_TEXT: embassy_sync::blocking_mutex::CriticalSectionMutex<
    core::cell::Cell<[[u8; crate::utils::lcd_text::COLS]; 2]>,
> = embassy_sync::blocking_mutex::CriticalSectionMutex::new(core::cell::Cell::new(
    [[b' '; crate::utils::lcd_text::COLS]; 2],
));

#[cfg(feature = "v3")]
pub fn lcd_intended(rows: &[[u8; crate::utils::lcd_text::COLS]; 2]) {
    LCD_TEXT.lock(|t| t.set(*rows));
}

const HELP: &str = "firmware sim commands: state | heap | wifi off|on";

/// One machine-readable line for sim/monkey.py: `@state key=value ...` (`-` = none).
fn print_state(state: GlobalState) {
    let mut line = heapless::String::<640>::new();
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
        " timer={:?} up={} id={:X}",
        state.timer.try_get(),
        Instant::now().as_millis(),
        crate::utils::get_efuse_u32()
    );
    #[cfg(feature = "v3")]
    {
        _ = write!(line, " lcdtext=");
        for b in LCD_TEXT.lock(|t| t.get()).as_flattened() {
            _ = write!(line, "{b:02x}");
        }
    }
    esp_println::println!("{line}");
}

/// Reads console commands (polled from the tap).
#[embassy_executor::task]
pub async fn command_task(state: GlobalState) {
    let mut line = heapless::Vec::<u8, 64>::new();
    let mut buf = [0u8; 64];
    loop {
        Timer::after_millis(10).await;
        let n = esp_hal::sim_tap::console(&mut buf);
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
}

fn command(line: &str, state: GlobalState) {
    match line.trim() {
        "state" => print_state(state),
        "heap" => {
            let stats = esp_alloc::HEAP.stats();
            esp_println::println!(
                "Heap: used {} max {} of {} bytes",
                stats.current_usage,
                stats.max_usage,
                stats.size
            );
        }
        "wifi off" => WIFI.signal(true),
        "wifi on" => WIFI.signal(false),
        _ => esp_println::println!("? {line}\r\n{HELP}"),
    }
}
