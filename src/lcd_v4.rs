use adv_shift_registers::wrappers::ShifterValueRange;
use alloc::{rc::Rc, string::ToString};
use display_interface_i2c::I2CInterface;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_graphics::{
    Drawable,
    pixelcolor::BinaryColor,
    prelude::{DrawTarget, Point},
    text::{Alignment, Text},
};
use embedded_graphics_framebuf::FrameBuf;
use esp_hal::gpio::Output;
use oled_async::{displays::ssd1309::Ssd1309_128_64, mode::GraphicsMode};

use crate::{
    consts::{
        DEEPER_SLEEP_AFTER_MS, INSPECTION_TIME_PLUS2, LCD_INSPECTION_FRAME_TIME,
        SCROLL_TICKER_INVERVAL_MS, SLEEP_AFTER_MS,
    },
    state::{
        GlobalState, MenuScene, Scene, SignaledGlobalStateInner, deeper_sleep_state, sleep_state,
    },
    translations::{TranslationKey, get_translation, get_translation_params},
    utils::{
        lcd_abstract::{LcdAbstract, PrintAlign},
        shared_i2c::SharedI2C,
        stackmat::ms_to_time_str,
    },
};

#[embassy_executor::task]
pub async fn lcd_task(
    i2c: SharedI2C,
    mut display_rst: Output<'static>,
    //lcd_shifter: adv_shift_registers::wrappers::ShifterValue,
    global_state: GlobalState,
    wifi_setup_sig: Rc<Signal<NoopRawMutex, ()>>,
    //display: ShifterValueRange,
) {
    let di = display_interface_i2c::I2CInterface::new(i2c, 0x3C, 0x40);
    let raw_disp =
        oled_async::builder::Builder::new(oled_async::displays::ssd1309::Ssd1309_128_64 {})
            .with_rotation(oled_async::prelude::DisplayRotation::Rotate0)
            .connect(di);

    let mut disp: oled_async::mode::GraphicsMode<_, _> = raw_disp.into();
    disp.reset(&mut display_rst, &mut Delay);

    disp.init().await.unwrap();
    disp.clear();
    disp.flush().await.unwrap();

    let mut data =
        alloc::vec![embedded_graphics::pixelcolor::BinaryColor::Off; (128 * 64) as usize];
    let data = data.as_mut_array().unwrap();

    let mut fbuf = embedded_graphics_framebuf::FrameBuf::new(data, 128, 64);

    let mut lcd_driver: LcdAbstract<80, 16, 2, 3> = LcdAbstract::new();
    _ = lcd_driver.print(
        0,
        &alloc::format!("{:X}", crate::utils::get_efuse_u32()),
        PrintAlign::Left,
        true,
    );
    _ = lcd_driver.print(1, crate::version::VERSION, PrintAlign::Center, true);
    fbuf.clear(BinaryColor::Off);
    lcd_driver.display_on_oled(&mut fbuf).await;
    embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter()).unwrap();
    disp.flush().await.unwrap();

    _ = lcd_driver.print(
        0,
        &alloc::format!("{}%", global_state.show_battery.wait().await),
        PrintAlign::Right,
        false,
    );
    fbuf.clear(BinaryColor::Off);
    lcd_driver.display_on_oled(&mut fbuf).await;
    embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter()).unwrap();
    disp.flush().await.unwrap();

    Timer::after_millis(2500).await;

    fbuf.clear(BinaryColor::Off);
    _ = lcd_driver.clear_all();
    embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter()).unwrap();
    disp.flush().await.unwrap();

    let mut last_update;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::debug!("lcd current_state: {current_state:?}");
        last_update = Instant::now();

        if sleep_state() {
            //lcd.backlight_on();

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
                &wifi_setup_sig,
                &mut fbuf,
                &mut disp,
            )
            .await;
            fbuf.clear(BinaryColor::Off);
            lcd_driver.display_on_oled(&mut fbuf).await;
            embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter()).unwrap();
            disp.flush().await.unwrap();

            let mut scroll_ticker =
                embassy_time::Ticker::every(Duration::from_millis(SCROLL_TICKER_INVERVAL_MS));
            loop {
                scroll_ticker.next().await;
                let changed = lcd_driver.scroll_step();
                if changed.is_ok_and(|c| c) {
                    fbuf.clear(BinaryColor::Off);
                    lcd_driver.display_on_oled(&mut fbuf).await;
                    embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter())
                        .unwrap();
                    disp.flush().await.unwrap();
                }

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if !sleep_state()
                    && (Instant::now() - last_update).as_millis() > SLEEP_AFTER_MS
                    && current_scene.can_sleep()
                {
                    _ = lcd_driver.print(0, "Sleep", PrintAlign::Center, true);
                    _ = lcd_driver.print(1, "Press any key", PrintAlign::Center, true);
                    fbuf.clear(BinaryColor::Off);
                    lcd_driver.display_on_oled(&mut fbuf).await;
                    embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter())
                        .unwrap();
                    disp.flush().await.unwrap();
                    //lcd.backlight_off();

                    {
                        global_state.state.lock().await.server_connected = Some(false);
                    }

                    unsafe {
                        crate::state::SLEEP_STATE = true;
                        crate::state::TRUST_SERVER = false;
                    }

                    global_state.state.signal_reset();
                }

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if sleep_state()
                    && !deeper_sleep_state()
                    && (Instant::now() - last_update).as_millis() > DEEPER_SLEEP_AFTER_MS
                {
                    _ = lcd_driver.print(0, "Deep Sleep", PrintAlign::Center, true);
                    _ = lcd_driver.print(1, "Press any key", PrintAlign::Center, true);
                    fbuf.clear(BinaryColor::Off);
                    lcd_driver.display_on_oled(&mut fbuf).await;
                    embedded_graphics::prelude::DrawTarget::draw_iter(&mut disp, fbuf.into_iter())
                        .unwrap();
                    disp.flush().await.unwrap();
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

async fn process_lcd(
    current_state: SignaledGlobalStateInner,
    global_state: &GlobalState,
    lcd_driver: &mut LcdAbstract<80, 16, 2, 3>,
    //lcd: &mut LcdDisplay<T, D>,
    wifi_setup_sig: &Signal<NoopRawMutex, ()>,
    //display: &ShifterValueRange,
    fbuf: &mut FrameBuf<BinaryColor, &mut [BinaryColor; 128 * 64]>,
    disp: &mut GraphicsMode<Ssd1309_128_64, I2CInterface<SharedI2C>>,
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

    // display custom message on top of everything!
    if let Some((line1, line2)) = &current_state.custom_message {
        _ = lcd_driver.print(0, line1, PrintAlign::Center, true);
        _ = lcd_driver.print(1, line2, PrintAlign::Center, true);

        return Some(());
    }

    if let Some(sel) = current_state.selected_config_menu {
        lcd_driver.clear_all().ok()?;
        lcd_driver.print(0, "<", PrintAlign::Left, false).ok()?;
        lcd_driver.print(0, ">", PrintAlign::Right, false).ok()?;

        lcd_driver
            .print(0, "Config Menu", PrintAlign::Center, false)
            .ok()?;

        lcd_driver
            .print(
                1,
                &alloc::format!("{}. {}", sel + 1, crate::structs::CONFIG_MENU_ITEMS[sel]),
                PrintAlign::Left,
                true,
            )
            .ok()?;

        return Some(());
    }

    match current_state.menu_scene {
        Some(MenuScene::Signing) | Some(MenuScene::Unsigning) => {
            let prefix = if current_state.menu_scene == Some(MenuScene::Signing) {
                "S"
            } else {
                "Uns"
            };

            lcd_driver.clear_all().ok()?;
            lcd_driver
                .print(
                    0,
                    &alloc::format!("{prefix}igning | Submit To Exit"),
                    PrintAlign::Left,
                    true,
                )
                .ok()?;

            if global_state.sign_unsign_progress.signaled() {
                let status = if global_state.sign_unsign_progress.wait().await {
                    "OK"
                } else {
                    "FAIL"
                };

                lcd_driver
                    .print(
                        1,
                        &alloc::format!("Operation {}", status),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
                fbuf.clear(BinaryColor::Off);
                lcd_driver.display_on_oled(fbuf).await;
                embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
                disp.flush().await.unwrap();

                Timer::after_millis(300).await;
                lcd_driver
                    .print(1, "Scan the card", PrintAlign::Center, true)
                    .ok()?;
            } else {
                lcd_driver
                    .print(1, "Scan the card", PrintAlign::Center, true)
                    .ok()?;
            }

            return Some(());
        }
        Some(crate::state::MenuScene::BtDisplay) => {
            lcd_driver.clear_all().ok()?;
            if current_state.selected_bluetooth_item
                == current_state.discovered_bluetooth_devices.len()
            {
                lcd_driver
                    .print(0, "Unpair", PrintAlign::Center, true)
                    .ok()?;
            } else if current_state.selected_bluetooth_item
                == current_state.discovered_bluetooth_devices.len() + 1
            {
                lcd_driver.print(0, "Exit", PrintAlign::Center, true).ok()?;
            } else if current_state.selected_bluetooth_item
                < current_state.discovered_bluetooth_devices.len()
            {
                if let Some(display_dev) = current_state
                    .discovered_bluetooth_devices
                    .get(current_state.selected_bluetooth_item)
                {
                    lcd_driver
                        .print(0, &display_dev.name, PrintAlign::Center, true)
                        .ok()?;

                    lcd_driver
                        .print(
                            1,
                            &alloc::format!("{:x?}", display_dev.addr),
                            PrintAlign::Center,
                            true,
                        )
                        .ok()?;
                }
            } else {
                global_state.state.lock().await.selected_bluetooth_item = 0;
            }

            return Some(());
        }
        None => {}
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

            fbuf.clear(BinaryColor::Off);
            lcd_driver.display_on_oled(fbuf).await;
            embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
            disp.flush().await.unwrap();
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
            lcd_driver.clear_all().ok()?;
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

                fbuf.clear(BinaryColor::Off);
                lcd_driver.display_on_oled(fbuf).await;
                embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
                disp.flush().await.unwrap();
                Timer::after_millis(LCD_INSPECTION_FRAME_TIME).await;
            }
        }
        Scene::Timer => loop {
            let time = global_state.timer_signal.wait().await;
            let time_str = ms_to_time_str(time);
            lcd_driver
                .print(0, &time_str, PrintAlign::Center, true)
                .ok()?;

            //display.set_data_raw(&crate::utils::stackmat::time_str_to_display(&time_str));
            fbuf.clear(BinaryColor::Off);
            lcd_driver.display_on_oled(fbuf).await;
            embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
            disp.flush().await.unwrap();
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

                fbuf.clear(BinaryColor::Off);
                lcd_driver.display_on_oled(fbuf).await;
                embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
                disp.flush().await.unwrap();
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
    if !current_state.scene.can_be_lcd_overwritten() {
        return false;
    }

    if current_state.server_connected == Some(false) {
        if current_state.wifi_conn_lost {
            // TODO: maybe add to this translation
            _ = lcd_driver.print(0, "Wi-Fi", PrintAlign::Center, true);
            _ = lcd_driver.print(1, "Connection lost", PrintAlign::Center, true);
        } else {
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
        }
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
