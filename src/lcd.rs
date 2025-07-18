use adv_shift_registers::wrappers::ShifterValueRange;
use ag_lcd_async::LcdDisplay;
use alloc::{rc::Rc, string::ToString};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;

use crate::{
    consts::{
        DEEPER_SLEEP_AFTER_MS, INSPECTION_TIME_PLUS2, LCD_INSPECTION_FRAME_TIME,
        SCROLL_TICKER_INVERVAL_MS, SLEEP_AFTER_MS,
    },
    state::{GlobalState, Scene, SignaledGlobalStateInner, deeper_sleep_state, sleep_state},
    translations::{TranslationKey, get_translation, get_translation_params},
    utils::{
        lcd_abstract::{LcdAbstract, PrintAlign},
        stackmat::ms_to_time_str,
    },
};

#[embassy_executor::task]
pub async fn lcd_task(
    lcd_shifter: adv_shift_registers::wrappers::ShifterValue,
    global_state: GlobalState,
    wifi_setup_sig: Rc<Signal<NoopRawMutex, ()>>,
    display: ShifterValueRange,
) {
    let mut lcd = {
        let bl_pin = lcd_shifter.get_pin_mut(1, true);
        let rs_pin = lcd_shifter.get_pin_mut(2, true);
        let en_pin = lcd_shifter.get_pin_mut(3, true);
        let d4_pin = lcd_shifter.get_pin_mut(4, false);
        let d5_pin = lcd_shifter.get_pin_mut(5, false);
        let d6_pin = lcd_shifter.get_pin_mut(6, false);
        let d7_pin = lcd_shifter.get_pin_mut(7, false);
        LcdDisplay::new(rs_pin, en_pin, Delay)
            .with_display(ag_lcd_async::Display::On)
            .with_blink(ag_lcd_async::Blink::Off)
            .with_cursor(ag_lcd_async::Cursor::Off)
            .with_size(ag_lcd_async::Size::Dots5x8)
            .with_cols(16)
            .with_lines(ag_lcd_async::Lines::TwoLines)
            .with_half_bus(d4_pin, d5_pin, d6_pin, d7_pin)
            .with_backlight(bl_pin)
            .build()
            .await
    };

    lcd.clear().await;
    lcd.backlight_on();

    let mut lcd_driver: LcdAbstract<80, 16, 2, 3> = LcdAbstract::new();

    _ = lcd_driver.print(
        0,
        &alloc::format!("{:X}", crate::utils::get_efuse_u32()),
        PrintAlign::Left,
        true,
    );
    _ = lcd_driver.print(1, crate::version::VERSION, PrintAlign::Center, true);
    lcd_driver.display_on_lcd(&mut lcd).await;

    _ = lcd_driver.print(
        0,
        &alloc::format!("{}%", global_state.show_battery.wait().await),
        PrintAlign::Right,
        false,
    );
    lcd_driver.display_on_lcd(&mut lcd).await;

    #[cfg(not(feature = "bat_dev_lcd"))]
    Timer::after_millis(2500).await;

    _ = lcd_driver.clear_all();
    let mut last_update;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::debug!("lcd current_state: {current_state:?}");
        last_update = Instant::now();

        if sleep_state() {
            lcd.backlight_on();

            unsafe {
                crate::state::SLEEP_STATE = false;
            }
        }

        let current_scene = current_state.scene.clone();
        let fut = async {
            let _ = process_lcd(
                current_state,
                &global_state,
                &mut lcd_driver,
                &mut lcd,
                &wifi_setup_sig,
                &display,
            )
            .await;
            lcd_driver.display_on_lcd(&mut lcd).await;

            let mut scroll_ticker =
                embassy_time::Ticker::every(Duration::from_millis(SCROLL_TICKER_INVERVAL_MS));
            loop {
                scroll_ticker.next().await;
                let changed = lcd_driver.scroll_step();
                if changed.is_ok_and(|c| c) {
                    lcd_driver.display_on_lcd(&mut lcd).await;
                }

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if !sleep_state()
                    && (Instant::now() - last_update).as_millis() > SLEEP_AFTER_MS
                    && current_scene.can_sleep()
                {
                    _ = lcd_driver.print(0, "Sleep", PrintAlign::Center, true);
                    _ = lcd_driver.print(1, "Press any key", PrintAlign::Center, true);
                    lcd_driver.display_on_lcd(&mut lcd).await;
                    lcd.backlight_off();

                    unsafe {
                        crate::state::SLEEP_STATE = true;
                    }
                }

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if sleep_state()
                    && !deeper_sleep_state()
                    && (Instant::now() - last_update).as_millis() > DEEPER_SLEEP_AFTER_MS
                {
                    _ = lcd_driver.print(0, "Deep Sleep", PrintAlign::Center, true);
                    _ = lcd_driver.print(1, "Press any key", PrintAlign::Center, true);
                    lcd_driver.display_on_lcd(&mut lcd).await;
                    crate::utils::deeper_sleep();
                }
            }
        };

        let res = embassy_futures::select::select(fut, global_state.state.wait()).await;
        match res {
            embassy_futures::select::Either::First(_) => {}
            embassy_futures::select::Either::Second(_) => {
                continue;
            }
        }
    }
}

async fn process_lcd<T: OutputPin, D: DelayNs>(
    current_state: SignaledGlobalStateInner,
    global_state: &GlobalState,
    lcd_driver: &mut LcdAbstract<80, 16, 2, 3>,
    lcd: &mut LcdDisplay<T, D>,
    wifi_setup_sig: &Signal<NoopRawMutex, ()>,
    display: &ShifterValueRange,
) -> Option<()> {
    #[cfg(feature = "bat_dev_lcd")]
    {
        let battery_read = current_state.current_bat_read.unwrap_or(-1.0);
        lcd_driver
            .print(
                0,
                &alloc::format!("BAT: {battery_read}"),
                PrintAlign::Left,
                true,
            )
            .ok()?;

        if let Some(avg) = current_state.avg_bat_read {
            lcd_driver
                .print(1, &alloc::format!("AVG: {avg}"), PrintAlign::Left, true)
                .ok()?;
        }

        return Some(());
    }

    if let Some(error_text) = current_state.error_text {
        lcd_driver
            .print(
                0,
                &get_translation(TranslationKey::ERROR_HEADER),
                PrintAlign::Center,
                true,
            )
            .ok()?;

        lcd_driver
            .print(1, &error_text, PrintAlign::Center, true)
            .ok()?;

        return Some(());
    }

    let overwritten = process_lcd_overwrite(&current_state, global_state, lcd_driver).await;
    if overwritten {
        return Some(());
    }

    lcd_driver.clear_all().ok()?;
    if let Some(time) = current_state.delegate_hold {
        let delegate_remaining = 3 - time;

        if delegate_remaining == 0 {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WAITING_FOR_DELEGATE_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation(TranslationKey::WAITING_FOR_DELEGATE_FOOTER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        } else {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::CALLING_FOR_DELEGATE_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation_params(
                        TranslationKey::CALLING_FOR_DELEGATE_FOOTER,
                        &[delegate_remaining],
                    ),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        }

        return Some(());
    }

    match current_state.scene {
        Scene::WifiConnect => {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WAITING_FOR_WIFI_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation(TranslationKey::WAITING_FOR_WIFI_FOOTER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver.display_on_lcd(lcd).await;
            wifi_setup_sig.wait().await;
            global_state.state.lock().await.scene = Scene::AutoSetupWait;
        }
        Scene::AutoSetupWait => {
            let wifi_ssid = alloc::format!("FKM-{:X}", crate::utils::get_efuse_u32());
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WIFI_SETUP_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(1, &wifi_ssid, PrintAlign::Center, true)
                .ok()?;
        }
        Scene::MdnsWait => {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WAITING_FOR_MDNS_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation(TranslationKey::WAITING_FOR_MDNS_FOOTER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        }
        Scene::GroupSelect => {
            lcd_driver.print(0, "<", PrintAlign::Left, false).ok()?;
            lcd_driver.print(1, "<", PrintAlign::Left, false).ok()?;
            lcd_driver.print(0, ">", PrintAlign::Right, false).ok()?;
            lcd_driver.print(1, ">", PrintAlign::Right, false).ok()?;

            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::SELECT_GROUP),
                    PrintAlign::Center,
                    false,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &current_state.possible_groups[current_state.group_selected_idx].secondary_text,
                    PrintAlign::Center,
                    false,
                )
                .ok()?;
        }
        Scene::WaitingForCompetitor => {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::SCAN_COMPETITOR_CARD_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            if let Some(solve_time) = current_state.solve_time {
                let time_str = ms_to_time_str(solve_time);
                lcd_driver
                    .print(
                        1,
                        &get_translation_params(
                            TranslationKey::SCAN_COMPETITOR_CARD_WITH_TIME_FOOTER,
                            &[time_str],
                        ),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
            } else {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::SCAN_COMPETITOR_CARD_FOOTER),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
            }
        }
        Scene::CompetitorInfo => {
            lcd_driver
                .print(
                    0,
                    &current_state
                        .competitor_display
                        .unwrap_or("??????".to_string()),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            if let Some(group) = current_state.solve_group {
                lcd_driver
                    .print(1, &group.secondary_text, PrintAlign::Center, true)
                    .ok()?;
            }
        }
        Scene::Inspection => {
            let inspection_start = global_state
                .state
                .value()
                .await
                .inspection_start
                .unwrap_or(Instant::now());

            lcd_driver
                .print(1, "Inspection", PrintAlign::Center, true)
                .ok()?;

            loop {
                let elapsed = (Instant::now() - inspection_start).as_millis();
                let time_str = ms_to_time_str(elapsed);

                lcd_driver
                    .print(0, &time_str, PrintAlign::Center, true)
                    .ok()?;

                lcd_driver.display_on_lcd(lcd).await;
                Timer::after_millis(LCD_INSPECTION_FRAME_TIME).await;
            }
        }
        Scene::Timer => loop {
            let time = global_state.timer_signal.wait().await;
            let time_str = ms_to_time_str(time);
            lcd_driver
                .print(0, &time_str, PrintAlign::Center, true)
                .ok()?;

            display.set_data_raw(&crate::utils::stackmat::time_str_to_display(&time_str));
            lcd_driver.display_on_lcd(lcd).await;
        },
        Scene::Finished => {
            let solve_time = current_state.solve_time.unwrap_or(0);
            let time_str = if solve_time > 0 {
                ms_to_time_str(solve_time)
            } else {
                heapless::String::new()
            };

            let inspection_time =
                match (current_state.inspection_start, current_state.inspection_end) {
                    (Some(start), Some(end)) => {
                        Some(end.saturating_duration_since(start).as_millis())
                    }
                    _ => None,
                };

            if current_state.use_inspection()
                && inspection_time.unwrap_or(0) > INSPECTION_TIME_PLUS2
            {
                let inspections_seconds = inspection_time.unwrap_or(0) / 1000;
                lcd_driver
                    .print(
                        0,
                        &alloc::format!("{time_str} ({inspections_seconds}s)"),
                        PrintAlign::Left,
                        true,
                    )
                    .ok()?;
            } else {
                lcd_driver
                    .print(0, &time_str, PrintAlign::Left, true)
                    .ok()?;
            }

            let penalty = current_state.penalty.unwrap_or(0);
            let penalty_str = match penalty {
                -2 => "DNS",
                -1 => "DNF",
                1.. => &alloc::format!("+{penalty}"),
                _ => "",
            };

            lcd_driver
                .print(0, penalty_str, PrintAlign::Right, false)
                .ok()?;

            if !current_state.time_confirmed {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::CONFIRM_TIME),
                        PrintAlign::Right,
                        true,
                    )
                    .ok()?;
            } else if current_state.current_judge.is_none() {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::SCAN_JUDGE_CARD),
                        PrintAlign::Right,
                        true,
                    )
                    .ok()?;
            } else if current_state.current_competitor.is_some()
                && current_state.current_judge.is_some()
            {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::SCAN_COMPETITOR_CARD),
                        PrintAlign::Right,
                        true,
                    )
                    .ok()?;
            }
        }
        Scene::Update => {
            _ = lcd_driver.print(0, "Updating...", PrintAlign::Center, true);
            loop {
                let progress = global_state.update_progress.wait().await;
                _ = lcd_driver.print(1, &alloc::format!("{progress}%"), PrintAlign::Center, true);

                lcd_driver.display_on_lcd(lcd).await;
            }
        }
    }

    Some(())
}

async fn process_lcd_overwrite(
    current_state: &SignaledGlobalStateInner,
    _global_state: &GlobalState,
    lcd_driver: &mut LcdAbstract<80, 16, 2, 3>,
) -> bool {
    // display custom message on top of everything!
    if let Some((line1, line2)) = &current_state.custom_message {
        _ = lcd_driver.print(0, line1, PrintAlign::Center, true);
        _ = lcd_driver.print(1, line2, PrintAlign::Center, true);

        return true;
    }

    if !current_state.scene.can_be_lcd_overwritten() {
        return false;
    }

    if current_state.server_connected == Some(false) {
        _ = lcd_driver.print(
            0,
            &get_translation(TranslationKey::SERVER_DISCONNECTED_HEADER),
            PrintAlign::Center,
            true,
        );
        _ = lcd_driver.print(
            1,
            &get_translation(TranslationKey::SERVER_DISCONNECTED_FOOTER),
            PrintAlign::Center,
            true,
        );
    } else if current_state.device_added == Some(false) {
        #[cfg(not(feature = "e2e"))]
        let lines = (
            &get_translation(TranslationKey::DEVICE_NOT_ADDED_HEADER),
            &get_translation(TranslationKey::DEVICE_NOT_ADDED_FOOTER),
        );

        #[cfg(feature = "e2e")]
        let lines = ("Press submit", "To start HIL");

        _ = lcd_driver.print(0, lines.0, PrintAlign::Center, true);
        _ = lcd_driver.print(1, lines.1, PrintAlign::Center, true);
    } else if current_state.stackmat_connected == Some(false) {
        _ = lcd_driver.print(
            0,
            &get_translation(TranslationKey::STACKMAT_DISCONNECTED_HEADER),
            PrintAlign::Center,
            true,
        );
        _ = lcd_driver.print(
            1,
            &get_translation(TranslationKey::STACKMAT_DISCONNECTED_FOOTER),
            PrintAlign::Center,
            true,
        );
    } else {
        return false;
    }

    true
}
