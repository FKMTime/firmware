use adv_shift_registers::wrappers::ShifterValue;
use alloc::rc::Rc;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Delay, Timer};
use embedded_hal::digital::OutputPin;
use hd44780_driver::{bus::FourBitBusPins, charset::CharsetA02, memory_map::MemoryMap1602, non_blocking::HD44780, setup::DisplayOptions4Bit, DisplayMode};

#[embassy_executor::task]
pub async fn lcd_task(lcd_shifter: ShifterValue, time_sig: Rc<Signal<NoopRawMutex, u64>>) {
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
            },
            Ok(lcd) => break lcd,
        }
    };
    _ = bl_pin.set_high();

    _ = lcd.clear(&mut delay).await;
    _ = lcd.reset(&mut delay).await;
    _ = lcd.set_display_mode(
        DisplayMode {
            display: hd44780_driver::Display::On,
            cursor_visibility: hd44780_driver::Cursor::Invisible,
            cursor_blink: hd44780_driver::CursorBlink::Off,
        },
        &mut delay,
    ).await;
    _ = lcd.clear(&mut delay).await;

    _ = lcd.write_str("Lorem Ipsum", &mut delay).await;
    _ = lcd.set_cursor_xy((13, 0), &mut delay).await;
    _ = lcd.write_str("WOW", &mut delay).await;

    _ = lcd.set_cursor_xy((0, 1), &mut delay).await;
    _ = lcd.write_str("Test 1234567890", &mut delay).await;

    Timer::after_millis(5000).await;
    _ = lcd.set_cursor_xy((5, 1), &mut delay).await;
    _ = lcd.write_bytes(&[b' '; 11], &mut delay).await;

    loop {
        let time_ms = time_sig.wait().await;
        _ = lcd.set_cursor_xy((5, 1), &mut delay).await;

        let minutes: u8 = (time_ms / 60000) as u8;
        let seconds: u8 = ((time_ms % 60000) / 1000) as u8;
        let ms: u16 = (time_ms % 1000) as u16;

        let mut time_str = heapless::String::<8>::new();
        if minutes > 0 {
            _ = time_str.push((minutes + b'0') as char);
            _ = time_str.push(':');
        }

        _ = time_str.push_str(&alloc::format!("{seconds:02}.{ms:03}"));
        _ = lcd.write_str(&time_str, &mut delay).await;
        _ = lcd.write_str(&" ".repeat(16 - 5 - time_str.len()), &mut delay).await;
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
