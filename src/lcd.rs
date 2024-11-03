use adv_shift_registers::wrappers::{ShifterPin, ShifterValue};
use embassy_time::{Delay, Timer};
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use hd44780_driver::{
    bus::{FourBitBus, FourBitBusPins},
    charset::{CharsetA02, CharsetWithFallback},
    memory_map::{DisplayMemoryMap, MemoryMap1602, StandardMemoryMap},
    non_blocking::{bus::DataBus, HD44780},
    setup::DisplayOptions4Bit,
    DisplayMode,
};

use crate::scenes::{GlobalState, Scene, SignaledGlobalStateInner};

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
        match HD44780::new(options, &mut delay).await {
            Err((opt, e)) => {
                log::error!("Error creating CLD driver: {e:?}");
                options = opt;
                Timer::after_millis(100).await;
            }
            Ok(lcd) => break lcd,
        }
    };
    _ = bl_pin.set_high();

    _ = lcd.clear(&mut delay).await;
    _ = lcd.reset(&mut delay).await;
    _ = lcd
        .set_display_mode(
            DisplayMode {
                display: hd44780_driver::Display::On,
                cursor_visibility: hd44780_driver::Cursor::Invisible,
                cursor_blink: hd44780_driver::CursorBlink::Off,
            },
            &mut delay,
        )
        .await;
    _ = lcd.clear(&mut delay).await;

    _ = lcd
        .print(
            &alloc::format!("ID: {:X}", 694202137),
            0,
            PrintAlign::Left,
            true,
            &mut delay,
        )
        .await;
    _ = lcd
        .print(
            &alloc::format!("{}%", 69),
            0,
            PrintAlign::Right,
            false,
            &mut delay,
        )
        .await;
    _ = lcd
        .print(
            &alloc::format!("VER: {}", "v3.0"),
            1,
            PrintAlign::Left,
            true,
            &mut delay,
        )
        .await;

    Timer::after_millis(2500).await;

    // TODO: print to lcd if wifi setup active
    _ = lcd.clear(&mut delay).await;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::warn!("current_state: {:?}", current_state);
        let res = process_lcd(current_state, &global_state, &mut lcd, &mut delay).await;
        if res.is_none() {
            continue;
        }

        global_state.state.wait().await;
    }

    /*
    _ = lcd.clear(&mut delay).await;

    loop {
        let time_ms = time_sig.wait().await;
        //_ = lcd.set_cursor_xy((5, 1), &mut delay).await;

        if let Some(time_ms) = time_ms {
            let minutes: u8 = (time_ms / 60000) as u8;
            let seconds: u8 = ((time_ms % 60000) / 1000) as u8;
            let ms: u16 = (time_ms % 1000) as u16;

            let mut time_str = heapless::String::<8>::new();
            if minutes > 0 {
                _ = time_str.push((minutes + b'0') as char);
                _ = time_str.push(':');
                _ = time_str.push_str(&alloc::format!("{seconds:02}.{ms:03}"));
            } else {
                _ = time_str.push_str(&alloc::format!("{seconds:01}.{ms:03}"));
            }

            _ = lcd.print(&time_str, 1, PrintAlign::Center, true, &mut delay).await;
        } else {
            _ = lcd.print("", 1, PrintAlign::Center, true, &mut delay).await;
        }
        /*
        let (digits, n) = num_to_digits(time_ms as u128);
        for digit in &digits[..n] {
            if *digit == 0xFF {
                break;
            }

            _ = lcd.write_char((digit + 0x30) as char, &mut delay).await;
        }
        */
    }
    */
}

type LcdType<C> = HD44780<
    FourBitBus<ShifterPin, ShifterPin, ShifterPin, ShifterPin, ShifterPin, ShifterPin>,
    StandardMemoryMap<16, 2>,
    C,
>;

async fn process_lcd<C: CharsetWithFallback>(
    current_state: SignaledGlobalStateInner,
    global_state: &GlobalState,
    lcd: &mut LcdType<C>,
    delay: &mut Delay,
) -> Option<()> {
    let overwritten = process_lcd_overwrite(&current_state, global_state, lcd, delay).await;
    if overwritten {
        return Some(());
    }

    match current_state.scene {
        Scene::WifiConnect => {
            _ = lcd
                .print("Waiting for", 0, PrintAlign::Center, true, delay)
                .await;
            _ = lcd
                .print("WIFI connection", 1, PrintAlign::Center, true, delay)
                .await;
        }
        Scene::AutoSetupWait => todo!(),
        Scene::MdnsWait => {
            _ = lcd
                .print("Waiting for", 0, PrintAlign::Center, true, delay)
                .await;
            _ = lcd.print("MDNS", 1, PrintAlign::Center, true, delay).await;
        }
        Scene::WaitingForCompetitor { .. } => {
            _ = lcd
                .print("Waiting for", 0, PrintAlign::Center, true, delay)
                .await;
            _ = lcd
                .print("Competitor", 1, PrintAlign::Center, true, delay)
                .await;
        }
        Scene::CompetitorInfo() => todo!(),
        Scene::Inspection { .. } => todo!(),
        Scene::Timer { .. } => {
            _ = lcd.print("", 0, PrintAlign::Left, true, delay).await;
            _ = lcd.print("", 1, PrintAlign::Left, true, delay).await;

            loop {
                let time = global_state
                    .sig_or_update(&global_state.timer_signal)
                    .await?;
                let time_ms = time.unwrap_or(0);
                let minutes: u8 = (time_ms / 60000) as u8;
                let seconds: u8 = ((time_ms % 60000) / 1000) as u8;
                let ms: u16 = (time_ms % 1000) as u16;

                let mut time_str = heapless::String::<8>::new();
                if minutes > 0 {
                    _ = time_str.push((minutes + b'0') as char);
                    _ = time_str.push(':');
                    _ = time_str.push_str(&alloc::format!("{seconds:02}.{ms:03}"));
                } else {
                    _ = time_str.push_str(&alloc::format!("{seconds:01}.{ms:03}"));
                }

                _ = lcd
                    .print(&time_str, 0, PrintAlign::Center, true, delay)
                    .await;
            }
        }
        Scene::Finished { .. } => todo!(),
        Scene::Error { .. } => todo!(),
    }

    Some(())
}

async fn process_lcd_overwrite<C: CharsetWithFallback>(
    current_state: &SignaledGlobalStateInner,
    _global_state: &GlobalState,
    lcd: &mut LcdType<C>,
    delay: &mut Delay,
) -> bool {
    if !current_state.scene.can_be_lcd_overwritten() {
        return false;
    }

    if current_state.server_connected == Some(false) {
        _ = lcd
            .print("Server", 0, PrintAlign::Center, true, delay)
            .await;
        _ = lcd
            .print("Disconnected", 1, PrintAlign::Center, true, delay)
            .await;
    } else if current_state.stackmat_connected == Some(false) {
        _ = lcd
            .print("Stackmat", 0, PrintAlign::Center, true, delay)
            .await;
        _ = lcd
            .print("Disconnected", 1, PrintAlign::Center, true, delay)
            .await;
    } else {
        return false;
    }

    return true;
}

fn num_to_digits(mut num: u128) -> ([u8; 40], usize) {
    let mut tmp = [0xFF; 40];
    let mut pos = 0;
    while num > 0 {
        let digit = (num % 10) as u8;
        tmp[pos] = digit;

        pos += 1;
        num /= 10;
    }

    // reverse
    for i in 0..(pos / 2) {
        let end_i = pos - i - 1;
        tmp.swap(i, end_i);
    }

    (tmp, pos)
}

pub enum PrintAlign {
    Left,
    Center,
    Right,
}

pub trait LcdExt {
    async fn print<'a, D: DelayNs>(
        &mut self,
        text: &str,
        line: u8,
        align: PrintAlign,
        pad: bool,
        delay: &'a mut D,
    ) -> Result<(), ()>;
}

impl<B, M, C> LcdExt for HD44780<B, M, C>
where
    B: DataBus,
    M: DisplayMemoryMap,
    C: CharsetWithFallback,
{
    async fn print<'a, D: DelayNs>(
        &mut self,
        text: &str,
        line: u8,
        align: PrintAlign,
        pad: bool,
        delay: &'a mut D,
    ) -> Result<(), ()> {
        if line > 1 {
            return Err(());
        }

        let x_offset = if text.len() < 16 {
            match align {
                PrintAlign::Left => 0,
                PrintAlign::Center => (16 - text.len()) / 2,
                PrintAlign::Right => 16 - text.len(),
            }
        } else {
            0
        };

        let text = if text.len() > 16 { &text[..16] } else { text };
        if pad {
            let mut tmp_line = [b' '; 16];
            let end_offset = (x_offset + text.len()).min(16);
            tmp_line[x_offset..end_offset]
                .copy_from_slice(&text.as_bytes()[..(end_offset - x_offset)]);

            self.set_cursor_xy((0, line), delay).await.map_err(|_| ())?;
            self.write_bytes(&tmp_line, delay).await.map_err(|_| ())?;
        } else {
            self.set_cursor_xy((x_offset as u8, line), delay)
                .await
                .map_err(|_| ())?;
            self.write_str(&text, delay).await.map_err(|_| ())?;
        }

        Ok(())
    }
}
