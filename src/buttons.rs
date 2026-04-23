use crate::{
    consts::{NVS_BONDING_KEY, NVS_ERROR_LOG, NVS_SIGN_KEY},
    stackmat::CURRENT_TIME,
    state::{
        BleAction, ErrorLogEntryStage, GlobalState, MenuScene, Scene, current_epoch,
        deeper_sleep_state, sleep_state,
    },
    structs::DelegateResponsePacket,
    utils::buttons::{Button, ButtonTrigger, ButtonsHandler},
};
use alloc::string::ToString;
use embassy_time::{Instant, Timer};
use esp_hal::gpio::Input;

macros::generate_button_handler_enum!(triggered: &ButtonTrigger, hold_time: u64, state: &GlobalState);

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
    let mut handler = ButtonsHandler::new(Some(wakeup_button()));
    handler.add_handler(Button::Third, ButtonTrigger::Up, submit_up());
    handler.add_handler(
        Button::Third,
        ButtonTrigger::HoldOnce(3000),
        submit_reset_competitor(),
    );
    handler.add_handler(
        Button::Third,
        ButtonTrigger::HoldOnce(10000),
        submit_config_menu(),
    );

    handler.add_handler(Button::First, ButtonTrigger::Down, sel_left());
    handler.add_handler(Button::First, ButtonTrigger::Down, inspection_start());
    handler.add_handler(
        Button::First,
        ButtonTrigger::HoldOnce(1000),
        inspection_hold_stop(),
    );

    handler.add_handler(Button::Fourth, ButtonTrigger::Down, sel_right());
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

    #[cfg(feature = "v3")]
    handler.run(&state, &button_input, &button_reg).await;
    #[cfg(feature = "v4")]
    handler.run(&state, &button_inputs).await;
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
async fn sel_left(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
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

        let total_items = state_val.error_log_entries.len() + 1; // + Exit
        state_val.selected_error_log_item = state_val
            .selected_error_log_item
            .wrapping_sub(1)
            .min(total_items - 1);

        return Ok(true);
    }

    if let Some(sel) = state_val.selected_config_menu.as_mut() {
        *sel = sel
            .wrapping_sub(1)
            .min(crate::structs::CONFIG_MENU_ITEMS.len() - 1);

        return Ok(true);
    }

    if state_val.menu_scene == Some(MenuScene::BtDisplay) {
        state_val.selected_bluetooth_item = state_val.selected_bluetooth_item.saturating_sub(1);

        return Ok(true);
    }

    if state_val.scene == Scene::GroupSelect {
        state_val.group_selected_idx = state_val
            .group_selected_idx
            .wrapping_sub(1)
            .min(state_val.possible_groups.len() - 1);

        return Ok(true);
    }

    Ok(false)
}

#[macros::button_handler]
async fn sel_right(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
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
            if let Some(entry_idx) = state_val.selected_error_log_entry {
                if let Some(entry) = state_val.error_log_entries.get(entry_idx) {
                    const VISIBLE_LINES: usize = 5;
                    let text = crate::lcd_v4::error_log_entry_details_text(entry);
                    let line_count = text.split('\n').count();
                    let max_scroll = line_count.saturating_sub(VISIBLE_LINES);
                    if state_val.error_log_details_scroll < max_scroll {
                        state_val.error_log_details_scroll += 1;
                        state.state.signal();
                    }
                }
            }
            return Ok(true);
        }

        if state_val.selected_error_log_entry.is_some() {
            return Ok(true);
        }

        let total_items = state_val.error_log_entries.len() + 1; // + Exit
        state_val.selected_error_log_item = (state_val.selected_error_log_item + 1) % total_items;

        return Ok(true);
    }

    if let Some(sel) = state_val.selected_config_menu.as_mut() {
        *sel += 1;
        if *sel == crate::structs::CONFIG_MENU_ITEMS.len() {
            *sel = 0;
        }

        return Ok(true);
    }

    if state_val.menu_scene == Some(MenuScene::BtDisplay) {
        if state_val.selected_bluetooth_item < state_val.discovered_bluetooth_devices.len() + 1 {
            state_val.selected_bluetooth_item += 1;
        }

        return Ok(true);
    }

    if state_val.scene == Scene::GroupSelect {
        state_val.group_selected_idx += 1;
        if state_val.group_selected_idx == state_val.possible_groups.len() {
            state_val.group_selected_idx = 0;
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
            if state_val.selected_bluetooth_item < state_val.discovered_bluetooth_devices.len() {
                log::debug!(
                    "[BtD] Try to connect to: {:?}",
                    state_val.discovered_bluetooth_devices[state_val.selected_bluetooth_item]
                );

                _ = state.nvs.delete(NVS_BONDING_KEY).await;
                state.ble_sig.signal(
                    BleAction::Connect(
                        state_val.discovered_bluetooth_devices[state_val.selected_bluetooth_item]
                            .clone(),
                    )
                    .clone(),
                );
            } else if state_val.selected_bluetooth_item
                == state_val.discovered_bluetooth_devices.len()
            {
                log::debug!("[BtD] Unpair current device");
                _ = state.nvs.delete(NVS_BONDING_KEY).await;
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
            if let Some(_entry_idx) = state_val.selected_error_log_entry {
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

            let exit_idx = state_val.error_log_entries.len();
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
                .set(crate::consts::NVS_BUZZER_VOLUME, current_volume)
                .await
            {
                log::error!("Cannot save buzzer volume to NVS: {e:?}");
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

                    _ = state.nvs.delete(esp_hal_wifimanager::WIFI_NVS_KEY).await;
                    _ = state.nvs.delete(NVS_SIGN_KEY).await;
                    _ = state.nvs.delete(NVS_BONDING_KEY).await;
                    _ = state.nvs.delete(NVS_ERROR_LOG).await;

                    Timer::after_millis(250).await;
                    esp_hal::system::software_reset();
                }
                1 => {
                    state_val.menu_scene = Some(MenuScene::BtDisplay);
                    state.ble_sig.signal(BleAction::StartScan);
                }
                2 => {
                    if unsafe { !crate::state::AUTO_SETUP } {
                        state_val.error_text = Some("AutoSetup Mode Disabled".to_string());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Signing);
                }
                3 => {
                    if unsafe { !crate::state::AUTO_SETUP } {
                        state_val.error_text = Some("AutoSetup Mode Disabled".to_string());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Unsigning);
                }
                CONFIG_MENU_ERROR_LOG_IDX => {
                    state_val.error_log_entries =
                        crate::utils::error_log::parse_error_log_entries().unwrap_or_else(|e| {
                            log::error!("Parse error log failed: {e:?}");
                            alloc::vec![]
                        });
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

                    _ = state.nvs.delete(esp_hal_wifimanager::WIFI_NVS_KEY).await;
                    _ = state.nvs.delete(NVS_SIGN_KEY).await;
                    _ = state.nvs.delete(NVS_BONDING_KEY).await;
                    _ = state.nvs.delete(NVS_ERROR_LOG).await;

                    Timer::after_millis(250).await;
                    esp_hal::system::software_reset();
                }
                1 => {
                    state_val.menu_scene = Some(MenuScene::BtDisplay);
                    state.ble_sig.signal(BleAction::StartScan);
                }
                2 => {
                    if unsafe { !crate::state::AUTO_SETUP } {
                        state_val.error_text = Some("AutoSetup Mode Disabled".to_string());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Signing);
                }
                3 => {
                    if unsafe { !crate::state::AUTO_SETUP } {
                        state_val.error_text = Some("AutoSetup Mode Disabled".to_string());
                        state.state.signal();
                        return Ok(true);
                    }

                    state_val.menu_scene = Some(MenuScene::Unsigning);
                }
                4 => {
                    state_val.menu_scene = Some(MenuScene::BuzzerVolume);
                }
                CONFIG_MENU_ERROR_LOG_IDX => {
                    state_val.error_log_entries =
                        crate::utils::error_log::parse_error_log_entries().unwrap_or_else(|e| {
                            log::error!("Parse error log failed: {e:?}");
                            alloc::vec![]
                        });
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

        _ = state.nvs.delete(NVS_SIGN_KEY).await;
        _ = state.nvs.set(NVS_SIGN_KEY, sign_key).await;
        unsafe { crate::state::SIGN_KEY = sign_key };
        unsafe { crate::state::TRUST_SERVER = true };

        crate::ws::send_packet(crate::structs::TimerPacket {
            tag: None,
            data: crate::structs::TimerPacketInner::Add {
                firmware: alloc::string::ToString::to_string(crate::version::FIRMWARE),
                sign_key: unsafe { crate::state::SIGN_KEY },
            },
        })
        .await;

        return Ok(true);
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
        panic!("test");
        return Ok(false);
    }

    if unsafe { CURRENT_TIME } != 0 {
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
async fn submit_config_menu(
    _triggered: &ButtonTrigger,
    _hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    {
        let mut state = state.state.lock().await;
        state.selected_config_menu = Some(0);
    }

    Ok(true)
}

#[macros::button_handler]
async fn delegate_hold(
    triggered: &ButtonTrigger,
    hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    match triggered {
        ButtonTrigger::Up => {
            crate::utils::error_log::add_error(69).await;
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

            let (Some(current_competitor), Some(solve_group)) = (
                state_val.current_competitor,
                state_val.solve_group.clone().map(|x| x.group_id),
            ) else {
                log::error!("Delegate hold: competitor or solve_group none!");
                return Ok(false);
            };

            #[cfg(not(feature = "e2e"))]
            if unsafe { !crate::state::TRUST_SERVER } {
                log::error!("Skipping delegate hold. Server not trusted!");
                return Ok(false);
            }

            let inspection_time = if state_val.use_inspection()
                && let Some(start) = state_val.inspection_start
                && let Some(end) = state_val.inspection_end
            {
                (end - start).as_millis() as i64
            } else {
                0
            };

            let session_id = match &state_val.session_id {
                Some(sess_id) => sess_id.clone(),
                None => {
                    let sess_id = uuid::Uuid::new_v4().to_string();
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
                sign_key: unsafe { crate::state::SIGN_KEY },
            };

            state_val.delegate_hold = Some(3);
            drop(state_val);

            let resp =
                crate::ws::send_tagged_request::<DelegateResponsePacket>(69420, packet, false)
                    .await;
            log::info!("Delegate resp: {resp:?}");

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
