// qa builds report button edges to the QA flow and never dispatch these handlers
#![cfg_attr(feature = "qa", allow(dead_code))]

use crate::{
    consts::{NVS_BONDING_KEY, NVS_ERROR_LOG, NVS_SIGN_KEY},
    state::{
        BleAction, ErrorLogEntryStage, GlobalState, MenuScene, Scene, current_epoch,
        deeper_sleep_state, sleep_state,
    },
    utils::buttons::{Button, ButtonTrigger, ButtonsHandler, HandlerTable},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;
use portable_atomic::{AtomicBool, Ordering};

/// Delegate solve waiting to be sent; the reply can take minutes, so it is awaited next to
/// (not inside) the button loop.
static DELEGATE_REQUEST: Signal<CriticalSectionRawMutex, crate::structs::TimerPacketInner> =
    Signal::new();
static DELEGATE_PENDING: AtomicBool = AtomicBool::new(false);

/// Every button action; dispatched from [`HANDLERS`] by [`ButtonsHandler`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Handler {
    Wakeup,
    SelLeft,
    SelRight,
    SubmitUp,
    SubmitResetCompetitor,
    SubmitConfigMenu,
    InspectionStart,
    InspectionHoldStop,
    Dnf,
    Penalty,
    DelegateHold,
}

impl Handler {
    pub async fn execute(
        self,
        triggered: &ButtonTrigger,
        hold_time: u64,
        state: &GlobalState,
    ) -> Result<bool, ()> {
        match self {
            Self::Wakeup => wakeup_button(state).await,
            Self::SelLeft => sel_left(state).await,
            Self::SelRight => sel_right(state).await,
            Self::SubmitUp => submit_up(state).await,
            Self::SubmitResetCompetitor => submit_reset_competitor(state).await,
            Self::SubmitConfigMenu => submit_config_menu(state).await,
            Self::InspectionStart => inspection_start(state).await,
            Self::InspectionHoldStop => inspection_hold_stop(state).await,
            Self::Dnf => dnf_button(state).await,
            Self::Penalty => penalty_button(state).await,
            Self::DelegateHold => delegate_hold(triggered, hold_time, state).await,
        }
    }
}

/// Index after moving one step left/right in a list of `len` items, wrapping around.
const fn wrap_step(idx: usize, len: usize, right: bool) -> usize {
    if len == 0 {
        return 0;
    }

    let idx = if idx < len { idx } else { len - 1 };
    if right {
        (idx + 1) % len
    } else if idx == 0 {
        len - 1
    } else {
        idx - 1
    }
}

const _: () = assert!(
    wrap_step(0, 3, false) == 2
        && wrap_step(2, 3, true) == 0
        && wrap_step(1, 3, true) == 2
        && wrap_step(9, 3, true) == 0
        && wrap_step(5, 3, false) == 1
        && wrap_step(4, 0, true) == 0
        && wrap_step(0, 1, false) == 0
);

/// Per button, in execution order.
const HANDLERS: &HandlerTable = &[
    (Button::Third, ButtonTrigger::Up, Handler::SubmitUp),
    (
        Button::Third,
        ButtonTrigger::HoldOnce(3000),
        Handler::SubmitResetCompetitor,
    ),
    (
        Button::Third,
        ButtonTrigger::HoldOnce(10000),
        Handler::SubmitConfigMenu,
    ),
    (Button::First, ButtonTrigger::Down, Handler::SelLeft),
    (Button::First, ButtonTrigger::Down, Handler::InspectionStart),
    (
        Button::First,
        ButtonTrigger::HoldOnce(1000),
        Handler::InspectionHoldStop,
    ),
    (Button::Fourth, ButtonTrigger::Down, Handler::SelRight),
    (Button::Fourth, ButtonTrigger::HoldOnce(1000), Handler::Dnf),
    (Button::Fourth, ButtonTrigger::Up, Handler::Penalty),
    (
        Button::Second,
        ButtonTrigger::HoldTimed(0, 1000),
        Handler::DelegateHold,
    ),
    (
        Button::Second,
        ButtonTrigger::HoldOnce(3000),
        Handler::DelegateHold,
    ),
    (Button::Second, ButtonTrigger::Up, Handler::DelegateHold),
];

#[cfg(feature = "v3")]
const CONFIG_MENU_ERROR_LOG_IDX: usize = 4;
#[cfg(feature = "v3")]
const CONFIG_MENU_EXIT_IDX: usize = 5;

#[cfg(feature = "v4")]
const CONFIG_MENU_BUZZER_IDX: usize = 4;
#[cfg(feature = "v4")]
const CONFIG_MENU_ERROR_LOG_IDX: usize = 5;
#[cfg(feature = "v4")]
const CONFIG_MENU_EXIT_IDX: usize = 6;

#[embassy_executor::task]
pub async fn buttons_task(
    state: GlobalState,
    #[cfg(feature = "v4")] button_inputs: [Input<'static>; 4],
    #[cfg(feature = "v3")] button_input: Input<'static>,
    #[cfg(feature = "v3")] button_reg: adv_shift_registers::wrappers::ShifterValue,
) {
    let mut handler = ButtonsHandler::new(Some(Handler::Wakeup), HANDLERS);

    #[cfg(feature = "v3")]
    let buttons = handler.run(&state, &button_input, &button_reg);
    #[cfg(feature = "v4")]
    let buttons = handler.run(&state, &button_inputs);

    embassy_futures::join::join(buttons, delegate_loop(state)).await;
}

async fn delegate_loop(state: GlobalState) {
    loop {
        let packet = DELEGATE_REQUEST.wait().await;
        let resp = crate::ws::delegate_request(
            packet,
            Duration::from_millis(crate::consts::DELEGATE_TIMEOUT_MS),
        )
        .await;
        log::info!("Delegate resp: {resp:?}");

        let mut state_val = state.state.lock().await;
        match resp {
            Ok(resp) => {
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
                    state_val.reset_solve_state(Some(state.nvs)).await;
                }
            }
            // no answer: drop the "hold" indicator, the solve stays as it was
            Err(_) => state_val.delegate_hold = None,
        }

        DELEGATE_PENDING.store(false, Ordering::Relaxed);
    }
}

async fn wakeup_button(state: &GlobalState) -> Result<bool, ()> {
    if sleep_state() {
        state.state.signal();
        if deeper_sleep_state() {
            esp_hal::system::software_reset();
        }

        return Ok(true);
    }

    Ok(false)
}

async fn sel_left(state: &GlobalState) -> Result<bool, ()> {
    let mut state_val = state.state.lock().await;
    #[cfg(feature = "v4")]
    if state_val.menu_scene == Some(MenuScene::BuzzerVolume) {
        let old_volume = crate::state::buzzer_volume();
        if old_volume > crate::consts::BUZZER_VOLUME_MIN {
            let new_volume = old_volume.saturating_sub(1);
            crate::state::set_buzzer_volume(new_volume);
            drop(state_val);
            state.state.signal();
            state.buzzer_sound_test.signal(());
        }

        return Ok(true);
    }

    if state_val.menu_scene == Some(MenuScene::ErrorLog) {
        #[cfg(feature = "v4")]
        if state_val.error_log_entry_stage == Some(ErrorLogEntryStage::Details) {
            state_val.error_log_details_scroll =
                state_val.error_log_details_scroll.saturating_sub(1);
            state.state.signal();
            return Ok(true);
        }

        if state_val.selected_error_log_entry.is_some() {
            return Ok(true);
        }

        let total_items = crate::utils::error_log::entry_count() + 1; // + Exit
        state_val.selected_error_log_item =
            wrap_step(state_val.selected_error_log_item, total_items, false);

        return Ok(true);
    }

    if let Some(sel) = state_val.selected_config_menu.as_mut() {
        *sel = wrap_step(*sel, crate::structs::CONFIG_MENU_ITEMS.len(), false);

        return Ok(true);
    }

    if state_val.menu_scene == Some(MenuScene::BtDisplay) {
        state_val.selected_bluetooth_item = state_val.selected_bluetooth_item.saturating_sub(1);

        return Ok(true);
    }

    if state_val.scene == Scene::GroupSelect {
        state_val.group_selected_idx = wrap_step(
            state_val.group_selected_idx,
            state_val.possible_groups.len(),
            false,
        );

        return Ok(true);
    }

    Ok(false)
}

async fn sel_right(state: &GlobalState) -> Result<bool, ()> {
    let mut state_val = state.state.lock().await;
    #[cfg(feature = "v4")]
    if state_val.menu_scene == Some(MenuScene::BuzzerVolume) {
        let old_volume = crate::state::buzzer_volume();
        if old_volume < crate::consts::BUZZER_VOLUME_MAX {
            let new_volume = (old_volume + 1).min(crate::consts::BUZZER_VOLUME_MAX);
            crate::state::set_buzzer_volume(new_volume);
            drop(state_val);
            state.state.signal();
            state.buzzer_sound_test.signal(());
        }

        return Ok(true);
    }

    if state_val.menu_scene == Some(MenuScene::ErrorLog) {
        #[cfg(feature = "v4")]
        if state_val.error_log_entry_stage == Some(ErrorLogEntryStage::Details) {
            if let Some(entry_idx) = state_val.selected_error_log_entry
                && let Some(entry) = crate::utils::error_log::entry(entry_idx)
            {
                let text = crate::lcd_v4::error_log_entry_details_text(&entry);
                let max_scroll = text
                    .split('\n')
                    .count()
                    .saturating_sub(crate::lcd_v4::DETAILS_VISIBLE_LINES);
                if state_val.error_log_details_scroll < max_scroll {
                    state_val.error_log_details_scroll += 1;
                    state.state.signal();
                }
            }
            return Ok(true);
        }

        if state_val.selected_error_log_entry.is_some() {
            return Ok(true);
        }

        let total_items = crate::utils::error_log::entry_count() + 1; // + Exit
        state_val.selected_error_log_item =
            wrap_step(state_val.selected_error_log_item, total_items, true);

        return Ok(true);
    }

    if let Some(sel) = state_val.selected_config_menu.as_mut() {
        *sel = wrap_step(*sel, crate::structs::CONFIG_MENU_ITEMS.len(), true);

        return Ok(true);
    }

    if state_val.menu_scene == Some(MenuScene::BtDisplay) {
        if state_val.selected_bluetooth_item < state_val.discovered_bluetooth_devices.len() + 1 {
            state_val.selected_bluetooth_item += 1;
        }

        return Ok(true);
    }

    if state_val.scene == Scene::GroupSelect {
        state_val.group_selected_idx = wrap_step(
            state_val.group_selected_idx,
            state_val.possible_groups.len(),
            true,
        );

        return Ok(true);
    }

    Ok(false)
}

async fn submit_up(state: &GlobalState) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;

    // Clear error (text)
    if state_val.error_text.is_some() {
        state_val.error_text = None;
        state.state.signal();

        return Ok(true);
    }

    match state_val.menu_scene {
        Some(MenuScene::Signing) | Some(MenuScene::Unsigning) => {
            state_val.menu_scene = None;
            state_val.selected_config_menu = Some(0);
            state.state.signal();
            return Ok(true);
        }
        Some(MenuScene::BtDisplay) => {
            if let Some(device) = state_val
                .discovered_bluetooth_devices
                .get(state_val.selected_bluetooth_item)
                .cloned()
            {
                log::debug!("[BtD] Try to connect to: {device:?}");

                _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                state.ble_sig.signal(BleAction::Connect(device));
            } else if state_val.selected_bluetooth_item
                == state_val.discovered_bluetooth_devices.len()
            {
                log::debug!("[BtD] Unpair current device");
                _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                state.ble_sig.signal(BleAction::Unpair);
            } else if state_val.selected_bluetooth_item
                == state_val.discovered_bluetooth_devices.len() + 1
            {
                log::debug!("[BtD] Exit");
            }

            state_val.menu_scene = None;
            state_val.selected_bluetooth_item = 0;
            state_val.selected_config_menu = Some(0);
            state.state.signal();
            return Ok(true);
        }
        Some(MenuScene::ErrorLog) => {
            if state_val.selected_error_log_entry.is_some() {
                #[cfg(feature = "v4")]
                if state_val.error_log_entry_stage == Some(ErrorLogEntryStage::Qr) {
                    state_val.error_log_entry_stage = Some(ErrorLogEntryStage::Details);
                    state_val.error_log_details_scroll = 0;
                    state.state.signal();
                    return Ok(true);
                }

                state_val.selected_error_log_entry = None;
                state_val.error_log_entry_stage = None;
                state_val.error_log_details_scroll = 0;
                state.state.signal();
                return Ok(true);
            }

            let exit_idx = crate::utils::error_log::entry_count();
            if state_val.selected_error_log_item == exit_idx {
                state_val.menu_scene = None;
                state_val.selected_error_log_item = 0;
                state_val.selected_error_log_entry = None;
                state_val.error_log_entry_stage = None;
                state_val.error_log_details_scroll = 0;
                state_val.selected_config_menu = Some(CONFIG_MENU_ERROR_LOG_IDX);
            } else {
                state_val.selected_error_log_entry = Some(state_val.selected_error_log_item);
                state_val.error_log_details_scroll = 0;
                #[cfg(feature = "v3")]
                {
                    state_val.error_log_entry_stage = Some(ErrorLogEntryStage::Details);
                }
                #[cfg(feature = "v4")]
                {
                    state_val.error_log_entry_stage = Some(ErrorLogEntryStage::Qr);
                }
            }

            state.state.signal();
            return Ok(true);
        }
        #[cfg(feature = "v4")]
        Some(MenuScene::BuzzerVolume) => {
            let current_volume = crate::state::buzzer_volume();
            if let Err(e) = state
                .nvs
                .set(&crate::consts::NVS_BUZZER_VOLUME, current_volume)
                .await
            {
                log::error!("Cannot save buzzer volume to NVS: {e:?}");
                crate::log_once!(crate::utils::error_log::codes::NVS_BUZZER_VOLUME_WRITE_FAILED);
            }

            state_val.menu_scene = None;
            state_val.selected_config_menu = Some(CONFIG_MENU_BUZZER_IDX);
            state.state.signal();
            return Ok(true);
        }
        _ => {}
    }

    if let Some(sel) = state_val.selected_config_menu {
        #[cfg(feature = "v3")]
        {
            match sel {
                0 => {
                    // Reset settings

                    _ = state.nvs.delete(&esp_hal_wifimanager::WIFI_NVS_KEY).await;
                    _ = state.nvs.delete(&NVS_SIGN_KEY).await;
                    _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                    _ = state.nvs.delete(&NVS_ERROR_LOG).await;

                    Timer::after_millis(250).await;
                    esp_hal::system::software_reset();
                }
                1 => {
                    state_val.menu_scene = Some(MenuScene::BtDisplay);
                    state.ble_sig.signal(BleAction::StartScan);
                }
                2 => {
                    if !crate::state::auto_setup() {
                        state_val.error_text = Some("AutoSetup Mode Disabled".into());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Signing);
                }
                3 => {
                    if !crate::state::auto_setup() {
                        state_val.error_text = Some("AutoSetup Mode Disabled".into());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Unsigning);
                }
                CONFIG_MENU_ERROR_LOG_IDX => {
                    state_val.selected_error_log_item = 0;
                    state_val.selected_error_log_entry = None;
                    state_val.error_log_entry_stage = None;
                    state_val.menu_scene = Some(MenuScene::ErrorLog);
                }
                CONFIG_MENU_EXIT_IDX => {} // Exit
                _ => {}
            }
        }

        #[cfg(feature = "v4")]
        {
            match sel {
                0 => {
                    // Reset settings

                    _ = state.nvs.delete(&esp_hal_wifimanager::WIFI_NVS_KEY).await;
                    _ = state.nvs.delete(&NVS_SIGN_KEY).await;
                    _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                    _ = state.nvs.delete(&NVS_ERROR_LOG).await;

                    Timer::after_millis(250).await;
                    esp_hal::system::software_reset();
                }
                1 => {
                    state_val.menu_scene = Some(MenuScene::BtDisplay);
                    state.ble_sig.signal(BleAction::StartScan);
                }
                2 => {
                    if !crate::state::auto_setup() {
                        state_val.error_text = Some("AutoSetup Mode Disabled".into());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Signing);
                }
                3 => {
                    if !crate::state::auto_setup() {
                        state_val.error_text = Some("AutoSetup Mode Disabled".into());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Unsigning);
                }
                4 => {
                    state_val.menu_scene = Some(MenuScene::BuzzerVolume);
                }
                CONFIG_MENU_ERROR_LOG_IDX => {
                    state_val.selected_error_log_item = 0;
                    state_val.selected_error_log_entry = None;
                    state_val.error_log_entry_stage = None;
                    state_val.menu_scene = Some(MenuScene::ErrorLog);
                }
                CONFIG_MENU_EXIT_IDX => {} // Exit
                _ => {}
            }
        }

        state_val.selected_config_menu = None;
        state.state.signal();

        return Ok(true);
    }

    // Device add
    if !state_val.device_added.unwrap_or(false) {
        let mut sign_key = [0; 4];
        _ = getrandom::getrandom(&mut sign_key);
        let sign_key = u32::from_be_bytes(sign_key) >> 1;

        _ = state.nvs.set(&NVS_SIGN_KEY, sign_key).await;
        crate::state::set_sign_key(sign_key);
        crate::state::set_trust_server(true);

        if crate::ws::send(crate::structs::TimerPacket {
            tag: None,
            data: crate::structs::TimerPacketInner::Add {
                firmware: crate::version::FIRMWARE.into(),
                sign_key: crate::state::sign_key(),
            },
        })
        .is_err()
        {
            log::error!("Add packet dropped: ws queue full");
        }

        return Ok(true);
    }

    if state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if state_val.scene == Scene::GroupSelect {
        let Some(group) = state_val
            .possible_groups
            .get(state_val.group_selected_idx)
            .cloned()
        else {
            log::error!("Group select: no group at {}", state_val.group_selected_idx);
            return Ok(false);
        };

        crate::state::set_group_limit(group.limit);
        state_val.solve_group = Some(group);
        if state_val.solve_time.is_some() {
            state_val.scene = crate::state::Scene::Finished;
        } else {
            state_val.scene = crate::state::Scene::CompetitorInfo;
        }

        state.state.signal();

        return Ok(false);
    } else if state_val.scene == Scene::Timer {
        #[cfg(not(feature = "timer-func"))]
        state.timer_stop_signal.signal(());
    }

    if state_val.scene == Scene::Finished && !state_val.time_confirmed {
        state_val.time_confirmed = true;
        state.state.signal();

        return Ok(false);
    }

    Ok(false)
}

async fn inspection_start(state: &GlobalState) -> Result<bool, ()> {
    let mut state_val = state.state.value().await;
    if !state_val.use_inspection() || state_val.should_skip_other_actions() {
        return Ok(false);
    }

    if crate::state::current_time() != Some(0) {
        log::warn!("Skipping inspection start because current timer time is not 0");
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

async fn inspection_hold_stop(state: &GlobalState) -> Result<bool, ()> {
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

async fn dnf_button(state: &GlobalState) -> Result<bool, ()> {
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

async fn penalty_button(state: &GlobalState) -> Result<bool, ()> {
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

async fn submit_reset_competitor(global_state: &GlobalState) -> Result<bool, ()> {
    let mut state = global_state.state.lock().await;
    if state.should_skip_other_actions() {
        return Ok(false);
    }

    global_state.timer_stop_signal.signal(());
    state.reset_solve_state(None).await;
    Ok(false)
}

async fn submit_config_menu(state: &GlobalState) -> Result<bool, ()> {
    {
        let mut state = state.state.lock().await;
        state.selected_config_menu = Some(0);
    }

    Ok(true)
}

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
                let hold_secs = (hold_time / 1000).min(3) as u8;

                state_val.delegate_hold = Some(hold_secs);
                state.state.signal();
            }
        }
        ButtonTrigger::HoldOnce(_) => {
            let mut state_val = state.state.lock().await;
            if state_val.should_skip_other_actions()
                || state_val.delegate_used
                || DELEGATE_PENDING.load(Ordering::Relaxed)
            {
                return Ok(false);
            }

            let (Some(current_competitor), Some(solve_group)) = (
                state_val.current_competitor,
                state_val.solve_group.clone().map(|x| x.group_id),
            ) else {
                log::error!("Delegate hold: competitor or solve_group none!");
                return Ok(false);
            };

            #[cfg(not(feature = "e2e"))]
            if !crate::state::trust_server() {
                log::error!("Skipping delegate hold. Server not trusted!");
                return Ok(false);
            }

            let inspection_time = if state_val.use_inspection()
                && let Some(start) = state_val.inspection_start
                && let Some(end) = state_val.inspection_end
            {
                end.saturating_duration_since(start).as_millis() as i64
            } else {
                0
            };

            let session_id = match &state_val.session_id {
                Some(sess_id) => sess_id.clone(),
                None => {
                    let sess_id = crate::state::new_session_id();
                    state_val.session_id = Some(sess_id.clone());
                    sess_id
                }
            };

            let packet = crate::structs::TimerPacketInner::Solve {
                solve_time: state_val.solve_time.unwrap_or(0),
                penalty: state_val.penalty.unwrap_or(0) as i64,
                competitor_id: current_competitor,
                judge_id: state_val.current_judge.unwrap_or(0),
                timestamp: current_epoch(),
                session_id,
                delegate: true,
                inspection_time,
                group_id: solve_group,
                sign_key: crate::state::sign_key(),
            };

            state_val.delegate_hold = Some(3);
            DELEGATE_PENDING.store(true, Ordering::Relaxed);
            DELEGATE_REQUEST.signal(packet);
        }
        _ => {}
    }
    Ok(false)
}
