// qa builds report button edges to the QA flow and never dispatch these handlers
#![cfg_attr(feature = "qa", allow(dead_code))]

use crate::bluetooth::BleAction;
use crate::{
    consts::{NVS_BONDING_KEY, NVS_ERROR_LOG, NVS_SIGN_KEY},
    state::{ErrorLogEntryStage, GlobalState, MenuScene, Scene, current_epoch},
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

#[cfg(feature = "v4")]
const CONFIG_MENU_BUZZER_IDX: usize = 4;
#[cfg(feature = "v4")]
const CONFIG_MENU_ERROR_LOG_IDX: usize = 5;

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

        match resp {
            Ok(resp) => {
                state.solve.update(|s| {
                    s.solve_time = Some(resp.solve_time.unwrap_or(s.solve_time.unwrap_or(0)));
                    s.penalty =
                        Some(resp.penalty.unwrap_or(s.penalty.unwrap_or(0) as i64) as i8);
                    s.scene = Scene::Finished;
                    s.time_confirmed = true;
                    s.delegate_used = true;
                });
                if !resp.should_scan_cards {
                    state.reset_solve(true).await;
                }
            }
            // no answer: drop the "hold" indicator, the solve stays as it was
            Err(_) => state.solve.update(|s| s.delegate_hold = None),
        }

        DELEGATE_PENDING.store(false, Ordering::Relaxed);
    }
}

async fn wakeup_button(_state: &GlobalState) -> Result<bool, ()> {
    if crate::power::is_sleeping() {
        crate::state::notify();
        if crate::power::is_deeper_sleeping() {
            esp_hal::system::software_reset();
        }

        return Ok(true);
    }

    Ok(false)
}

async fn sel_left(state: &GlobalState) -> Result<bool, ()> {
    Ok(select_step(state, false))
}

async fn sel_right(state: &GlobalState) -> Result<bool, ()> {
    Ok(select_step(state, true))
}

/// Moves the cursor of whatever list is shown one step left/right.
fn select_step(state: &GlobalState, right: bool) -> bool {
    #[cfg(feature = "v4")]
    if state.ui.read(|u| u.menu_scene) == Some(MenuScene::BuzzerVolume) {
        let changed = crate::state::SETTINGS.write(|s| {
            let old = s.buzzer_volume;
            s.buzzer_volume = match right {
                true => (old + 1).min(crate::consts::BUZZER_VOLUME_MAX),
                false => old.saturating_sub(1).max(crate::consts::BUZZER_VOLUME_MIN),
            };
            s.buzzer_volume != old
        });
        if changed {
            crate::state::notify();
            state.buzzer_sound_test.signal(());
        }
        return true;
    }

    let handled = state.ui.update(|u| {
        if u.menu_scene == Some(MenuScene::ErrorLog) {
            #[cfg(feature = "v4")]
            if u.error_log_entry_stage == Some(ErrorLogEntryStage::Details) {
                u.error_log_details_scroll = match right {
                    true => (u.error_log_details_scroll + 1).min(details_max_scroll(u)),
                    false => u.error_log_details_scroll.saturating_sub(1),
                };
                return true;
            }

            if u.selected_error_log_entry.is_none() {
                let total_items = crate::utils::error_log::entry_count() + 1; // + Exit
                u.selected_error_log_item = wrap_step(u.selected_error_log_item, total_items, right);
            }
            return true;
        }

        if let Some(sel) = u.selected_config_menu.as_mut() {
            *sel = wrap_step(*sel, crate::structs::CONFIG_MENU_ITEMS.len(), right);
            return true;
        }

        if u.menu_scene == Some(MenuScene::BtDisplay) {
            // devices + "Unpair" + "Exit"
            let last = u.discovered_bluetooth_devices.len() + 1;
            u.selected_bluetooth_item = match right {
                true => (u.selected_bluetooth_item + 1).min(last),
                false => u.selected_bluetooth_item.saturating_sub(1),
            };
            return true;
        }

        false
    });
    if handled {
        return true;
    }

    state.solve.update(|s| {
        if s.scene != Scene::GroupSelect {
            return false;
        }
        s.group_selected_idx = wrap_step(s.group_selected_idx, s.possible_groups.len(), right);
        true
    })
}

/// Last scroll position of the error log details view.
#[cfg(feature = "v4")]
fn details_max_scroll(u: &crate::state::Ui) -> usize {
    u.selected_error_log_entry
        .and_then(crate::utils::error_log::entry)
        .map_or(0, |entry| {
            crate::lcd_v4::error_log_entry_details_text(&entry)
                .split('\n')
                .count()
                .saturating_sub(crate::lcd_v4::DETAILS_VISIBLE_LINES)
        })
}

async fn submit_up(state: &GlobalState) -> Result<bool, ()> {
    // Clear error (text)
    if state.ui.update(|u| u.error_text.take().is_some()) {
        return Ok(true);
    }

    let menu_scene = state.ui.read(|u| u.menu_scene);
    match menu_scene {
        Some(MenuScene::Signing) | Some(MenuScene::Unsigning) => {
            state.ui.update(|u| {
                u.menu_scene = None;
                u.selected_config_menu = Some(0);
            });
            return Ok(true);
        }
        Some(MenuScene::BtDisplay) => {
            let (selected, device, count) = state.ui.read(|u| {
                let sel = u.selected_bluetooth_item;
                (sel, u.discovered_bluetooth_devices.get(sel).cloned(), u.discovered_bluetooth_devices.len())
            });
            if let Some(device) = device {
                log::debug!("[BtD] Try to connect to: {device:?}");
                _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                crate::bluetooth::COMMANDS.signal(BleAction::Connect(device));
            } else if selected == count {
                log::debug!("[BtD] Unpair current device");
                _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                crate::bluetooth::COMMANDS.signal(BleAction::Unpair);
            } else {
                log::debug!("[BtD] Exit");
            }

            state.ui.update(|u| {
                u.menu_scene = None;
                u.selected_bluetooth_item = 0;
                u.selected_config_menu = Some(0);
            });
            return Ok(true);
        }
        Some(MenuScene::ErrorLog) => {
            state.ui.update(|u| {
                if u.selected_error_log_entry.is_some() {
                    #[cfg(feature = "v4")]
                    if u.error_log_entry_stage == Some(ErrorLogEntryStage::Qr) {
                        u.error_log_entry_stage = Some(ErrorLogEntryStage::Details);
                        u.error_log_details_scroll = 0;
                        return;
                    }

                    u.selected_error_log_entry = None;
                    u.error_log_entry_stage = None;
                    u.error_log_details_scroll = 0;
                    return;
                }

                if u.selected_error_log_item == crate::utils::error_log::entry_count() {
                    // "Exit"
                    u.menu_scene = None;
                    u.selected_error_log_item = 0;
                    u.selected_error_log_entry = None;
                    u.error_log_entry_stage = None;
                    u.error_log_details_scroll = 0;
                    u.selected_config_menu = Some(CONFIG_MENU_ERROR_LOG_IDX);
                } else {
                    u.selected_error_log_entry = Some(u.selected_error_log_item);
                    u.error_log_details_scroll = 0;
                    #[cfg(feature = "v3")]
                    {
                        u.error_log_entry_stage = Some(ErrorLogEntryStage::Details);
                    }
                    #[cfg(feature = "v4")]
                    {
                        u.error_log_entry_stage = Some(ErrorLogEntryStage::Qr);
                    }
                }
            });
            return Ok(true);
        }
        #[cfg(feature = "v4")]
        Some(MenuScene::BuzzerVolume) => {
            let current_volume = crate::state::SETTINGS.read(|s| s.buzzer_volume);
            if let Err(e) = state
                .nvs
                .set(&crate::consts::NVS_BUZZER_VOLUME, current_volume)
                .await
            {
                log::error!("Cannot save buzzer volume to NVS: {e:?}");
                crate::log_once!(crate::utils::error_log::codes::NVS_BUZZER_VOLUME_WRITE_FAILED);
            }

            state.ui.update(|u| {
                u.menu_scene = None;
                u.selected_config_menu = Some(CONFIG_MENU_BUZZER_IDX);
            });
            return Ok(true);
        }
        None => {}
    }

    if let Some(sel) = state.ui.read(|u| u.selected_config_menu) {
        submit_config_item(state, sel).await;
        return Ok(true);
    }

    // Device add
    if !state.status.read(|s| s.device_added).unwrap_or(false) {
        let mut sign_key = [0; 4];
        _ = getrandom::getrandom(&mut sign_key);
        let sign_key = u32::from_be_bytes(sign_key) >> 1;

        _ = state.nvs.set(&NVS_SIGN_KEY, sign_key).await;
        crate::state::SETTINGS.write(|s| s.sign_key = sign_key);
        crate::state::SESSION.write(|s| s.trust_server = true);

        if crate::ws::send(crate::structs::TimerPacket {
            tag: None,
            data: crate::structs::TimerPacketInner::Add {
                firmware: crate::version::FIRMWARE.into(),
                sign_key,
            },
        })
        .is_err()
        {
            log::error!("Add packet dropped: ws queue full");
        }

        return Ok(true);
    }

    if state.should_skip_other_actions() {
        return Ok(false);
    }

    let stop_timer = state.solve.update(|s| match s.scene {
        Scene::GroupSelect => {
            match s.possible_groups.get(s.group_selected_idx).cloned() {
                Some(group) => {
                    s.set_group(group);
                    s.scene = if s.solve_time.is_some() {
                        Scene::Finished
                    } else {
                        Scene::CompetitorInfo
                    };
                }
                None => log::error!("Group select: no group at {}", s.group_selected_idx),
            }
            false
        }
        Scene::Timer => true,
        Scene::Finished => {
            s.time_confirmed = true;
            false
        }
        _ => false,
    });

    #[cfg(not(feature = "timer-func"))]
    if stop_timer {
        state.timer_stop_signal.signal(());
    }
    #[cfg(feature = "timer-func")]
    let _ = stop_timer;

    Ok(false)
}

/// Submit on a config menu item.
async fn submit_config_item(state: &GlobalState, sel: usize) {
    let auto_setup_disabled = || {
        if crate::state::SESSION.read(|s| s.auto_setup) {
            return false;
        }
        state.ui.update(|u| u.error_text = Some("AutoSetup Mode Disabled".into()));
        true
    };

    let menu = match sel {
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
            crate::bluetooth::COMMANDS.signal(BleAction::StartScan);
            Some(MenuScene::BtDisplay)
        }
        2 if auto_setup_disabled() => return,
        2 => Some(MenuScene::Signing),
        3 if auto_setup_disabled() => return,
        3 => Some(MenuScene::Unsigning),
        #[cfg(feature = "v4")]
        4 => Some(MenuScene::BuzzerVolume),
        CONFIG_MENU_ERROR_LOG_IDX => Some(MenuScene::ErrorLog),
        _ => None, // Exit
    };

    state.ui.update(|u| {
        if menu == Some(MenuScene::ErrorLog) {
            u.selected_error_log_item = 0;
            u.selected_error_log_entry = None;
            u.error_log_entry_stage = None;
        }
        u.menu_scene = menu;
        u.selected_config_menu = None;
    });
}

async fn inspection_start(state: &GlobalState) -> Result<bool, ()> {
    if state.should_skip_other_actions() || !state.solve.read(|s| s.use_inspection()) {
        return Ok(false);
    }

    // the stackmat must show 0 (no time, or no stackmat reading yet, blocks it)
    if state.timer.try_get() != Some(0) {
        log::warn!("Skipping inspection start because current timer time is not 0");
        return Ok(false);
    }

    Ok(state.solve.update(|s| {
        if s.scene < Scene::Inspection && s.inspection_start.is_none() && s.solve_time.is_none() {
            s.inspection_start = Some(Instant::now());
            s.scene = Scene::Inspection;
            return true;
        }
        false
    }))
}

async fn inspection_hold_stop(state: &GlobalState) -> Result<bool, ()> {
    if state.should_skip_other_actions() {
        return Ok(false);
    }

    Ok(state.solve.update(|s| {
        if s.scene != Scene::Inspection {
            return false;
        }
        s.scene = match s.current_competitor {
            Some(_) => Scene::CompetitorInfo,
            None => Scene::WaitingForCompetitor,
        };
        s.inspection_start = None;
        s.inspection_end = None;
        true
    }))
}

async fn dnf_button(state: &GlobalState) -> Result<bool, ()> {
    if state.should_skip_other_actions() {
        return Ok(false);
    }

    Ok(state.solve.update(|s| {
        if s.scene == Scene::Inspection {
            s.inspection_end = Some(Instant::now());
            s.solve_time = Some(0);
            s.penalty = Some(-1);
            s.time_confirmed = true;
            s.scene = match s.current_competitor {
                Some(_) => Scene::Finished,
                None => Scene::WaitingForCompetitor,
            };
            true
        } else if s.scene == Scene::Finished && !s.time_confirmed {
            s.penalty = Some(if s.penalty == Some(-1) { 0 } else { -1 });
            true
        } else {
            false
        }
    }))
}

async fn penalty_button(state: &GlobalState) -> Result<bool, ()> {
    if state.should_skip_other_actions() {
        return Ok(false);
    }

    state.solve.update(|s| {
        if s.scene == Scene::Finished && !s.time_confirmed {
            let old = s.penalty.unwrap_or(0);
            s.penalty = Some(if old >= 16 || old == -1 { 0 } else { old + 2 });
        }
    });
    Ok(false)
}

async fn submit_reset_competitor(state: &GlobalState) -> Result<bool, ()> {
    if state.should_skip_other_actions() {
        return Ok(false);
    }

    state.timer_stop_signal.signal(());
    state.reset_solve(false).await;
    Ok(false)
}

async fn submit_config_menu(state: &GlobalState) -> Result<bool, ()> {
    state.ui.update(|u| u.selected_config_menu = Some(0));
    Ok(true)
}

async fn delegate_hold(
    triggered: &ButtonTrigger,
    hold_time: u64,
    state: &GlobalState,
) -> Result<bool, ()> {
    match triggered {
        ButtonTrigger::Up => state.solve.update(|s| s.delegate_hold = None),
        ButtonTrigger::HoldTimed(_, _) => {
            if state.should_skip_other_actions() {
                return Ok(false);
            }
            state.solve.update(|s| {
                if !s.delegate_used && s.current_competitor.is_some() && s.solve_group.is_some() {
                    s.delegate_hold = Some((hold_time / 1000).min(3) as u8);
                }
            });
        }
        ButtonTrigger::HoldOnce(_) => {
            if state.should_skip_other_actions() || DELEGATE_PENDING.load(Ordering::Relaxed) {
                return Ok(false);
            }

            #[cfg(not(feature = "e2e"))]
            if !crate::state::SESSION.read(|s| s.trust_server) {
                log::error!("Skipping delegate hold. Server not trusted!");
                return Ok(false);
            }

            let packet = state.solve.update(|s| {
                if s.delegate_used {
                    return None;
                }
                let (Some(current_competitor), Some(group_id)) = (
                    s.current_competitor,
                    s.solve_group.as_ref().map(|g| g.group_id.clone()),
                ) else {
                    log::error!("Delegate hold: competitor or solve_group none!");
                    return None;
                };

                let inspection_time = match s.inspection_end.and(s.inspection_ms()) {
                    Some(ms) if s.use_inspection() => ms as i64,
                    _ => 0,
                };
                let session_id = s
                    .session_id
                    .get_or_insert_with(crate::state::new_session_id)
                    .clone();

                s.delegate_hold = Some(3);
                Some(crate::structs::TimerPacketInner::Solve {
                    solve_time: s.solve_time.unwrap_or(0),
                    penalty: s.penalty.unwrap_or(0) as i64,
                    competitor_id: current_competitor,
                    judge_id: s.current_judge.unwrap_or(0),
                    timestamp: current_epoch(),
                    session_id,
                    delegate: true,
                    inspection_time,
                    group_id,
                    sign_key: crate::state::SETTINGS.read(|s| s.sign_key),
                })
            });

            if let Some(packet) = packet {
                DELEGATE_PENDING.store(true, Ordering::Relaxed);
                DELEGATE_REQUEST.signal(packet);
            }
        }
        _ => {}
    }
    Ok(false)
}
