// qa builds report button edges to the QA flow and never dispatch these handlers
#![cfg_attr(feature = "qa", allow(dead_code))]

use crate::{buttons::Handler, state::GlobalState};
use embassy_time::{Instant, Timer};
use esp_hal::gpio::Input;

#[cfg(feature = "v3")]
use crate::consts::BUTTON_DEBOUNCE_MS;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ButtonTrigger {
    Down,
    Up,
    /// Once per press, after the button was held for `ms`.
    HoldOnce(u64),
    /// Repeatedly while held: first after `offset` ms, then every `gap` ms.
    HoldTimed(u64, u64),
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Button {
    First,
    Second,
    Third,
    Fourth,
}

impl Button {
    /// Single-button mask to button (`None` for no or multiple buttons).
    fn from_mask(mask: u8) -> Option<Self> {
        match mask {
            0b0001 => Some(Self::First),
            0b0010 => Some(Self::Second),
            0b0100 => Some(Self::Third),
            0b1000 => Some(Self::Fourth),
            _ => None,
        }
    }
}

pub type HandlerTable = [(Button, ButtonTrigger, Handler)];

/// Dispatches button edges/holds to a const table of handlers.
///
/// Handlers of a button run in table order; a handler returning `Ok(true)` stops the rest
/// for this press.
pub struct ButtonsHandler {
    default_handler: Option<Handler>,
    table: &'static HandlerTable,

    press_time: Instant,
    last_hold_execute: Instant,
    current_button: Option<Button>,
    /// `HoldOnce` entries (by table index) already fired during this press.
    fired: u32,
}

impl ButtonsHandler {
    pub fn new(default_handler: Option<Handler>, table: &'static HandlerTable) -> Self {
        // `fired` is a bitmask over table entries
        assert!(table.len() <= 32);

        Self {
            default_handler,
            table,
            press_time: Instant::now(),
            last_hold_execute: Instant::now(),
            current_button: None,
            fired: 0,
        }
    }

    pub async fn run(
        &mut self,
        state: &GlobalState,
        #[cfg(feature = "v4")] button_inputs: &[Input<'static>],
        #[cfg(feature = "v3")] button_input: &Input<'static>,
        #[cfg(feature = "v3")] button_reg: &adv_shift_registers::wrappers::ShifterValue,
    ) {
        let mut old_val = 0u8;
        // Pending sample while waiting for v3 debounce stability.
        #[cfg(feature = "v3")]
        let mut debounce: Option<(u8, Instant)> = None;

        #[cfg(feature = "e2e")]
        let mut e2e_data = (Instant::now(), 0, 0);

        #[cfg(feature = "e2e")]
        let mut send_ack = false;

        #[cfg(feature = "qa")]
        let mut last_button_down = None;
        #[cfg(all(feature = "qa", not(feature = "e2e")))]
        let _ = state;

        loop {
            crate::watchdog::heartbeat(crate::watchdog::TaskId::Buttons);
            let mut out_val = 0u8;

            #[cfg(feature = "e2e")]
            {
                if let Some((btn_idx, press_ms)) = state.e2e.buttons_sig.try_take() {
                    out_val |= 1 << btn_idx;

                    e2e_data = (Instant::now(), press_ms, btn_idx);
                    send_ack = true;

                    log::debug!("[E2E] Button pressed: {btn_idx} for {press_ms}ms");
                } else if e2e_data.0.elapsed().as_millis() <= e2e_data.1 {
                    out_val |= 1 << e2e_data.2;
                }
            }

            #[cfg(feature = "v4")]
            for (i, btn) in button_inputs.iter().enumerate().take(4) {
                if btn.is_high() {
                    out_val |= 1 << i;
                }
            }

            #[cfg(feature = "v3")]
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

            // v3: only accept an edge after the new mask is stable for
            // BUTTON_DEBOUNCE_MS (filters bounce / scan glitches).
            // v4: keep immediate edges (per-pin pull-downs on GPIO).
            #[cfg(feature = "v3")]
            let edge = if out_val == old_val {
                debounce = None;
                None
            } else {
                match debounce {
                    Some((cand, since)) if cand == out_val => {
                        if since.elapsed().as_millis() >= BUTTON_DEBOUNCE_MS {
                            debounce = None;
                            Some((old_val, out_val))
                        } else {
                            None
                        }
                    }
                    _ => {
                        debounce = Some((out_val, Instant::now()));
                        None
                    }
                }
            };

            #[cfg(not(feature = "v3"))]
            let edge = (old_val != out_val).then_some((old_val, out_val));

            if let Some((prev, next)) = edge {
                if prev == 0 {
                    #[cfg(not(feature = "qa"))]
                    self.button_down(Button::from_mask(next), state).await;

                    #[cfg(feature = "qa")]
                    {
                        crate::qa::send_qa_resp(crate::qa::QaSignal::ButtonDown(next));
                        last_button_down = Some(next);
                        log::warn!("Button pressed down: {next}");
                    }
                } else {
                    #[cfg(not(feature = "qa"))]
                    self.button_up(state).await;

                    #[cfg(feature = "qa")]
                    if let Some(button) = last_button_down.take() {
                        crate::qa::send_qa_resp(crate::qa::QaSignal::ButtonUp(button));
                        log::warn!("Button pressed up: {button}");
                    }

                    #[cfg(feature = "e2e")]
                    if send_ack {
                        crate::ws::send_test_ack(state).await;
                        send_ack = false;
                    }
                }

                old_val = next;
            }

            #[cfg(not(feature = "qa"))]
            if old_val != 0 {
                self.button_hold(state).await;
            }

            Timer::after_millis(5).await;
        }
    }

    fn entries(
        &self,
        button: Button,
    ) -> impl Iterator<Item = (usize, ButtonTrigger, Handler)> + 'static {
        self.table
            .iter()
            .enumerate()
            .filter(move |(_, (b, _, _))| *b == button)
            .map(|(i, (_, trigger, handler))| (i, *trigger, *handler))
    }

    async fn button_down(&mut self, button: Option<Button>, state: &GlobalState) {
        self.press_time = Instant::now();

        if let Some(default_handler) = self.default_handler
            && default_handler
                .execute(&ButtonTrigger::Down, 0, state)
                .await
                == Ok(true)
        {
            return;
        }

        self.current_button = button;
        self.fired = 0;
        let Some(button) = button else {
            return;
        };

        for (_, trigger, handler) in self.entries(button) {
            if trigger != ButtonTrigger::Down {
                continue;
            }

            if self.run_handler(handler, &trigger, 0, state, "down").await {
                return;
            }
        }
    }

    async fn button_hold(&mut self, state: &GlobalState) {
        let Some(button) = self.current_button else {
            return;
        };

        let hold_time = self.press_time.elapsed().as_millis();
        for (i, trigger, handler) in self.entries(button) {
            match trigger {
                ButtonTrigger::Down | ButtonTrigger::Up => continue,
                ButtonTrigger::HoldTimed(offset, gap) => {
                    if hold_time < offset || self.last_hold_execute.elapsed().as_millis() < gap {
                        continue;
                    }

                    self.last_hold_execute = Instant::now();
                }
                ButtonTrigger::HoldOnce(after) => {
                    if hold_time <= after || self.fired & (1 << i) != 0 {
                        continue;
                    }

                    self.fired |= 1 << i;
                }
            }

            if self
                .run_handler(handler, &trigger, hold_time, state, "hold")
                .await
            {
                return;
            }
        }
    }

    async fn button_up(&mut self, state: &GlobalState) {
        let Some(button) = self.current_button.take() else {
            return;
        };

        let hold_time = self.press_time.elapsed().as_millis();
        if let Some(default_handler) = self.default_handler
            && default_handler
                .execute(&ButtonTrigger::Up, hold_time, state)
                .await
                == Ok(true)
        {
            return;
        }

        for (_, trigger, handler) in self.entries(button) {
            if trigger == ButtonTrigger::Up {
                let res = handler.execute(&trigger, hold_time, state).await;
                if let Err(e) = res {
                    log::error!("buttons_handler:up_err: {e:?}");
                }
            }
        }
    }

    /// Runs `handler`; returns true (and ends this press) if it consumed the event.
    async fn run_handler(
        &mut self,
        handler: Handler,
        trigger: &ButtonTrigger,
        hold_time: u64,
        state: &GlobalState,
        what: &str,
    ) -> bool {
        let res = handler.execute(trigger, hold_time, state).await;
        if let Err(e) = res {
            log::error!("buttons_handler:{what}_err: {e:?}");
        }

        if res == Ok(true) {
            self.current_button = None; // skip other handlers
            return true;
        }

        false
    }
}
