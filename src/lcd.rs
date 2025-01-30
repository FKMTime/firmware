use alloc::{rc::Rc, string::ToString};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Instant, Timer};
use hd44780_driver::{
    charset::{CharsetA02, CharsetWithFallback},
    memory_map::{MemoryMap1602, StandardMemoryMap},
    DisplayMode, HD44780,
};

use crate::{
    consts::{INSPECTION_TIME_PLUS2, LCD_INSPECTION_FRAME_TIME, SCROLL_TICKER_INVERVAL_MS},
    state::{sleep_state, GlobalState, Scene, SignaledGlobalStateInner},
    translations::{get_translation, get_translation_params},
    utils::{
        lcd_abstract::{LcdAbstract, PrintAlign},
        stackmat::ms_to_time_str,
    },
};

#[cfg(feature = "esp32c3")]
use embedded_hal::digital::OutputPin;

#[embassy_executor::task]
pub async fn lcd_task(
    #[cfg(feature = "esp32")] i2c: esp_hal::i2c::master::I2c<'static, esp_hal::Blocking>,

    #[cfg(feature = "esp32c3")] lcd_shifter: adv_shift_registers::wrappers::ShifterValue,

    global_state: GlobalState,
    wifi_setup_sig: Rc<Signal<NoopRawMutex, ()>>,
) {
    #[cfg(feature = "esp32c3")]
    let bl_pin = lcd_shifter.get_pin_mut(1, true);

    let rs_pin = lcd_shifter.get_pin_mut(2, true);
    let en_pin = lcd_shifter.get_pin_mut(3, true);
    let d4_pin = lcd_shifter.get_pin_mut(7, false);
    let d5_pin = lcd_shifter.get_pin_mut(6, false);
    let d6_pin = lcd_shifter.get_pin_mut(5, false);
    let d7_pin = lcd_shifter.get_pin_mut(4, false);
    let mut lcd: ag_lcd::LcdDisplay<_, _> = ag_lcd::LcdDisplay::new(rs_pin, en_pin, Delay)
        .with_half_bus(d4_pin, d5_pin, d6_pin, d7_pin)
        .with_display(ag_lcd::Display::On)
        .with_blink(ag_lcd::Blink::Off)
        .with_cursor(ag_lcd::Cursor::Off)
        .with_size(ag_lcd::Size::Dots5x8)
        .with_backlight(bl_pin)
        .with_cols(16)
        .with_lines(ag_lcd::Lines::TwoLines)
        .build();

    #[cfg(feature = "esp32")]
    let mut options = {
        hd44780_driver::setup::DisplayOptionsI2C::new(MemoryMap1602::new())
            .with_i2c_bus(i2c, 0x27)
            .with_charset(CharsetA02::QUESTION_FALLBACK)
    };

    lcd.clear();
    //lcd.backlight_on();

    lcd.set_character(
        2,
        [
            0b10010, 0b10000, 0b10010, 0b01000, 0b01111, 0b01000, 0b00001, 0b00011,
        ],
    );
    lcd.set_character(
        3,
        [
            0b11011, 0b11011, 0b11011, 0b11011, 0b11011, 0b00000, 0b00000, 0b00000,
        ],
    );
    lcd.set_character(
        1,
        [
            0b10101, 0b01010, 0b10101, 0b01010, 0b00100, 0b00111, 0b00111, 0b11100,
        ],
    );
    lcd.set_character(
        0,
        [
            0b00001, 0b00010, 0b00100, 0b01100, 0b10010, 0b11001, 0b00010, 0b00100,
        ],
    );

    lcd.set_position(0, 0);
    lcd.write(0);

    lcd.set_position(0, 1);
    lcd.write(1);

    lcd.set_position(1, 0);
    lcd.write(2);

    lcd.set_position(1, 1);
    lcd.write(3);

    Timer::after_millis(1000).await;
    lcd.clear();
    lcd.set_character(
        0,
        [
            0b00000, 0b00000, 0b01100, 0b01100, 0b01001, 0b00011, 0b00011, 0b00111,
        ],
    );
    lcd.set_character(
        1,
        [
            0b00000, 0b00000, 0b00110, 0b00110, 0b10010, 0b11000, 0b11000, 0b11100,
        ],
    );
    lcd.set_character(
        2,
        [
            0b00111, 0b00011, 0b00011, 0b01001, 0b01100, 0b01100, 0b00000, 0b00000,
        ],
    );
    lcd.set_character(
        3,
        [
            0b11100, 0b11000, 0b11000, 0b10010, 0b00110, 0b00110, 0b00000, 0b00000,
        ],
    );

    lcd.set_position(0, 0);
    lcd.write(0);

    lcd.set_position(0, 1);
    lcd.write(2);

    lcd.set_position(1, 0);
    lcd.write(1);

    lcd.set_position(1, 1);
    lcd.write(3);

    /*
    let mut lcd_driver: LcdAbstract<80, 16, 2, 3> = LcdAbstract::new();

    _ = lcd_driver.print(
        0,
        &alloc::format!("ID: {:X}", crate::utils::get_efuse_u32()),
        PrintAlign::Left,
        true,
    );
    _ = lcd_driver.print(
        1,
        &alloc::format!("VER: {}", crate::version::VERSION),
        PrintAlign::Left,
        true,
    );
    _ = lcd_driver.display_on_lcd(&mut lcd, &mut delay);

    _ = lcd_driver.print(
        0,
        &alloc::format!("{}%", global_state.show_battery.wait().await),
        PrintAlign::Right,
        false,
    );
    _ = lcd_driver.display_on_lcd(&mut lcd, &mut delay);

    _ = lcd.clear(&mut delay);
    _ = lcd.define_custom_character(
        0,
        &hd44780_driver::character::CharacterDefinition {
            pattern: [
                0b00001, 0b00010, 0b00000, 0b00100, 0b00100, 0b00100, 0b11111, 0b11111, 0, 0,
            ],
            cursor: 8,
        },
        &mut delay,
    );
    _ = lcd.set_cursor_xy((0, 0), &mut delay);
    _ = lcd.write_byte(0, &mut delay);

    _ = lcd.define_custom_character(
        1,
        &hd44780_driver::character::CharacterDefinition {
            pattern: [
                0b11111, 0b00001, 0b01001, 0b01001, 0b01001, 0b01000, 0b01110, 0b00000, 0, 0,
            ],
            cursor: 8,
        },
        &mut delay,
    );
    _ = lcd.set_cursor_xy((0, 1), &mut delay);
    _ = lcd.write_byte(1, &mut delay);

    _ = lcd.define_custom_character(
        2,
        &hd44780_driver::character::CharacterDefinition {
            pattern: [
                0b10010, 0b10000, 0b10010, 0b01000, 0b01111, 0b01000, 0b00001, 0b00011, 0, 0,
            ],
            cursor: 8,
        },
        &mut delay,
    );
    _ = lcd.set_cursor_xy((1, 1), &mut delay);
    _ = lcd.write_byte(2, &mut delay);

    #[cfg(not(feature = "bat_dev_lcd"))]
    //Timer::after_millis(2500).await;
    Timer::after_millis(25000).await;

    _ = lcd_driver.clear_all();
    let mut last_update;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::debug!("current_state: {:?}", current_state);
        last_update = Instant::now();

        let fut = async {
            let _ = process_lcd(
                current_state,
                &global_state,
                &mut lcd_driver,
                &mut lcd,
                &mut delay,
                &wifi_setup_sig,
            )
            .await;
            lcd_driver.display_on_lcd(&mut lcd, &mut delay).unwrap();

            let mut scroll_ticker =
                embassy_time::Ticker::every(Duration::from_millis(SCROLL_TICKER_INVERVAL_MS));
            loop {
                scroll_ticker.next().await;
                let changed = lcd_driver.scroll_step().unwrap();
                if changed {
                    lcd_driver.display_on_lcd(&mut lcd, &mut delay).unwrap();
                }

                if !sleep_state() && (Instant::now() - last_update).as_secs() > 60 * 5 {
                    #[cfg(feature = "esp32c3")]
                    {
                        _ = bl_pin.set_low();
                    }
                    #[cfg(feature = "esp32")]
                    {
                        _ = lcd.set_backlight(false, &mut delay);
                    }

                    unsafe {
                        crate::state::SLEEP_STATE = true;
                    }
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
    */
}

#[cfg(feature = "esp32")]
type LcdType<C> = HD44780<
    hd44780_driver::bus::I2CBus<esp_hal::i2c::master::I2c<'static, esp_hal::Blocking>>,
    StandardMemoryMap<16, 2>,
    C,
>;

#[cfg(feature = "esp32c3")]
type LcdType<C> = HD44780<
    hd44780_driver::bus::FourBitBus<
        adv_shift_registers::wrappers::ShifterPin,
        adv_shift_registers::wrappers::ShifterPin,
        adv_shift_registers::wrappers::ShifterPin,
        adv_shift_registers::wrappers::ShifterPin,
        adv_shift_registers::wrappers::ShifterPin,
        adv_shift_registers::wrappers::ShifterPin,
    >,
    StandardMemoryMap<16, 2>,
    C,
>;

async fn process_lcd<C: CharsetWithFallback>(
    current_state: SignaledGlobalStateInner,
    global_state: &GlobalState,
    lcd_driver: &mut LcdAbstract<80, 16, 2, 3>,
    lcd: &mut LcdType<C>,
    delay: &mut Delay,
    wifi_setup_sig: &Signal<NoopRawMutex, ()>,
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
                &get_translation("ERROR_HEADER"),
                PrintAlign::Center,
                true,
            )
            .ok()?;

        lcd_driver
            .print(1, &error_text, PrintAlign::Left, true)
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
                    &get_translation("DELEGATE_CALLED_1"),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation("DELEGATE_CALLED_2"),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        } else {
            lcd_driver
                .print(
                    0,
                    &get_translation("DELEGATE_WAIT_HEADER"),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation_params("DELEGATE_WAIT_TIME", &[delegate_remaining]),
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
                .print(0, &get_translation("WIFI_WAIT_1"), PrintAlign::Center, true)
                .ok()?;

            lcd_driver
                .print(1, &get_translation("WIFI_WAIT_2"), PrintAlign::Center, true)
                .ok()?;

            wifi_setup_sig.wait().await;
            global_state.state.lock().await.scene = Scene::AutoSetupWait;
        }
        Scene::AutoSetupWait => {
            let wifi_ssid = alloc::format!("FKM-{:X}", crate::utils::get_efuse_u32());
            lcd_driver
                .print(
                    0,
                    &get_translation("WIFI_SETUP_HEADER"),
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
                .print(0, &get_translation("MDNS_WAIT_1"), PrintAlign::Center, true)
                .ok()?;

            lcd_driver
                .print(1, &get_translation("MDNS_WAIT_2"), PrintAlign::Center, true)
                .ok()?;
        }
        Scene::WaitingForCompetitor => {
            lcd_driver
                .print(
                    0,
                    &get_translation("SCAN_COMPETITOR_1"),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            if let Some(solve_time) = current_state.solve_time {
                let time_str = ms_to_time_str(solve_time);
                lcd_driver
                    .print(
                        1,
                        &get_translation_params("SCAN_COMPETITOR_3", &[time_str]),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
            } else {
                lcd_driver
                    .print(
                        1,
                        &get_translation("SCAN_COMPETITOR_2"),
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
                        .unwrap_or("Unknown competitor?".to_string()),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            if let Some(secondary_text) = current_state.secondary_text {
                lcd_driver
                    .print(1, &secondary_text, PrintAlign::Center, true)
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

            loop {
                let elapsed = (Instant::now() - inspection_start).as_millis();
                let time_str = ms_to_time_str(elapsed);

                lcd_driver
                    .print(0, &time_str, PrintAlign::Center, true)
                    .ok()?;

                lcd_driver.display_on_lcd(lcd, delay).ok()?;
                Timer::after_millis(LCD_INSPECTION_FRAME_TIME).await;
            }
        }
        Scene::Timer => loop {
            let time = global_state.timer_signal.wait().await;
            let time_str = ms_to_time_str(time);
            lcd_driver
                .print(0, &time_str, PrintAlign::Center, true)
                .ok()?;

            lcd_driver.display_on_lcd(lcd, delay).ok()?;
        },
        Scene::Finished => {
            let solve_time = current_state.solve_time.unwrap_or(0);
            let time_str = if solve_time > 0 {
                ms_to_time_str(solve_time)
            } else {
                heapless::String::new()
            };

            let inspection_time = current_state
                .inspection_start
                .map(|x| (current_state.inspection_end.unwrap() - x).as_millis());

            if current_state.use_inspection && inspection_time.unwrap_or(0) > INSPECTION_TIME_PLUS2
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
                    .print(1, &get_translation("CONFIRM_TIME"), PrintAlign::Right, true)
                    .ok()?;
            } else if current_state.current_judge.is_none() {
                lcd_driver
                    .print(
                        1,
                        &get_translation("SCAN_JUDGE_CARD"),
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
                        &get_translation("SCAN_COMPETITOR_CARD"),
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

                lcd_driver.display_on_lcd(lcd, delay).ok()?;
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
        _ = lcd_driver.print(0, "Server", PrintAlign::Center, true);
        _ = lcd_driver.print(
            1,
            &get_translation("DISCONNECTED_FOOTER"),
            PrintAlign::Center,
            true,
        );
    } else if current_state.device_added == Some(false) {
        _ = lcd_driver.print(
            0,
            &get_translation("DEV_NOT_ADDED_HEADER"),
            PrintAlign::Center,
            true,
        );
        _ = lcd_driver.print(
            1,
            &get_translation("DEV_NOT_ADDED_FOOTER"),
            PrintAlign::Center,
            true,
        );
    } else if current_state.stackmat_connected == Some(false) {
        _ = lcd_driver.print(0, "Stackmat", PrintAlign::Center, true);
        _ = lcd_driver.print(
            1,
            &get_translation("DISCONNECTED_FOOTER"),
            PrintAlign::Center,
            true,
        );
    } else {
        return false;
    }

    true
}
