use adv_shift_registers::wrappers::ShifterValue;
use alloc::{boxed::Box, vec::Vec};
use core::{future::Future, pin::Pin};
use embassy_time::{Instant, Timer};
use esp_hal::gpio::Input;

use crate::{arc::Arc, scenes::GlobalState};

#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub enum Button {
    First,
    Second,
    Third,
    Fourth,
    Unknown,
}

impl From<u8> for Button {
    fn from(value: u8) -> Self {
        match value {
            0b00000001 => Self::First,
            0b00000010 => Self::Second,
            0b00000100 => Self::Third,
            0b00001000 => Self::Fourth,
            _ => Self::Unknown,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Clone)]
pub enum ButtonTrigger {
    Down,
    Up,
    HoldTimed(u64, u64),
    Hold,
}

type ButtonFunc =
    fn(ButtonTrigger, u64, GlobalState) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>;

#[embassy_executor::task]
pub async fn buttons_task(
    button_input: Input<'static>,
    button_reg: ShifterValue,
    state: GlobalState,
) {
    let mut handler = ButtonsHandler::new();
    handler.add_handler(Button::First, ButtonTrigger::Down, button_test());
    handler.add_handler(
        Button::First,
        ButtonTrigger::HoldTimed(500, 300),
        button_test(),
    );
    handler.add_handler(Button::First, ButtonTrigger::Up, button_test());
    handler.add_handler(Button::Fourth, ButtonTrigger::Down, button_test());

    handler.add_handler(Button::Second, ButtonTrigger::Hold, test_hold());
    handler.add_handler(Button::Second, ButtonTrigger::Up, test_hold());

    /*
    let mut triggers = vec![button_handler, button_test()];
    for trigger in triggers {
        let res = (trigger)(ButtonTrigger::Down(Button::First), 6940).await;
        log::info!("trigger res: {res:?}");
    }
    */

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
                    if old_debounced == 0 {
                        handler
                            .button_down((out_val as u8).into(), state.clone())
                            .await;
                    } else {
                        handler.button_up(state.clone()).await;
                    }

                    //log::info!("CHANGE: {out_val:08b}");
                    old_debounced = out_val;
                }
            } else {
                debounce_time = esp_hal::time::now();
            }
        }

        if old_debounced != 0 {
            handler.button_hold(state.clone()).await;
        }
        Timer::after_millis(5).await;
    }
}

struct ButtonHandler {
    button: Button,
    handlers: Vec<(ButtonTrigger, ButtonFunc)>,
}

struct ButtonsHandler {
    handlers: Vec<ButtonHandler>,
    press_time: Instant,
    last_hold_execute: Instant,
    current_handler_down: Option<usize>,
}

impl ButtonsHandler {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            press_time: Instant::now(),
            last_hold_execute: Instant::now(),
            current_handler_down: None,
        }
    }

    pub fn add_handler(&mut self, button: Button, trigger: ButtonTrigger, func: ButtonFunc) {
        let existing_handler = self.handlers.iter_mut().find(|h| h.button == button);
        match existing_handler {
            Some(handler) => handler.handlers.push((trigger, func)),
            None => self.handlers.push(ButtonHandler {
                button,
                handlers: alloc::vec![(trigger, func)],
            }),
        }
    }

    pub async fn button_down(&mut self, button: Button, state: GlobalState) {
        self.press_time = Instant::now();
        let handler = self
            .handlers
            .iter()
            .enumerate()
            .find(|(_, h)| h.button == button);

        if let Some((i, handler)) = handler {
            self.current_handler_down = Some(i);

            let handler = handler.handlers.iter().find(|h| h.0 == ButtonTrigger::Down);
            if let Some(handler) = handler {
                let res = (handler.1)(handler.0.clone(), 0, state).await;
                if let Err(e) = res {
                    log::error!("buttons_handler:down_err: {e:?}");
                }
            }
        }
    }

    pub async fn button_hold(&mut self, state: GlobalState) {
        if self.current_handler_down.is_none() {
            return;
        }

        let handler = &self.handlers[self.current_handler_down.expect("Cant fail")];
        let hold_time = (Instant::now() - self.press_time).as_millis();

        for (trigger, handler) in &handler.handlers {
            match trigger {
                ButtonTrigger::Down => continue,
                ButtonTrigger::Up => continue,
                ButtonTrigger::HoldTimed(offset, gap) => {
                    if hold_time < *offset
                        || (Instant::now() - self.last_hold_execute).as_millis() < *gap
                    {
                        break;
                    }

                    let res = (handler)(trigger.clone(), hold_time, state).await;
                    if let Err(e) = res {
                        log::error!("buttons_handler:hold_timed_err: {e:?}");
                    }

                    self.last_hold_execute = Instant::now();
                    break;
                }
                ButtonTrigger::Hold => {
                    let res = (handler)(trigger.clone(), hold_time, state).await;
                    if let Err(e) = res {
                        log::error!("buttons_handler:hold_err: {e:?}");
                    }

                    break;
                }
            }
        }
    }

    pub async fn button_up(&mut self, state: GlobalState) {
        if self.current_handler_down.is_none() {
            return;
        }

        let handler = &self.handlers[self.current_handler_down.expect("Cant fail")];
        let handler = handler.handlers.iter().find(|h| h.0 == ButtonTrigger::Up);
        if let Some(handler) = handler {
            let hold_time = (Instant::now() - self.press_time).as_millis();
            let res = (handler.1)(handler.0.clone(), hold_time, state).await;
            if let Err(e) = res {
                log::error!("buttons_handler:up_err: {e:?}");
            }
        }

        self.current_handler_down = None;
    }
}

#[macros::button_handler]
async fn button_test(
    triggered: ButtonTrigger,
    hold_time: u64,
    state: GlobalState,
) -> Result<(), ()> {
    log::info!("Triggered: {triggered:?} - {hold_time}");
    Ok(())
}

#[macros::button_handler]
async fn test_hold(triggered: ButtonTrigger, hold_time: u64, state: GlobalState) -> Result<(), ()> {
    match triggered {
        ButtonTrigger::Up => {
            state.state.lock().await.test_hold = None;
        }
        ButtonTrigger::Hold => {
            state.state.lock().await.test_hold = Some(hold_time);
        }
        _ => {}
    }
    Ok(())
}
