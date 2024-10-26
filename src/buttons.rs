use adv_shift_registers::wrappers::ShifterValue;
use alloc::{boxed::Box, vec, vec::Vec};
use core::{future::Future, pin::Pin};
use embassy_time::Timer;
use esp_hal::gpio::Input;

#[allow(dead_code)]
#[derive(Debug)]
pub enum ButtonTrigger {
    Down,
    Up,
    Hold(u64),
}

type ButtonFunc = fn(ButtonTrigger, u64) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>;
//type ButtonFunc = ;

#[embassy_executor::task]
pub async fn buttons_task(button_input: Input<'static>, button_reg: ShifterValue) {
    let button_handler: ButtonFunc = |trigger, value| {
        Box::pin(async move {
            log::info!("Handling trigger: {:?} with value: {}", trigger, value);
            Ok(())
        })
    };
    let mut triggers = vec![button_handler, button_test()];
    for trigger in triggers {
        let res = (trigger)(ButtonTrigger::Down, 6940).await;
        log::info!("trigger res: {res:?}");
    }

    let mut debounce_time = esp_hal::time::now();
    let mut old_debounced = i32::MAX;
    let mut old_val = 0;
    loop {
        let mut val = 0b10000000;
        let mut out_val = 0;
        for i in 0..4 {
            button_reg.set_value(val);
            if button_input.is_high() {
                out_val |= 1 << i;
            }

            /*
            let pin_value: u16 = nb::block!(adc1.read_oneshot(&mut adc1_pin)).unwrap();
            log::info!("i({i}): {:?}", pin_value);
            if pin_value > 100 {
                out_val |= 1 << i;
            }
            */

            val >>= 1;
        }

        if old_val != out_val {
            old_val = out_val;
            debounce_time = esp_hal::time::now();
        } else {
            if old_debounced != out_val {
                let duration = esp_hal::time::now() - debounce_time;
                if duration.to_millis() > 50 {
                    log::info!("CHANGE: {out_val:08b}");
                    old_debounced = out_val;
                }
            } else {
                debounce_time = esp_hal::time::now();
            }
        }

        Timer::after_millis(5).await;
    }
}

#[macros::button_handler]
async fn button_test(triggered: ButtonTrigger, hold_time: u64) -> Result<(), ()> {
    log::warn!("Triggered: {triggered:?}");
    Err(())
}
