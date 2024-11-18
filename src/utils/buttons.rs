use crate::{buttons::HandlersDerive, state::GlobalState};
use adv_shift_registers::wrappers::ShifterValue;
use alloc::vec::Vec;
use embassy_time::{Instant, Timer};
use esp_hal::gpio::Input;

#[allow(dead_code)]
#[derive(Debug, PartialEq, Clone)]
pub enum ButtonTrigger {
    Down,
    Up,
    HoldOnce(u64),
    HoldTimed(u64, u64),
    Hold,
}

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

pub struct ButtonHandler {
    button: Button,
    handlers: Vec<(ButtonTrigger, bool, HandlersDerive)>,
}

pub struct ButtonsHandler {
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

    pub async fn run(
        &mut self,
        state: &GlobalState,
        button_input: &Input<'static>,
        button_reg: &ShifterValue,
    ) {
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
                            self.button_down((out_val as u8).into(), &state).await;
                        } else {
                            self.button_up(&state).await;
                        }

                        old_debounced = out_val;
                    }
                } else {
                    debounce_time = esp_hal::time::now();
                }
            }

            if old_debounced != 0 {
                self.button_hold(&state).await;
            }

            Timer::after_millis(5).await;
        }
    }

    pub fn add_handler(&mut self, button: Button, trigger: ButtonTrigger, func: HandlersDerive) {
        let existing_handler = self.handlers.iter_mut().find(|h| h.button == button);
        match existing_handler {
            Some(handler) => handler.handlers.push((trigger, false, func)),
            None => self.handlers.push(ButtonHandler {
                button,
                handlers: alloc::vec![(trigger, false, func)],
            }),
        }
    }

    pub async fn button_down(&mut self, button: Button, state: &GlobalState) {
        self.press_time = Instant::now();
        let mut handler = self
            .handlers
            .iter_mut()
            .enumerate()
            .find(|(_, h)| h.button == button);

        if let Some((i, handler)) = &mut handler {
            self.current_handler_down = Some(*i);

            for handler in &mut handler.handlers {
                handler.1 = false;

                if handler.0 == ButtonTrigger::Down {
                    let res = handler.2.execute(&handler.0, 0, &state).await;
                    if let Err(e) = res {
                        log::error!("buttons_handler:down_err: {e:?}");
                    }

                    if res == Ok(true) {
                        self.current_handler_down = None; // skip other handlers
                        break;
                    }
                }
            }
        }
    }

    pub async fn button_hold(&mut self, state: &GlobalState) {
        if self.current_handler_down.is_none() {
            return;
        }

        let handler = &mut self.handlers[self.current_handler_down.expect("Cant fail")];
        let hold_time = (Instant::now() - self.press_time).as_millis();

        for (trigger, activated, handler) in &mut handler.handlers {
            match trigger {
                ButtonTrigger::Down => continue,
                ButtonTrigger::Up => continue,
                ButtonTrigger::HoldTimed(offset, gap) => {
                    if hold_time < *offset
                        || (Instant::now() - self.last_hold_execute).as_millis() < *gap
                    {
                        continue;
                    }

                    let res = handler.execute(&trigger, hold_time, &state).await;
                    if let Err(e) = res {
                        log::error!("buttons_handler:hold_timed_err: {e:?}");
                    }

                    self.last_hold_execute = Instant::now();
                    if res == Ok(true) {
                        self.current_handler_down = None; // skip other handlers
                        break;
                    }
                }
                ButtonTrigger::Hold => {
                    let res = handler.execute(&trigger, hold_time, &state).await;
                    if let Err(e) = res {
                        log::error!("buttons_handler:hold_err: {e:?}");
                    }

                    if res == Ok(true) {
                        self.current_handler_down = None; // skip other handlers
                        break;
                    }
                }
                ButtonTrigger::HoldOnce(after) => {
                    if hold_time > *after && !*activated {
                        *activated = true;

                        let res = handler.execute(&trigger, hold_time, &state).await;
                        if let Err(e) = res {
                            log::error!("buttons_handler:hold_once_err: {e:?}");
                        }

                        if res == Ok(true) {
                            self.current_handler_down = None; // skip other handlers
                            break;
                        }
                    }
                }
            }
        }
    }

    pub async fn button_up(&mut self, state: &GlobalState) {
        if self.current_handler_down.is_none() {
            return;
        }

        let handler = &self.handlers[self.current_handler_down.expect("Cant fail")];
        let handlers = handler.handlers.iter().filter(|h| h.0 == ButtonTrigger::Up);
        for handler in handlers {
            let hold_time = (Instant::now() - self.press_time).as_millis();
            let res = handler.2.execute(&handler.0, hold_time, &state).await;
            if let Err(e) = res {
                log::error!("buttons_handler:up_err: {e:?}");
            }
        }

        self.current_handler_down = None;
    }
}
