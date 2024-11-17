use adv_shift_registers::wrappers::{ShifterPin, ShifterValue};
use alloc::string::ToString;
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_hal::digital::OutputPin;
use hd44780_driver::{
    bus::{FourBitBus, FourBitBusPins},
    charset::{CharsetA02, CharsetWithFallback},
    memory_map::{MemoryMap1602, StandardMemoryMap},
    setup::DisplayOptions4Bit,
    DisplayMode, HD44780,
};

use crate::{
    lcd_abstract::{LcdAbstract, PrintAlign},
    state::{sleep_state, GlobalState, Scene, SignaledGlobalStateInner},
};

#[embassy_executor::task]
pub async fn lcd_task(lcd_shifter: ShifterValue, global_state: GlobalState) {
    let mut bl_pin = lcd_shifter.get_pin_mut(1, true);
    let reg_sel_pin = lcd_shifter.get_pin_mut(2, true);
    let e_pin = lcd_shifter.get_pin_mut(3, true);
    let d4_pin = lcd_shifter.get_pin_mut(7, false);
    let d5_pin = lcd_shifter.get_pin_mut(6, false);
    let d6_pin = lcd_shifter.get_pin_mut(5, false);
    let d7_pin = lcd_shifter.get_pin_mut(4, false);

    let mut options = DisplayOptions4Bit::new(MemoryMap1602::new())
        .with_pins(FourBitBusPins {
            rs: reg_sel_pin,
            en: e_pin,
            d4: d4_pin,
            d5: d5_pin,
            d6: d6_pin,
            d7: d7_pin,
        })
        .with_charset(CharsetA02::QUESTION_FALLBACK);

    let mut delay = Delay;

    let mut lcd = loop {
        match HD44780::new(options, &mut delay) {
            Err((opt, e)) => {
                log::error!("Error creating CLD driver: {e:?}");
                options = opt;
                Timer::after_millis(100).await;
            }
            Ok(lcd) => break lcd,
        }
    };
    _ = bl_pin.set_high();

    _ = lcd.clear(&mut delay);
    _ = lcd.reset(&mut delay);
    _ = lcd.set_display_mode(
        DisplayMode {
            display: hd44780_driver::Display::On,
            cursor_visibility: hd44780_driver::Cursor::Invisible,
            cursor_blink: hd44780_driver::CursorBlink::Off,
        },
        &mut delay,
    );
    _ = lcd.clear(&mut delay);
    let mut lcd_driver: LcdAbstract<80, 16, 2, 3> = LcdAbstract::new();

    _ = lcd_driver.print(
        0,
        &alloc::format!("ID: {:X}", esp_hal_wifimanager::get_efuse_u32()),
        PrintAlign::Left,
        true,
    );
    _ = lcd_driver.print(0, &alloc::format!("{}%", 69), PrintAlign::Right, false);
    _ = lcd_driver.print(
        1,
        &alloc::format!("VER: {}", "v3.0"),
        PrintAlign::Left,
        true,
    );

    _ = lcd_driver.display_on_lcd(&mut lcd, &mut delay);
    Timer::after_millis(2500).await;

    // TODO: print to lcd if wifi setup active
    _ = lcd_driver.clear_all();

    let mut last_update;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::warn!("current_state: {:?}", current_state);
        last_update = Instant::now();
        if !sleep_state() {
            _ = bl_pin.set_high();
            unsafe {
                crate::state::SLEEP_STATE = true;
            }
        }

        let fut = async {
            let _ = process_lcd(
                current_state,
                &global_state,
                &mut lcd_driver,
                &mut lcd,
                &mut delay,
            )
            .await;
            lcd_driver.display_on_lcd(&mut lcd, &mut delay).unwrap();

            let mut scroll_ticker = embassy_time::Ticker::every(Duration::from_millis(500));
            loop {
                scroll_ticker.next().await;
                let changed = lcd_driver.scroll_step().unwrap();
                if changed {
                    lcd_driver.display_on_lcd(&mut lcd, &mut delay).unwrap();
                }

                if sleep_state() && (Instant::now() - last_update).as_secs() > 60 * 5 {
                    _ = bl_pin.set_low();
                    unsafe {
                        crate::state::SLEEP_STATE = false;
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
}

type LcdType<C> = HD44780<
    FourBitBus<ShifterPin, ShifterPin, ShifterPin, ShifterPin, ShifterPin, ShifterPin>,
    StandardMemoryMap<16, 2>,
    C,
>;

async fn process_lcd<C: CharsetWithFallback>(
    current_state: SignaledGlobalStateInner,
    global_state: &GlobalState,
    lcd_driver: &mut LcdAbstract<80, 16, 2, 3>,
    lcd: &mut LcdType<C>,
    delay: &mut Delay,
) -> Option<()> {
    if let Some(error_text) = current_state.error_text {
        lcd_driver
            .print(0, "Error", PrintAlign::Center, true)
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
                .print(0, "Waiting for", PrintAlign::Center, true)
                .ok()?;

            lcd_driver
                .print(1, "delegate", PrintAlign::Center, true)
                .ok()?;
        } else {
            lcd_driver
                .print(0, "Delegate", PrintAlign::Center, true)
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &alloc::format!("In: {delegate_remaining}"),
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
                .print(0, "Waiting for", PrintAlign::Center, true)
                .ok()?;

            lcd_driver
                .print(1, "WIFI connection", PrintAlign::Center, true)
                .ok()?;
        }
        Scene::AutoSetupWait => todo!(),
        Scene::MdnsWait => {
            lcd_driver
                .print(0, "Waiting for", PrintAlign::Center, true)
                .ok()?;

            lcd_driver.print(1, "MDNS", PrintAlign::Center, true).ok()?;
        }
        Scene::WaitingForCompetitor => {
            lcd_driver
                .print(0, "Scan the card", PrintAlign::Center, true)
                .ok()?;

            if let Some(solve_time) = current_state.solve_time {
                let time_str = crate::utils::ms_to_time_str(solve_time);
                lcd_driver
                    .print(
                        1,
                        &alloc::format!("of a competitor ({time_str})"),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
            } else {
                lcd_driver
                    .print(1, "of a competitor", PrintAlign::Center, true)
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
                let time_str = crate::utils::ms_to_time_str(elapsed);

                lcd_driver
                    .print(0, &time_str, PrintAlign::Center, true)
                    .ok()?;

                lcd_driver.display_on_lcd(lcd, delay).ok()?;
                Timer::after_millis(1000 / 30).await;
            }
        }
        Scene::Timer => loop {
            let time = global_state.timer_signal.wait().await;
            let time_str = crate::utils::ms_to_time_str(time);
            lcd_driver
                .print(0, &time_str, PrintAlign::Center, true)
                .ok()?;

            lcd_driver.display_on_lcd(lcd, delay).ok()?;
        },
        Scene::Finished => {
            let solve_time = current_state.solve_time.unwrap_or(0);
            let time_str = if solve_time > 0 {
                crate::utils::ms_to_time_str(solve_time)
            } else {
                heapless::String::new()
            };

            let inspection_time = current_state
                .inspection_start
                .and_then(|x| Some((current_state.inspection_end.unwrap() - x).as_millis()));

            if current_state.use_inspection && inspection_time.unwrap_or(0) > 15000 {
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
                .print(0, &penalty_str, PrintAlign::Right, false)
                .ok()?;

            if !current_state.time_confirmed {
                lcd_driver
                    .print(1, "Confirm the time", PrintAlign::Right, true)
                    .ok()?;
            } else if current_state.current_judge.is_none() {
                lcd_driver
                    .print(1, "Scan the judge's card", PrintAlign::Right, true)
                    .ok()?;
            } else if current_state.current_competitor.is_some()
                && current_state.current_judge.is_some()
            {
                lcd_driver
                    .print(1, "Scan the competitor's card", PrintAlign::Right, true)
                    .ok()?;
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
        _ = lcd_driver.print(1, "Disconnected", PrintAlign::Center, true);
    } else if current_state.device_added == Some(false) {
        _ = lcd_driver.print(0, "Device not added", PrintAlign::Center, true);
        _ = lcd_driver.print(1, "Press submit to connect", PrintAlign::Center, true);
    } else if current_state.stackmat_connected == Some(false) {
        _ = lcd_driver.print(0, "Stackmat", PrintAlign::Center, true);
        _ = lcd_driver.print(1, "Disconnected", PrintAlign::Center, true);
    } else {
        return false;
    }

    return true;
}
