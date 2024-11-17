use crate::{
    state::{current_epoch, GlobalState, Scene},
    structs::DelegateResponsePacket,
};
use adv_shift_registers::wrappers::ShifterValue;
use alloc::{string::ToString, vec::Vec};
use embassy_time::{Instant, Timer};
use esp_hal::gpio::Input;

macros::generate_button_handler_enum!(triggered: &ButtonTrigger, hold_time: u64, state: &GlobalState);

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

#[embassy_executor::task]
pub async fn buttons_task(
    button_input: Input<'static>,
    button_reg: ShifterValue,
    state: GlobalState,
) {
    let mut handler = ButtonsHandler::new();
    handler.add_handler(Button::Third, ButtonTrigger::Up, submit_up());
    handler.add_handler(
        Button::Third,
        ButtonTrigger::HoldOnce(3000),
        submit_reset_competitor(),
    );
    handler.add_handler(
        Button::Third,
        ButtonTrigger::HoldOnce(15000),
        submit_reset_wifi(),
    );

    handler.add_handler(Button::First, ButtonTrigger::Down, inspection_start());
    handler.add_handler(
        Button::First,
        ButtonTrigger::HoldOnce(1000),
        inspection_hold_stop(),
    );

    handler.add_handler(Button::Fourth, ButtonTrigger::HoldOnce(1000), dnf_button());
    handler.add_handler(Button::Fourth, ButtonTrigger::Up, penalty_button());

    handler.add_handler(
        Button::Second,
        ButtonTrigger::HoldTimed(0, 1000),
        delegate_hold(),
    );
    handler.add_handler(
        Button::Second,
        ButtonTrigger::HoldOnce(3000),
        delegate_hold(),
    );
    handler.add_handler(Button::Second, ButtonTrigger::Up, delegate_hold());

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
                        handler.button_down((out_val as u8).into(), &state).await;
                    } else {
                        handler.button_up(&state).await;
                    }

                    old_debounced = out_val;
                }
            } else {
                debounce_time = esp_hal::time::now();
            }
        }

        if old_debounced != 0 {
            handler.button_hold(&state).await;
        }
        Timer::after_millis(5).await;
    }
}

struct ButtonHandler {
    button: Button,
    handlers: Vec<(ButtonTrigger, bool, HandlersDerive)>,
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

#[macros::button_handler]
async fn button_test(
    triggered: &ButtonTrigger,
    hold_time: u64,
    _state: &GlobalState,
) -> Result<bool, ()> {
    log::info!("Triggered: {triggered:?} - {hold_time}");
    Ok(false)
}

#[macros::button_handler]
async fn submit_up(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;

    // Clear error (text)
    if state_val.error_text.is_some() {
        state_val.error_text = None;
        state.state.signal();

        return Ok(false);
    }

    // Device add
    if state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if !state_val.device_added.unwrap_or(false) {
        crate::ws::send_packet(crate::structs::TimerPacket {
            tag: None,
            data: crate::structs::TimerPacketInner::Add {
                firmware: alloc::string::ToString::to_string(&"STATION"),
            },
        })
        .await;

        return Ok(false);
    }

    if state_val.scene == Scene::Finished && !state_val.time_confirmed {
        state_val.time_confirmed = true;
        state.state.signal();

        return Ok(false);
    }

    Ok(false)
}

#[macros::button_handler]
async fn inspection_start(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;
    if !state_val.use_inspection || state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if state_val.scene < Scene::Inspection
        && state_val.inspection_start.is_none()
        && state_val.solve_time.is_none()
    {
        state_val.inspection_start = Some(Instant::now());
        state_val.scene = Scene::Inspection;
        state.state.signal();

        return Ok(true);
    }

    Ok(false)
}

#[macros::button_handler]
async fn inspection_hold_stop(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;
    if state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if state_val.scene == Scene::Inspection {
        let scene = if state_val.current_competitor.is_none() {
            Scene::WaitingForCompetitor
        } else {
            Scene::CompetitorInfo
        };

        state_val.scene = scene;
        state_val.inspection_start = None;
        state_val.inspection_end = None;
        state.state.signal();
        return Ok(true);
    }

    Ok(false)
}

#[macros::button_handler]
async fn dnf_button(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;
    if state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if state_val.scene == Scene::Inspection {
        state_val.inspection_end = Some(Instant::now());
        state_val.solve_time = Some(0);
        state_val.penalty = Some(-1);
        state_val.time_confirmed = true;

        if state_val.current_competitor.is_some() {
            state_val.scene = Scene::Finished;
        } else {
            state_val.scene = Scene::WaitingForCompetitor;
        }

        state.state.signal();
        return Ok(true);
    } else if state_val.scene == Scene::Finished && !state_val.time_confirmed {
        let old_penalty = state_val.penalty.unwrap_or(0);
        state_val.penalty = Some(if old_penalty == -1 { 0 } else { -1 });

        state.state.signal();
        return Ok(true);
    }

    Ok(false)
}

#[macros::button_handler]
async fn penalty_button(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;
    if state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if state_val.scene == Scene::Finished && !state_val.time_confirmed {
        let old_penalty = state_val.penalty.unwrap_or(0);
        state_val.penalty = Some(if old_penalty >= 16 || old_penalty == -1 {
            0
        } else {
            old_penalty + 2
        });

        state.state.signal();
        return Ok(false);
    }

    Ok(false)
}

#[macros::button_handler]
async fn submit_reset_competitor(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state = state.state.lock().await;
    if state.should_skip_other_actions() {
        return Ok(false);
    }

    state.reset_solve_state();
    Ok(false)
}

#[macros::button_handler]
async fn submit_reset_wifi(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    _ = state
        .nvs
        .invalidate_key(esp_hal_wifimanager::WIFI_NVS_KEY)
        .await;
    Timer::after_millis(500).await;
    esp_hal::reset::software_reset();

    Ok(false)
}

#[macros::button_handler]
async fn delegate_hold(
    triggered: &ButtonTrigger,
    hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    match triggered {
        ButtonTrigger::Up => {
            state.state.lock().await.delegate_hold = None;
        }
        ButtonTrigger::HoldTimed(_, _) => {
            let mut state_val = state.state.value().await;
            if state_val.should_skip_other_actions() {
                return Ok(false);
            }

            if state_val.current_competitor.is_some() {
                let hold_secs = hold_time / 1000;
                let hold_secs = if hold_secs > 3 { 3 } else { hold_secs as u8 };

                state_val.delegate_hold = Some(hold_secs);
                state.state.signal();
            }
        }
        ButtonTrigger::HoldOnce(_) => {
            let mut state_val = state.state.lock().await;
            if state_val.should_skip_other_actions() {
                return Ok(false);
            }

            let inspection_time = if state_val.use_inspection
                && state_val.inspection_start.is_some()
                && state_val.inspection_end.is_some()
            {
                (state_val.inspection_end.unwrap() - state_val.inspection_start.unwrap())
                    .as_millis() as i64
            } else {
                0
            };

            if state_val.session_id.is_none() {
                state_val.session_id = Some(uuid::Uuid::new_v4().to_string());
            }

            let packet = crate::structs::TimerPacketInner::Solve {
                solve_time: state_val.solve_time.unwrap_or(0),
                penalty: state_val.penalty.unwrap_or(0) as i64,
                competitor_id: state_val.current_competitor.unwrap(),
                judge_id: state_val.current_judge.unwrap_or(0),
                timestamp: current_epoch(),
                session_id: state_val.session_id.clone().unwrap(),
                delegate: true,
                inspection_time,
            };

            state_val.delegate_hold = Some(3);
            drop(state_val);

            let resp =
                crate::ws::send_tagged_request::<DelegateResponsePacket>(69420, packet).await;
            log::info!("{:?}", resp);

            if let Ok(resp) = resp {
                let mut state_val = state.state.lock().await;
                state_val.solve_time =
                    Some(resp.solve_time.unwrap_or(state_val.solve_time.unwrap_or(0)));

                state_val.penalty = Some(
                    resp.penalty
                        .unwrap_or(state_val.penalty.unwrap_or(0) as i64) as i8,
                );
                state_val.scene = Scene::Finished;

                state_val.time_confirmed = true;
                if !resp.should_scan_cards {
                    state_val.reset_solve_state();
                }
            }
        }
        _ => {}
    }
    Ok(false)
}
