use crate::{
    stackmat::CURRENT_TIME,
    state::{current_epoch, deeper_sleep_state, sleep_state, GlobalState, Scene},
    structs::DelegateResponsePacket,
    utils::buttons::{Button, ButtonTrigger, ButtonsHandler},
};
use alloc::string::ToString;
use embassy_time::{Instant, Timer};
use esp_hal::gpio::Input;

macros::generate_button_handler_enum!(triggered: &ButtonTrigger, hold_time: u64, state: &GlobalState);

#[embassy_executor::task]
pub async fn buttons_task(
    state: GlobalState,

    #[cfg(feature = "esp32c3")] button_input: Input<'static>,

    #[cfg(feature = "esp32c3")] button_reg: adv_shift_registers::wrappers::ShifterValue,

    #[cfg(feature = "esp32")] buttons: [Input<'static>; 4],
) {
    let mut handler = ButtonsHandler::new(Some(wakeup_button()));
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

    handler.add_handler(Button::First, ButtonTrigger::Down, room_left());
    handler.add_handler(Button::First, ButtonTrigger::Down, inspection_start());
    handler.add_handler(
        Button::First,
        ButtonTrigger::HoldOnce(1000),
        inspection_hold_stop(),
    );

    handler.add_handler(Button::Fourth, ButtonTrigger::Down, room_right());
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

    #[cfg(feature = "esp32c3")]
    {
        handler.run(&state, &button_input, &button_reg).await;
    }

    #[cfg(feature = "esp32")]
    {
        handler.run(&state, &buttons).await;
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
async fn wakeup_button(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    if sleep_state() {
        state.state.signal();
        if deeper_sleep_state() {
            esp_hal::system::software_reset();
        }

        return Ok(true);
    }

    Ok(false)
}

#[macros::button_handler]
async fn room_left(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state = state.state.lock().await;
    if state.scene == Scene::GroupSelect {
        state.group_selected_idx = state
            .group_selected_idx
            .wrapping_sub(1)
            .min(state.possible_groups.len() - 1);

        return Ok(true);
    }

    Ok(false)
}

#[macros::button_handler]
async fn room_right(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    let mut state = state.state.lock().await;
    if state.scene == Scene::GroupSelect {
        state.group_selected_idx += 1;
        if state.group_selected_idx == state.possible_groups.len() {
            state.group_selected_idx = 0;
        }

        return Ok(true);
    }

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
    if !state_val.device_added.unwrap_or(false) {
        crate::ws::send_packet(crate::structs::TimerPacket {
            tag: None,
            data: crate::structs::TimerPacketInner::Add {
                firmware: alloc::string::ToString::to_string(crate::version::FIRMWARE),
            },
        })
        .await;

        return Ok(false);
    }

    if state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if state_val.scene == Scene::GroupSelect {
        state_val.solve_group =
            Some(state_val.possible_groups[state_val.group_selected_idx].clone());
        if state_val.solve_time.is_some() {
            state_val.scene = crate::state::Scene::Finished;
        } else {
            state_val.scene = crate::state::Scene::CompetitorInfo;
        }

        state.state.signal();

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
    if !state_val.use_inspection() || state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if unsafe { CURRENT_TIME } != 0 {
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

    state.reset_solve_state(None).await;
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

    {
        let mut state = state.state.lock().await;
        state.custom_message = Some(("Resseting WIFI".to_string(), "Restart in 5s...".to_string()));
    }

    Timer::after_millis(5000).await;
    esp_hal::system::software_reset();
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
            if state_val.should_skip_other_actions() || state_val.delegate_used {
                return Ok(false);
            }

            if state_val.current_competitor.is_some() && state_val.solve_group.is_some() {
                let hold_secs = hold_time / 1000;
                let hold_secs = if hold_secs > 3 { 3 } else { hold_secs as u8 };

                state_val.delegate_hold = Some(hold_secs);
                state.state.signal();
            }
        }
        ButtonTrigger::HoldOnce(_) => {
            let mut state_val = state.state.lock().await;
            if state_val.should_skip_other_actions() || state_val.delegate_used {
                return Ok(false);
            }

            if state_val.session_id.is_none() {
                state_val.session_id = Some(uuid::Uuid::new_v4().to_string());
            }

            if state_val.current_competitor.is_none() || state_val.solve_group.is_none() {
                log::error!("Delegate hold: competitor or solve_group none!");
                return Ok(false);
            }

            let inspection_time = if state_val.use_inspection()
                && state_val.inspection_start.is_some()
                && state_val.inspection_end.is_some()
            {
                (state_val.inspection_end.expect("") - state_val.inspection_start.expect(""))
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
                competitor_id: state_val.current_competitor.expect(""),
                judge_id: state_val.current_judge.unwrap_or(0),
                timestamp: current_epoch(),
                session_id: state_val.session_id.clone().expect(""),
                delegate: true,
                inspection_time,
                group_id: state_val.solve_group.clone().map(|r| r.group_id).expect(""),
            };

            state_val.delegate_hold = Some(3);
            drop(state_val);

            let resp =
                crate::ws::send_tagged_request::<DelegateResponsePacket>(69420, packet, false)
                    .await;
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
                state_val.delegate_used = true;
                if !resp.should_scan_cards {
                    state_val.reset_solve_state(Some(&state.nvs)).await;
                }
            }
        }
        _ => {}
    }
    Ok(false)
}
