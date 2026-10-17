use crate::{
    power,
    state::{CHANGED, ErrorLogEntryStage, GlobalState, MenuScene, Scene, Ui, View},
    translations::{TranslationKey, get_translation, get_translation_params},
    utils::{
        error_log::{self, ErrorLogEntry, codes},
        oled_ui::{self as ui, Frame, Line64, QrData, TopBar},
        screen::{Event, Live, Local, SIGN_RESULT_MS, Screen, next_event, screen},
        shared_i2c::SharedI2C,
        ssd1309::Ssd1309,
    },
};
use core::fmt::Write;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::gpio::Output;

pub use ui::DETAILS_VISIBLE_LINES;

type Text = heapless::String<256>;

const EVENT_TIMEOUT: Duration = Duration::from_secs(30);
/// Idle redraw-free tick: the sleep check.
const IDLE_TICK: Duration = Duration::from_secs(1);

async fn flush(oled: &mut Ssd1309<SharedI2C>) {
    match with_timeout(Duration::from_millis(1500), oled.flush()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::error!("OLED flush error: {e:?}"),
        Err(_) => {
            oled.frame.invalidate();
            log::error!("OLED FLUSH TIMEOUT");
            error_log::add_error(codes::LCD_FLUSH_TIMEOUT);
        }
    }
}

async fn init(i2c: SharedI2C, mut display_rst: Output<'static>) -> Ssd1309<SharedI2C> {
    let mut oled = Ssd1309::new(i2c);
    if let Err(e) = oled.init(&mut display_rst).await {
        log::error!("Disp init error: {e:?} (but continuing i guess)");
        error_log::add_error(codes::LCD_INIT_FAILED);
    }
    flush(&mut oled).await; // clear the panel's power-up content
    oled
}

pub async fn lcd_show_critical_code(i2c: SharedI2C, display_rst: Output<'static>, code: u8) {
    let mut oled = init(i2c, display_rst).await;
    let mut text = Text::new();
    _ = write!(text, "CRITICAL ERROR\nCODE: {code}");
    ui::centered_main(&mut oled.frame, &text);
    flush(&mut oled).await;
}

#[embassy_executor::task]
pub async fn lcd_task(
    i2c: SharedI2C,
    display_rst: Output<'static>,
    global_state: GlobalState,
    wifi_setup_sig: &'static Signal<CriticalSectionRawMutex, ()>,
) {
    let mut oled = init(i2c, display_rst).await;

    global_state.battery_ready.wait().await;
    global_state.view(|v| top_bar(&mut oled.frame, v));
    let mut text = Text::new();
    _ = write!(
        text,
        "S/N: {:X}\nVER: {}",
        crate::utils::get_efuse_u32(),
        crate::version::VERSION
    );
    ui::centered_main(&mut oled.frame, &text);
    flush(&mut oled).await;
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
                power::wake();
                log::warn!("Sleep wakeup!");
            }
        }

        if rerender {
            crate::bench!(LCD_FRAME);
            rerender = false;
            global_state.ui.write(clamp_selection);
            live = global_state.view(|v| render(v, &mut oled.frame, &local));
            if live != Live::Timer {
                local.timer_ms = None;
            }
            flush(&mut oled).await;
        }

        crate::watchdog::heartbeat(crate::watchdog::TaskId::Lcd);
        // bounded so the heartbeat keeps beating while a live screen waits for its signal
        let event = with_timeout(
            EVENT_TIMEOUT,
            next_event(
                global_state,
                &mut timer,
                wifi_setup_sig,
                live,
                &local,
                IDLE_TICK,
            ),
        );
        let Ok(event) = event.await else {
            continue;
        };
        match event {
            Event::State => state_changed = true,
            Event::TimerValue(ms) => {
                local.timer_ms = Some(ms);
                ui::digits(&mut oled.frame, ms);
                flush(&mut oled).await;
            }
            Event::InspectionFrame(start) => {
                ui::digits(&mut oled.frame, start.elapsed().as_millis());
                flush(&mut oled).await;
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
                global_state
                    .solve
                    .update(|s| s.scene = Scene::AutoSetupWait);
            }
            Event::Tick => sleep_check(&mut oled, global_state, last_update).await,
        }
    }
}

/// Sleep after `SLEEP_AFTER_MS` without state changes, deep sleep after `DEEPER_SLEEP_AFTER_MS`.
async fn sleep_check(
    oled: &mut Ssd1309<SharedI2C>,
    global_state: GlobalState,
    last_update: Instant,
) {
    use crate::consts::{DEEPER_SLEEP_AFTER_MS, SLEEP_AFTER_MS, SLEEP_ENABLED};
    if !SLEEP_ENABLED {
        return;
    }

    let idle_ms = last_update.elapsed().as_millis();
    let can_sleep = global_state.solve.read(|s| s.scene.can_sleep());
    if !power::is_sleeping() && idle_ms > SLEEP_AFTER_MS && can_sleep {
        enter_sleep(oled, global_state).await;
    }
    if power::is_sleeping()
        && !crate::power::is_deeper_sleeping()
        && idle_ms > DEEPER_SLEEP_AFTER_MS
    {
        oled.frame.clear();
        ui::centered_screen(&mut oled.frame, "Sleep", ui::SMALL_FONT);
        flush(oled).await;
        power::enter_deep_sleep();
    }
}

/// Sleep screen + connection flags.
async fn enter_sleep(oled: &mut Ssd1309<SharedI2C>, global_state: GlobalState) {
    oled.frame.clear();
    ui::centered_screen(&mut oled.frame, "Sleep", ui::SMALL_FONT);
    flush(oled).await;

    global_state.status.update(|s| {
        s.server_connected = Some(false);
        s.wifi_connected = Some(false);
    });

    power::request_sleep();
    crate::state::SESSION.write(|s| s.trust_server = false);
    CHANGED.reset(); // these changes must not wake the display again
}

/// Out-of-range menu selections reset to the first item (was done mid-render before).
fn clamp_selection(state: &mut Ui) {
    let error_log_items = error_log::entry_count() + 1;
    if state.menu_scene == Some(MenuScene::ErrorLog)
        && state.selected_error_log_entry.is_none()
        && state.selected_error_log_item >= error_log_items
    {
        state.selected_error_log_item = 0;
    }
    let bt_items = state.discovered_bluetooth_devices.len() + 2;
    if state.menu_scene == Some(MenuScene::BtDisplay) && state.selected_bluetooth_item >= bt_items {
        state.selected_bluetooth_item = 0;
    }
}

fn top_bar(f: &mut Frame, s: &View) {
    let title = if s.ui.selected_config_menu.is_some() {
        Some("CONFIG")
    } else if let Some(ref menu_scene) = s.ui.menu_scene {
        Some(match menu_scene {
            MenuScene::Signing => "SIGN",
            MenuScene::Unsigning => "UNSIGN",
            MenuScene::BtDisplay => "BTDISP",
            MenuScene::ErrorLog => "ERRLOG",
            MenuScene::BuzzerVolume => "BUZZER",
        })
    } else if let Some(ref group) = s.solve.solve_group
        && s.solve.scene == Scene::CompetitorInfo
    {
        Some(group.name.as_str())
    } else {
        None
    };

    ui::top_bar(
        f,
        &TopBar {
            battery: (s.battery.percent, s.battery.charging),
            wifi: s.status.wifi_connected.unwrap_or(false),
            server: s.status.server_connected.unwrap_or(false),
            stackmat: s.status.stackmat_connected.unwrap_or(false),
            title,
        },
    );
}

/// `a\nb` into a stack buffer.
fn two_lines(a: &str, b: &str) -> Text {
    let mut t = Text::new();
    _ = write!(t, "{a}\n{b}");
    t
}

fn tr_lines(header: usize, footer: usize) -> Text {
    two_lines(&get_translation(header), &get_translation(footer))
}

/// Draws the whole frame for the current state (no I/O, no heap).
fn render(s: &View, f: &mut Frame, local: &Local) -> Live {
    f.clear();
    top_bar(f, s);

    let mut text = Text::new();
    match screen(s.solve, s.ui, &s.status) {
        Screen::Error(error) => ui::text_main(
            f,
            &two_lines(&get_translation(TranslationKey::ERROR_HEADER), error),
        ),
        Screen::Custom(l1, l2) => ui::text_main(f, &two_lines(l1, l2)),
        Screen::ConfigMenu(sel) => {
            let items = &crate::structs::CONFIG_MENU_ITEMS;
            ui::menu(f, items.len(), sel, |i, buf| {
                _ = write!(buf, "{}. {}", i + 1, items[i]);
            });
        }
        Screen::BuzzerVolume => {
            _ = write!(
                text,
                "Buzzer Volume\n< {} >\nSubmit to save",
                crate::state::SETTINGS.read(|s| s.buzzer_volume)
            );
            ui::text_main(f, &text);
        }
        Screen::ErrorLogEntry { idx, stage, scroll } => match error_log::entry(idx) {
            Some(entry) if stage == Some(&ErrorLogEntryStage::Qr) => draw_qr(f, &entry),
            Some(entry) => ui::details(f, &error_log_entry_details_text(&entry), scroll),
            None => ui::text_main(f, "Invalid entry\nSubmit to return"),
        },
        Screen::ErrorLogList(sel) => {
            let count = error_log::entry_count();
            ui::menu(f, count + 1, sel, |i, buf| match error_log::entry(i) {
                Some(entry) if i < count => _ = buf.push_str(&entry.list_label()),
                _ if i < count => _ = buf.push('?'),
                _ => _ = buf.push_str("Exit"),
            });
        }
        Screen::Signing(sign) => {
            let title = if sign { "Signing" } else { "Unsigning" };
            match local.sign_result {
                Some((ok, at)) if at.elapsed().as_millis() < SIGN_RESULT_MS => {
                    let status = if ok { "OK" } else { "FAIL" };
                    _ = write!(text, "{title}\nOperation: {status}");
                }
                _ => _ = write!(text, "{title}\nScan the card\n\nSubmit to exit"),
            }
            ui::text_main(f, &text);
            return Live::Signing;
        }
        Screen::BtDevices(sel) => {
            let devices = &s.ui.discovered_bluetooth_devices;
            ui::menu(f, devices.len() + 2, sel, |i, buf| match devices.get(i) {
                Some(dev) => _ = write!(buf, "{} [{:X?}]", dev.name, dev.addr),
                None if i == devices.len() => _ = buf.push_str("Unpair"),
                None => _ = buf.push_str("Exit"),
            });
        }
        Screen::WifiLost => ui::text_main(f, "Wi-Fi\nConnection lost"),
        Screen::ServerLost => ui::text_main(
            f,
            &tr_lines(
                TranslationKey::SERVER_DISCONNECTED_HEADER,
                TranslationKey::SERVER_DISCONNECTED_FOOTER,
            ),
        ),
        Screen::DeviceNotAdded => {
            let lines = tr_lines(
                TranslationKey::DEVICE_NOT_ADDED_HEADER,
                TranslationKey::DEVICE_NOT_ADDED_FOOTER,
            );
            ui::text_main(f, &lines);
        }
        Screen::StackmatLost => ui::text_main(
            f,
            &tr_lines(
                TranslationKey::STACKMAT_DISCONNECTED_HEADER,
                TranslationKey::STACKMAT_DISCONNECTED_FOOTER,
            ),
        ),
        Screen::Delegate(0) => ui::text_main(
            f,
            &tr_lines(
                TranslationKey::WAITING_FOR_DELEGATE_HEADER,
                TranslationKey::WAITING_FOR_DELEGATE_FOOTER,
            ),
        ),
        Screen::Delegate(remaining) => ui::text_main(
            f,
            &two_lines(
                &get_translation(TranslationKey::CALLING_FOR_DELEGATE_HEADER),
                &get_translation_params(TranslationKey::CALLING_FOR_DELEGATE_FOOTER, &[remaining]),
            ),
        ),
        Screen::WifiConnect => {
            ui::text_main(
                f,
                &tr_lines(
                    TranslationKey::WAITING_FOR_WIFI_HEADER,
                    TranslationKey::WAITING_FOR_WIFI_FOOTER,
                ),
            );
            return Live::WifiConnect;
        }
        Screen::AutoSetup => {
            _ = write!(
                text,
                "{}\nFKM-{:X}",
                get_translation(TranslationKey::WIFI_SETUP_HEADER),
                crate::utils::get_efuse_u32()
            );
            ui::text_main(f, &text);
        }
        Screen::MdnsWait => ui::text_main(
            f,
            &tr_lines(
                TranslationKey::WAITING_FOR_MDNS_HEADER,
                TranslationKey::WAITING_FOR_MDNS_FOOTER,
            ),
        ),
        Screen::GroupSelect(name) => ui::group_select(
            f,
            &two_lines(&get_translation(TranslationKey::SELECT_GROUP), name),
        ),
        Screen::WaitingForCompetitor(solve_time) => {
            let header = get_translation(TranslationKey::SCAN_COMPETITOR_CARD_HEADER);
            let footer = match solve_time {
                Some(time) => get_translation_params(
                    TranslationKey::SCAN_COMPETITOR_CARD_WITH_TIME_FOOTER,
                    &[crate::utils::text::time_str(time)],
                ),
                None => get_translation(TranslationKey::SCAN_COMPETITOR_CARD_FOOTER),
            };
            ui::text_main(f, &two_lines(&header, &footer));
        }
        Screen::CompetitorInfo => {
            _ = text.push_str(s.solve.competitor_display.as_deref().unwrap_or("------"));
            if let Some(secondary) = s
                .solve
                .solve_group
                .as_ref()
                .and_then(|g| g.secondary_text.as_ref())
            {
                _ = write!(text, "\n{secondary}");
            }
            ui::text_main(f, &text);
        }
        Screen::Inspection(start) => {
            ui::inspection_label(f, &get_translation(TranslationKey::INSPECTION));
            ui::digits(f, start.elapsed().as_millis());
            return Live::Inspection(start);
        }
        Screen::Timer => {
            // keep the last value on re-render so unrelated state changes don't blank the time
            if let Some(ms) = local.timer_ms {
                ui::digits(f, ms);
            }
            return Live::Timer;
        }
        Screen::Finished(fin) => {
            let time = crate::utils::text::time_str(fin.time);
            let mut main = Line64::new();
            _ = match fin.penalty {
                -2 if fin.time == 0 => write!(main, "DNS"),
                -2 => write!(main, "DNS ({time})"),
                -1 if fin.time == 0 => write!(main, "DNF"),
                -1 => write!(main, "DNF ({time})"),
                p @ 1.. => write!(main, "{time} +{p}"),
                _ => write!(main, "{time}"),
            };

            let inspection = fin.inspection.map(|ms| {
                let mut s = Line64::new();
                _ = write!(
                    s,
                    "{}: {}",
                    get_translation(TranslationKey::INSPECTION),
                    crate::utils::text::time_str(ms)
                );
                s
            });
            let status = fin.status.map(get_translation);
            ui::finished(f, &main, inspection.as_deref(), status.as_deref());
        }
        Screen::Update => {
            _ = write!(text, "Updating\n{}%", local.progress.unwrap_or(0));
            ui::text_main(f, &text);
            return Live::Update;
        }
    }

    Live::Idle
}

fn draw_qr(f: &mut Frame, entry: &ErrorLogEntry) {
    match entry {
        ErrorLogEntry::Code { code, .. } => {
            let mut url = Line64::new();
            _ = write!(url, "https://docs.fkmtime.com/debugging/e{code}");
            ui::qr(f, QrData::Text(&url));
        }
        ErrorLogEntry::Stacktrace { version, addrs, .. } => {
            let mut data: heapless::Vec<u8, 96> = heapless::Vec::new();
            _ = data.push(version.len() as u8);
            _ = data.extend_from_slice(version.as_bytes());
            for addr in addrs {
                _ = data.extend_from_slice(&addr.to_be_bytes());
            }
            ui::qr(f, QrData::Binary(&data));
        }
    }
}

/// Lines of the error log details view (also used by the buttons to limit scrolling).
pub fn error_log_entry_details_text(entry: &ErrorLogEntry) -> Text {
    let mut t = Text::new();
    _ = match entry {
        ErrorLogEntry::Code { timestamp, code } => write!(
            t,
            "Error E{code}\n{}\nSubmit to return",
            error_log::format_timestamp_full(*timestamp)
        ),
        ErrorLogEntry::Stacktrace {
            timestamp,
            version,
            addrs,
        } => (|| {
            write!(
                t,
                "Panic {}\nVER: {}\n",
                error_log::format_timestamp_compact(*timestamp),
                version
            )?;
            if addrs.is_empty() {
                write!(t, "No addrs")?;
            }
            for (idx, addr) in addrs.iter().enumerate() {
                let sep = if idx > 0 { "\n" } else { "" };
                write!(t, "{sep}0x{addr:X}")?;
            }
            write!(t, "\nSubmit to return")
        })(),
    };
    t
}
