use crate::{
    consts::{LCD_INSPECTION_FRAME_TIME, SCROLL_TICKER_INVERVAL_MS},
    power,
    state::{CHANGED, GlobalState, MenuScene, Scene, View},
    translations::{TranslationKey, get_translation, get_translation_params},
    utils::{
        error_log::{self, ErrorLogEntry},
        lcd_text::{Align, LcdText},
        screen::{Screen, screen},
        text::time_str,
    },
};
use adv_shift_registers::wrappers::{ShifterPin, ShifterValue};
use ag_lcd_async::LcdDisplay;
use core::fmt::Write;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Instant, Timer, with_timeout};

type Lcd = LcdDisplay<ShifterPin, Delay>;
type Text = heapless::String<96>;

const SIGN_RESULT_MS: u64 = 300;
const EVENT_TIMEOUT: Duration = Duration::from_secs(30);

async fn new_lcd(lcd_shifter: ShifterValue) -> Lcd {
    let mut lcd = LcdDisplay::new(
        lcd_shifter.get_pin_mut(2, true),
        lcd_shifter.get_pin_mut(3, true),
        Delay,
    )
    .with_display(ag_lcd_async::Display::On)
    .with_blink(ag_lcd_async::Blink::Off)
    .with_cursor(ag_lcd_async::Cursor::Off)
    .with_size(ag_lcd_async::Size::Dots5x8)
    .with_cols(16)
    .with_lines(ag_lcd_async::Lines::TwoLines)
    .with_half_bus(
        lcd_shifter.get_pin_mut(4, false),
        lcd_shifter.get_pin_mut(5, false),
        lcd_shifter.get_pin_mut(6, false),
        lcd_shifter.get_pin_mut(7, false),
    )
    .with_backlight(lcd_shifter.get_pin_mut(1, true))
    .build()
    .await;

    lcd.clear().await;
    lcd.backlight_on();
    lcd
}

/// Writes the changed characters to the LCD.
async fn show(text: &mut LcdText, lcd: &mut Lcd) {
    let mut runs: heapless::Vec<(u8, u8, heapless::String<16>), 2> = heapless::Vec::new();
    text.changes(|row, col, s| _ = runs.push((row, col, s.try_into().unwrap_or_default())));
    for (row, col, s) in runs {
        lcd.set_position(col, row).await;
        lcd.print(&s).await;
    }
}

pub async fn lcd_show_critical_code(lcd_shifter: ShifterValue, code: u8) {
    let mut lcd = new_lcd(lcd_shifter).await;
    let mut line = Text::new();
    _ = write!(line, "       E{code}");

    lcd.set_position(0, 0).await;
    lcd.print(" CRITICAL ERROR ").await;
    lcd.set_position(0, 1).await;
    lcd.print(&line).await;
}

#[derive(Clone, Copy, PartialEq)]
enum Live {
    /// nothing live: the scroll ticker (and sleep check) runs
    Idle,
    Signing,
    WifiConnect,
    Inspection(Instant),
    Timer,
    Update,
}

#[derive(Default)]
struct Local {
    timer_ms: Option<u64>,
    progress: Option<u8>,
    sign_result: Option<(bool, Instant)>,
}

#[embassy_executor::task]
pub async fn lcd_task(
    lcd_shifter: ShifterValue,
    global_state: GlobalState,
    wifi_setup_sig: &'static Signal<CriticalSectionRawMutex, ()>,
) {
    let mut lcd = new_lcd(lcd_shifter).await;
    let mut text = LcdText::new();

    let mut line = Text::new();
    _ = write!(line, "{:X}", crate::utils::get_efuse_u32());
    text.print(0, &line, Align::Left, true);
    text.print(1, crate::version::VERSION, Align::Center, true);
    show(&mut text, &mut lcd).await;

    line.clear();
    global_state.battery_ready.wait().await;
    _ = write!(line, "{}%", global_state.battery.read(|b| b.percent));
    text.print(0, &line, Align::Right, false);
    show(&mut text, &mut lcd).await;

    #[cfg(not(feature = "bat_dev_lcd"))]
    Timer::after_millis(2500).await;

    let Some(mut timer) = global_state.timer.receiver() else {
        log::error!("lcd: no timer receiver left");
        return;
    };
    let mut local = Local::default();
    let mut live = Live::Idle;
    let mut last_update = Instant::now();
    let (mut state_changed, mut rerender) = (true, true);
    loop {
        if state_changed {
            state_changed = false;
            rerender = true;
            last_update = Instant::now();
            if power::is_sleeping() {
                lcd.backlight_on();
                power::wake();
            }
        }

        if rerender {
            rerender = false;
            global_state.ui.write(|u| {
                if u.menu_scene == Some(MenuScene::BtDisplay)
                    && u.selected_bluetooth_item > u.discovered_bluetooth_devices.len() + 1
                {
                    u.selected_bluetooth_item = 0;
                }
            });
            live = global_state.view(|v| render(v, &mut text, &local));
            if live != Live::Timer {
                local.timer_ms = None;
            }
            show(&mut text, &mut lcd).await;
        }

        crate::watchdog::heartbeat(crate::watchdog::TaskId::Lcd);
        // bounded so the heartbeat keeps beating while a live screen waits for its signal
        let event = with_timeout(EVENT_TIMEOUT, next_event(global_state, &mut timer, wifi_setup_sig, live, &local));
        let Ok(event) = event.await else {
            continue;
        };
        match event {
            Event::State => state_changed = true,
            Event::TimerValue(ms) => {
                local.timer_ms = Some(ms);
                text.print(0, &time_str(ms), Align::Center, true);
                show(&mut text, &mut lcd).await;
            }
            Event::InspectionFrame(start) => {
                let elapsed = start.elapsed().as_millis();
                text.print(0, &time_str(elapsed), Align::Center, true);
                show(&mut text, &mut lcd).await;
            }
            Event::Progress(p) => {
                local.progress = Some(p);
                rerender = true;
            }
            Event::SignResult(ok) => {
                local.sign_result = Some((ok, Instant::now()));
                rerender = true;
            }
            Event::SignResultEnd => {
                local.sign_result = None;
                rerender = true;
            }
            Event::WifiSetup => {
                global_state.solve.update(|s| s.scene = Scene::AutoSetupWait);
            }
            Event::ScrollTick => {
                if text.scroll_step() {
                    show(&mut text, &mut lcd).await;
                }
                sleep_check(&mut text, &mut lcd, global_state, last_update).await;
            }
        }
    }
}

enum Event {
    State,
    TimerValue(u64),
    InspectionFrame(Instant),
    Progress(u8),
    SignResult(bool),
    SignResultEnd,
    WifiSetup,
    ScrollTick,
}

async fn next_event(
    gs: GlobalState,
    timer: &mut embassy_sync::watch::Receiver<'_, crate::state::Cs, u64, 2>,
    wifi_setup_sig: &Signal<CriticalSectionRawMutex, ()>,
    live: Live,
    local: &Local,
) -> Event {
    let other = async {
        match live {
            Live::Timer => Event::TimerValue(timer.changed().await),
            Live::Inspection(start) => {
                Timer::after_millis(LCD_INSPECTION_FRAME_TIME).await;
                Event::InspectionFrame(start)
            }
            Live::Update => Event::Progress(gs.update_progress.wait().await),
            Live::WifiConnect => {
                wifi_setup_sig.wait().await;
                Event::WifiSetup
            }
            Live::Signing => {
                let result_end = async {
                    match local.sign_result {
                        Some((_, at)) => Timer::at(at + Duration::from_millis(SIGN_RESULT_MS)).await,
                        None => core::future::pending().await,
                    }
                };
                match select3(gs.sign_unsign_progress.wait(), result_end, scroll_tick()).await {
                    Either3::First(ok) => Event::SignResult(ok),
                    Either3::Second(()) => Event::SignResultEnd,
                    Either3::Third(()) => Event::ScrollTick,
                }
            }
            Live::Idle => {
                scroll_tick().await;
                Event::ScrollTick
            }
        }
    };

    match select(CHANGED.wait(), other).await {
        Either::First(()) => Event::State,
        Either::Second(event) => event,
    }
}

async fn scroll_tick() {
    Timer::after_millis(SCROLL_TICKER_INVERVAL_MS).await;
}

/// Sleep after `SLEEP_AFTER_MS` without state changes, "deeper sleep" after
/// `DEEPER_SLEEP_AFTER_MS` (logic unchanged; WP5's power module takes it over later).
#[cfg(not(any(feature = "e2e", feature = "qa")))]
async fn sleep_check(text: &mut LcdText, lcd: &mut Lcd, global_state: GlobalState, last_update: Instant) {
    use crate::consts::{DEEPER_SLEEP_AFTER_MS, SLEEP_AFTER_MS};

    let idle_ms = last_update.elapsed().as_millis();
    let can_sleep = global_state.solve.read(|s| s.scene.can_sleep());
    if !power::is_sleeping() && idle_ms > SLEEP_AFTER_MS && can_sleep {
        text.print(0, "Sleep", Align::Center, true);
        text.print(1, "Press any key", Align::Center, true);
        show(text, lcd).await;
        lcd.backlight_off();

        global_state.status.update(|s| s.server_connected = Some(false));
        power::request_sleep();
        crate::state::SESSION.write(|s| s.trust_server = false);
        CHANGED.reset(); // these changes must not wake the display again
    }

    if power::is_sleeping() && !crate::power::is_deeper_sleeping() && idle_ms > DEEPER_SLEEP_AFTER_MS {
        text.print(0, "Deep Sleep", Align::Center, true);
        text.print(1, "Press any key", Align::Center, true);
        show(text, lcd).await;
        power::enter_deep_sleep();
    }
}

#[cfg(any(feature = "e2e", feature = "qa"))]
async fn sleep_check(_: &mut LcdText, _: &mut Lcd, _: GlobalState, _: Instant) {}

fn tr(text: &mut LcdText, header: usize, footer: usize) {
    text.print(0, &get_translation(header), Align::Center, true);
    text.print(1, &get_translation(footer), Align::Center, true);
}

/// Lays out both lines for the current state (no I/O, no heap).
fn render(s: &View, text: &mut LcdText, local: &Local) -> Live {
    text.clear();
    let mut line = Text::new();

    #[cfg(feature = "bat_dev_lcd")]
    {
        _ = write!(line, "BAT: {}", s.battery.current_read.unwrap_or(-1.0));
        text.print(0, &line, Align::Left, true);
        if let Some(avg) = s.battery.avg_read {
            line.clear();
            _ = write!(line, "AVG: {avg}");
            text.print(1, &line, Align::Left, true);
        }
        return Live::Idle;
    }

    match screen(s.solve, s.ui, &s.status) {
        Screen::Error(error) => {
            text.print(0, &get_translation(TranslationKey::ERROR_HEADER), Align::Center, true);
            text.print(1, error, Align::Center, true);
        }
        Screen::Custom(l1, l2) => {
            text.print(0, l1, Align::Center, true);
            text.print(1, l2, Align::Center, true);
        }
        Screen::ConfigMenu(sel) => {
            text.print(0, "<", Align::Left, false);
            text.print(0, ">", Align::Right, false);
            text.print(0, "Config Menu", Align::Center, false);
            let item = crate::structs::CONFIG_MENU_ITEMS.get(sel).copied().unwrap_or("");
            _ = write!(line, "{}. {item}", sel + 1);
            text.print(1, &line, Align::Left, true);
        }
        Screen::Signing(sign) => {
            let title = if sign { "Signing" } else { "Unsigning" };
            _ = write!(line, "{title} | Submit To Exit");
            text.print(0, &line, Align::Left, true);
            match local.sign_result {
                Some((ok, at)) if at.elapsed().as_millis() < SIGN_RESULT_MS => {
                    let status = if ok { "Operation OK" } else { "Operation FAIL" };
                    text.print(1, status, Align::Center, true);
                }
                _ => text.print(1, "Scan the card", Align::Center, true),
            }
            return Live::Signing;
        }
        Screen::BtDevices(sel) => {
            let devices = &s.ui.discovered_bluetooth_devices;
            match devices.get(sel) {
                Some(dev) => {
                    text.print(0, &dev.name, Align::Center, true);
                    _ = write!(line, "{:x?}", dev.addr);
                    text.print(1, &line, Align::Center, true);
                }
                None if sel == devices.len() => text.print(0, "Unpair", Align::Center, true),
                None => text.print(0, "Exit", Align::Center, true),
            }
        }
        Screen::ErrorLogEntry { idx, .. } => match error_log::entry(idx) {
            Some(ErrorLogEntry::Code { timestamp, code }) => {
                _ = write!(line, "Error E{code}");
                text.print(0, &line, Align::Left, true);
                text.print(1, &error_log::format_timestamp_full(timestamp), Align::Left, true);
            }
            Some(ErrorLogEntry::Stacktrace { timestamp, .. }) => {
                text.print(0, "Panic (view logs for details)", Align::Left, true);
                text.print(1, &error_log::format_timestamp_full(timestamp), Align::Left, true);
            }
            None => {
                text.print(0, "Invalid log entry", Align::Center, true);
                text.print(1, "Submit to return", Align::Center, true);
            }
        },
        Screen::ErrorLogList(sel) => {
            if sel == error_log::entry_count() {
                text.print(0, "Exit", Align::Center, true);
                text.print(1, "Submit to return", Align::Center, true);
            } else if let Some(entry) = error_log::entry(sel) {
                text.print(0, &entry.list_label(), Align::Center, true);
                text.print(1, "Submit for details", Align::Center, true);
            } else {
                text.print(0, "No entries", Align::Center, true);
                text.print(1, "Exit", Align::Center, true);
            }
        }
        Screen::WifiLost => {
            text.print(0, "Wi-Fi", Align::Center, true);
            text.print(1, "Connection lost", Align::Center, true);
        }
        Screen::ServerLost => tr(
            text,
            TranslationKey::SERVER_DISCONNECTED_HEADER,
            TranslationKey::SERVER_DISCONNECTED_FOOTER,
        ),
        Screen::DeviceNotAdded => {
            #[cfg(not(feature = "e2e"))]
            tr(
                text,
                TranslationKey::DEVICE_NOT_ADDED_HEADER,
                TranslationKey::DEVICE_NOT_ADDED_FOOTER,
            );
            #[cfg(feature = "e2e")]
            {
                text.print(0, "Press submit", Align::Center, true);
                text.print(1, "To start HIL", Align::Center, true);
            }
        }
        Screen::StackmatLost => tr(
            text,
            TranslationKey::STACKMAT_DISCONNECTED_HEADER,
            TranslationKey::STACKMAT_DISCONNECTED_FOOTER,
        ),
        Screen::Delegate(0) => tr(
            text,
            TranslationKey::WAITING_FOR_DELEGATE_HEADER,
            TranslationKey::WAITING_FOR_DELEGATE_FOOTER,
        ),
        Screen::Delegate(remaining) => {
            let header = get_translation(TranslationKey::CALLING_FOR_DELEGATE_HEADER);
            text.print(0, &header, Align::Center, true);
            let footer =
                get_translation_params(TranslationKey::CALLING_FOR_DELEGATE_FOOTER, &[remaining]);
            text.print(1, &footer, Align::Center, true);
        }
        Screen::WifiConnect => {
            tr(
                text,
                TranslationKey::WAITING_FOR_WIFI_HEADER,
                TranslationKey::WAITING_FOR_WIFI_FOOTER,
            );
            return Live::WifiConnect;
        }
        Screen::AutoSetup => {
            text.print(0, &get_translation(TranslationKey::WIFI_SETUP_HEADER), Align::Center, true);
            _ = write!(line, "FKM-{:X}", crate::utils::get_efuse_u32());
            text.print(1, &line, Align::Center, true);
        }
        Screen::MdnsWait => tr(
            text,
            TranslationKey::WAITING_FOR_MDNS_HEADER,
            TranslationKey::WAITING_FOR_MDNS_FOOTER,
        ),
        Screen::GroupSelect(name) => {
            for row in 0..2 {
                text.print(row, "<", Align::Left, false);
                text.print(row, ">", Align::Right, false);
            }
            text.print(0, &get_translation(TranslationKey::SELECT_GROUP), Align::Center, false);
            text.print(1, name, Align::Center, false);
        }
        Screen::WaitingForCompetitor(solve_time) => {
            let header = get_translation(TranslationKey::SCAN_COMPETITOR_CARD_HEADER);
            text.print(0, &header, Align::Center, true);
            let footer = match solve_time {
                Some(time) => get_translation_params(
                    TranslationKey::SCAN_COMPETITOR_CARD_WITH_TIME_FOOTER,
                    &[time_str(time)],
                ),
                None => get_translation(TranslationKey::SCAN_COMPETITOR_CARD_FOOTER),
            };
            text.print(1, &footer, Align::Center, true);
        }
        Screen::CompetitorInfo => {
            let name = s.solve.competitor_display.as_deref().unwrap_or("??????");
            text.print(0, name, Align::Center, true);
            if let Some(ref group) = s.solve.solve_group {
                text.print(1, &group.name, Align::Center, true);
            }
        }
        Screen::Inspection(start) => {
            text.print(1, "Inspection", Align::Center, true);
            text.print(0, &time_str(start.elapsed().as_millis()), Align::Center, true);
            return Live::Inspection(start);
        }
        Screen::Timer => {
            // keep the last value on re-render so unrelated state changes don't blank the time
            if let Some(ms) = local.timer_ms {
                text.print(0, &time_str(ms), Align::Center, true);
            }
            return Live::Timer;
        }
        Screen::Finished(fin) => {
            let time = if fin.time > 0 { time_str(fin.time) } else { heapless::String::new() };
            _ = match fin.inspection {
                Some(ms) => write!(line, "{time} ({}s)", ms / 1000),
                None => write!(line, "{time}"),
            };
            text.print(0, &line, Align::Left, true);

            line.clear();
            _ = match fin.penalty {
                -2 => write!(line, "DNS"),
                -1 => write!(line, "DNF"),
                p @ 1.. => write!(line, "+{p}"),
                _ => Ok(()),
            };
            text.print(0, &line, Align::Right, false);

            if let Some(status) = fin.status {
                text.print(1, &get_translation(status), Align::Right, true);
            }
        }
        Screen::Update => {
            text.print(0, "Updating...", Align::Center, true);
            if let Some(p) = local.progress {
                _ = write!(line, "{p}%");
                text.print(1, &line, Align::Center, true);
            }
            return Live::Update;
        }
    }

    Live::Idle
}
