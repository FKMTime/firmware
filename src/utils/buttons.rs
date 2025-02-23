use crate::{
    buttons::HandlersDerive,
    state::{sleep_state, GlobalState},
};
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
    default_handler: Option<HandlersDerive>,

    handlers: Vec<ButtonHandler>,
    press_time: Instant,
    last_hold_execute: Instant,
    current_handler_down: Option<usize>,
}

impl ButtonsHandler {
    pub fn new(default_handler: Option<HandlersDerive>) -> Self {
        Self {
            default_handler,

            handlers: Vec::new(),
            press_time: Instant::now(),
            last_hold_execute: Instant::now(),
            current_handler_down: None,
        }
    }

    pub async fn run(
        &mut self,
        state: &GlobalState,

        #[cfg(feature = "esp32c3")] button_input: &Input<'static>,
        #[cfg(feature = "esp32c3")] button_reg: &adv_shift_registers::wrappers::ShifterValue,

        #[cfg(feature = "esp32")] buttons: &[Input<'static>],
    ) {
        let mut debounce_time = esp_hal::time::now();
        let mut old_debounced = i32::MAX;
        let mut old_val = 0;

        #[cfg(feature = "e2e")]
        let mut e2e_data = (esp_hal::time::now(), 0, 0);

        #[cfg(feature = "e2e")]
        let mut send_ack = false;

        loop {
            let mut out_val = 0;

            #[cfg(feature = "e2e")]
            {
                if state.e2e.buttons_sig.signaled() {
                    let (btn_idx, press_ms) = state.e2e.buttons_sig.wait().await;
                    out_val |= 1 << btn_idx;

                    e2e_data.0 = esp_hal::time::now();
                    e2e_data.1 = press_ms;
                    e2e_data.2 = btn_idx;
                    send_ack = true;

                    log::debug!("[E2E] Button pressed: {btn_idx} for {press_ms}ms");
                } else if (esp_hal::time::now() - e2e_data.0).to_millis() <= e2e_data.1 {
                    out_val |= 1 << e2e_data.2;
                }
            }

            #[cfg(feature = "esp32c3")]
            {
                let mut val = 0b10000000;
                for i in 0..4 {
                    button_reg.set_value(val);
                    if button_input.is_high() {
                        out_val |= 1 << i;
                    }

                    val >>= 1;
                }
            }

            #[cfg(feature = "esp32")]
            {
                for (i, button) in buttons.iter().enumerate() {
                    if button.is_low() {
                        out_val |= 1 << i;
                    }
                }
            }

            if old_val != out_val {
                old_val = out_val;
                debounce_time = esp_hal::time::now();
            } else if old_debounced != out_val {
                let duration = esp_hal::time::now() - debounce_time;
                if duration.to_millis() > 50 || sleep_state() {
                    if old_debounced == 0 {
                        self.button_down((out_val as u8).into(), state).await;
                    } else {
                        self.button_up(state).await;

                        #[cfg(feature = "e2e")]
                        if send_ack {
                            crate::ws::send_test_ack(&state).await;
                            send_ack = false;
                        }
                    }

                    old_debounced = out_val;
                }
            } else {
                debounce_time = esp_hal::time::now();
            }

            if old_debounced != 0 {
                self.button_hold(state).await;
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

        if let Some(ref default_handler) = self.default_handler {
            let res = default_handler
                .execute(&ButtonTrigger::Down, 0, state)
                .await;

            if res == Ok(true) {
                return;
            }
        }

        if let Some((i, handler)) = &mut handler {
            self.current_handler_down = Some(*i);

            for handler in &mut handler.handlers {
                handler.1 = false;

                if handler.0 == ButtonTrigger::Down {
                    let res = handler.2.execute(&handler.0, 0, state).await;
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
        let Some(current_handler_down) = self.current_handler_down else {
            return;
        };

        let handler = &mut self.handlers[current_handler_down];
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

                    let res = handler.execute(trigger, hold_time, state).await;
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
                    let res = handler.execute(trigger, hold_time, state).await;
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

                        let res = handler.execute(trigger, hold_time, state).await;
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
        let Some(current_handler_down) = self.current_handler_down else {
            return;
        };

        let hold_time = (Instant::now() - self.press_time).as_millis();
        if let Some(ref default_handler) = self.default_handler {
            let res = default_handler
                .execute(&ButtonTrigger::Up, hold_time, state)
                .await;

            if res == Ok(true) {
                self.current_handler_down = None;
                return;
            }
        }

        let handler = &self.handlers[current_handler_down];
        let handlers = handler.handlers.iter().filter(|h| h.0 == ButtonTrigger::Up);
        for handler in handlers {
            let res = handler.2.execute(&handler.0, hold_time, state).await;
            if let Err(e) = res {
                log::error!("buttons_handler:up_err: {e:?}");
            }
        }

        self.current_handler_down = None;
    }
}
