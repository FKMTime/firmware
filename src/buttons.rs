// qa builds report button edges to the QA flow and never run these actions
#![cfg_attr(feature = "qa", allow(dead_code))]

use crate::bluetooth::BleAction;
use crate::{
    consts::{NVS_BONDING_KEY, NVS_ERROR_LOG, NVS_SIGN_KEY},
    state::{ErrorLogEntryStage, GlobalState, MenuScene, Scene, Ui, current_epoch},
    utils::buttons::{Button, ButtonInputs},
    watchdog::{TaskId, heartbeat},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;
use portable_atomic::{AtomicBool, Ordering};

const INSPECTION_STOP_HOLD_MS: u64 = 1000;
const DNF_HOLD_MS: u64 = 1000;
const RESET_COMPETITOR_HOLD_MS: u64 = 3000;
const CONFIG_MENU_HOLD_MS: u64 = 10000;
const DELEGATE_HOLD_MS: u64 = 3000;

#[cfg(feature = "v3")]
const CONFIG_MENU_ERROR_LOG_IDX: usize = 4;
#[cfg(feature = "v4")]
const CONFIG_MENU_BUZZER_IDX: usize = 4;
#[cfg(feature = "v4")]
const CONFIG_MENU_ERROR_LOG_IDX: usize = 5;

/// Delegate solve waiting to be sent; the reply can take minutes, so it is awaited next to
/// (not inside) the button loop.
static DELEGATE_REQUEST: Signal<CriticalSectionRawMutex, crate::structs::TimerPacketInner> =
    Signal::new();
static DELEGATE_PENDING: AtomicBool = AtomicBool::new(false);

/// The single button being held. Dropped when an action consumes the press, so it gets no
/// further hold/up actions.
struct Press {
    button: Button,
    since: Instant,
    last_ms: u64,
}

#[embassy_executor::task]
pub async fn buttons_task(
    state: GlobalState,
    #[cfg(feature = "v4")] button_inputs: [Input<'static>; 4],
    #[cfg(feature = "v3")] button_input: Input<'static>,
    #[cfg(feature = "v3")] button_reg: adv_shift_registers::wrappers::ShifterValue,
) {
    #[cfg(feature = "v4")]
    let inputs = ButtonInputs::new(button_inputs);
    #[cfg(feature = "v3")]
    let inputs = ButtonInputs::new(button_input, button_reg);

    embassy_futures::join::join(button_loop(state, inputs), delegate_loop(state)).await;
}

async fn button_loop(state: GlobalState, mut inputs: ButtonInputs) {
    let mut press: Option<Press> = None;

    // simulated press: (start, length ms, button index)
    #[cfg(feature = "e2e")]
    let mut e2e_press = (Instant::now(), 0, 0);
    #[cfg(feature = "e2e")]
    let mut send_ack = false;
    #[cfg(feature = "qa")]
    let mut qa_down = None;

    loop {
        heartbeat(TaskId::Buttons);

        #[allow(unused_mut)]
        let mut extra = 0u8;
        #[cfg(feature = "e2e")]
        {
            if let Some((btn_idx, press_ms)) = state.e2e.buttons_sig.try_take() {
                e2e_press = (Instant::now(), press_ms, btn_idx);
                send_ack = true;
                log::debug!("[E2E] Button pressed: {btn_idx} for {press_ms}ms");
            }
            if e2e_press.0.elapsed().as_millis() <= e2e_press.1 {
                extra |= 1 << e2e_press.2;
            }
        }

        if let Some((prev, next)) = inputs.poll(extra) {
            #[cfg(feature = "qa")]
            if prev == 0 {
                crate::qa::send_qa_resp(crate::qa::QaSignal::ButtonDown(next));
                qa_down = Some(next);
            } else if let Some(button) = qa_down.take() {
                crate::qa::send_qa_resp(crate::qa::QaSignal::ButtonUp(button));
            }

            #[cfg(not(feature = "qa"))]
            if prev == 0 {
                press = on_down(&state, Button::from_mask(next));
            } else if let Some(p) = press.take() {
                on_up(&state, p.button).await;
            }

            #[cfg(feature = "e2e")]
            if prev != 0 && send_ack {
                crate::ws::send_test_ack(&state);
                send_ack = false;
            }
        }

        if let Some(p) = press.as_mut() {
            let ms = p.since.elapsed().as_millis();
            let consumed = on_hold(&state, p.button, p.last_ms, ms).await;
            p.last_ms = ms;
            if consumed {
                press = None;
            }
        }

        Timer::after_millis(5).await;
    }
}

/// Returns the press to track, or `None` if the press was consumed (wake-up, list navigation,
/// inspection start) or more than one button is down.
fn on_down(state: &GlobalState, button: Option<Button>) -> Option<Press> {
    if wake_up() {
        return None;
    }

    let button = button?;
    let consumed = match button {
        Button::First => select_step(state, false) || inspection_start(state),
        Button::Fourth => select_step(state, true),
        Button::Second | Button::Third => false,
    };

    (!consumed).then(|| Press {
        button,
        since: Instant::now(),
        last_ms: 0,
    })
}

/// Called every loop tick while `button` is held for `ms` (`prev_ms` at the last tick).
/// Returns true if the press was consumed.
async fn on_hold(state: &GlobalState, button: Button, prev_ms: u64, ms: u64) -> bool {
    let crossed = |threshold: u64| prev_ms <= threshold && ms > threshold;

    match button {
        Button::First if crossed(INSPECTION_STOP_HOLD_MS) => inspection_hold_stop(state),
        Button::Fourth if crossed(DNF_HOLD_MS) => dnf(state),
        Button::Third if crossed(RESET_COMPETITOR_HOLD_MS) => {
            if !state.should_skip_other_actions() {
                state.timer_stop_signal.signal(());
                state.reset_solve(false).await;
            }
            false
        }
        Button::Third if crossed(CONFIG_MENU_HOLD_MS) => {
            // consumed: releasing the button must not also submit
            state.ui.update(|u| u.selected_config_menu = Some(0));
            true
        }
        Button::Second => {
            // hold indicator: once per started second
            if prev_ms == 0 || prev_ms / 1000 != ms / 1000 {
                delegate_progress(state, ms);
            }
            if crossed(DELEGATE_HOLD_MS) {
                delegate_send(state);
            }
            false
        }
        _ => false,
    }
}

async fn on_up(state: &GlobalState, button: Button) {
    if wake_up() {
        return;
    }

    match button {
        Button::Third => submit(state).await,
        Button::Fourth => penalty(state),
        Button::Second => state.solve.update(|s| s.delegate_hold = None),
        Button::First => {}
    }
}

/// Any button wakes the device first; returns true if it was asleep (the press is consumed).
fn wake_up() -> bool {
    if !crate::power::is_sleeping() {
        return false;
    }

    crate::state::notify();
    if crate::power::is_deeper_sleeping() {
        esp_hal::system::software_reset();
    }
    true
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

/// Moves the cursor of whatever list is shown one step left/right. Returns false if no list
/// is shown.
fn select_step(state: &GlobalState, right: bool) -> bool {
    #[cfg(feature = "v4")]
    if state.ui.read(|u| u.menu_scene) == Some(MenuScene::BuzzerVolume) {
        let changed = crate::state::SETTINGS.write(|s| {
            let old = s.buzzer_volume;
            s.buzzer_volume = match right {
                true => (old + 1).min(crate::consts::BUZZER_VOLUME_MAX),
                false => old.saturating_sub(1), // BUZZER_VOLUME_MIN is 0
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
                let items = crate::utils::error_log::entry_count() + 1; // + Exit
                u.selected_error_log_item = wrap_step(u.selected_error_log_item, items, right);
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

    handled
        || state.solve.update(|s| {
            if s.scene != Scene::GroupSelect {
                return false;
            }
            s.group_selected_idx = wrap_step(s.group_selected_idx, s.possible_groups.len(), right);
            true
        })
}

/// Last scroll position of the error log details view.
#[cfg(feature = "v4")]
fn details_max_scroll(u: &Ui) -> usize {
    u.selected_error_log_entry
        .and_then(crate::utils::error_log::entry)
        .map_or(0, |entry| {
            crate::lcd_v4::error_log_entry_details_text(&entry)
                .split('\n')
                .count()
                .saturating_sub(crate::lcd_v4::DETAILS_VISIBLE_LINES)
        })
}

fn close_error_log_entry(u: &mut Ui) {
    u.selected_error_log_entry = None;
    u.error_log_entry_stage = None;
    u.error_log_details_scroll = 0;
}

/// Third button release: dismiss / confirm whatever is shown.
async fn submit(state: &GlobalState) {
    if state.ui.update(|u| u.error_text.take().is_some()) {
        return;
    }

    match state.ui.read(|u| u.menu_scene) {
        Some(MenuScene::Signing | MenuScene::Unsigning) => {
            state.ui.update(|u| {
                u.menu_scene = None;
                u.selected_config_menu = Some(0);
            });
            return;
        }
        Some(MenuScene::BtDisplay) => {
            let (selected, device, count) = state.ui.read(|u| {
                let devices = &u.discovered_bluetooth_devices;
                let sel = u.selected_bluetooth_item;
                (sel, devices.get(sel).cloned(), devices.len())
            });
            if let Some(device) = device {
                log::debug!("[BtD] Try to connect to: {device:?}");
                crate::bluetooth::COMMANDS.signal(BleAction::Connect(device));
            } else if selected == count {
                log::debug!("[BtD] Unpair current device");
                crate::bluetooth::COMMANDS.signal(BleAction::Unpair);
            }

            state.ui.update(|u| {
                u.menu_scene = None;
                u.selected_bluetooth_item = 0;
                u.selected_config_menu = Some(0);
            });
            return;
        }
        Some(MenuScene::ErrorLog) => {
            state.ui.update(|u| {
                if u.selected_error_log_entry.is_some() {
                    // v4: QR → details → back to the list; v3 has no QR stage
                    #[cfg(feature = "v4")]
                    if u.error_log_entry_stage == Some(ErrorLogEntryStage::Qr) {
                        u.error_log_entry_stage = Some(ErrorLogEntryStage::Details);
                        u.error_log_details_scroll = 0;
                        return;
                    }
                    close_error_log_entry(u);
                } else if u.selected_error_log_item == crate::utils::error_log::entry_count() {
                    // "Exit"
                    close_error_log_entry(u);
                    u.menu_scene = None;
                    u.selected_error_log_item = 0;
                    u.selected_config_menu = Some(CONFIG_MENU_ERROR_LOG_IDX);
                } else {
                    u.selected_error_log_entry = Some(u.selected_error_log_item);
                    u.error_log_details_scroll = 0;
                    #[cfg(feature = "v4")]
                    let stage = ErrorLogEntryStage::Qr;
                    #[cfg(feature = "v3")]
                    let stage = ErrorLogEntryStage::Details;
                    u.error_log_entry_stage = Some(stage);
                }
            });
            return;
        }
        #[cfg(feature = "v4")]
        Some(MenuScene::BuzzerVolume) => {
            let volume = crate::state::SETTINGS.read(|s| s.buzzer_volume);
            if let Err(e) = state
                .nvs
                .set(&crate::consts::NVS_BUZZER_VOLUME, volume)
                .await
            {
                log::error!("Cannot save buzzer volume to NVS: {e:?}");
                crate::log_once!(crate::utils::error_log::codes::NVS_BUZZER_VOLUME_WRITE_FAILED);
            }

            state.ui.update(|u| {
                u.menu_scene = None;
                u.selected_config_menu = Some(CONFIG_MENU_BUZZER_IDX);
            });
            return;
        }
        None => {}
    }

    if let Some(sel) = state.ui.read(|u| u.selected_config_menu) {
        submit_config_item(state, sel).await;
        return;
    }

    if !state.status.read(|s| s.device_added).unwrap_or(false) {
        add_device(state).await;
        return;
    }

    if state.should_skip_other_actions() {
        return;
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

    // the pads stop on their own; a stackmat solve can be stopped from here
    if stop_timer && !cfg!(feature = "timer-func") {
        state.timer_stop_signal.signal(());
    }
}

/// Registers this device on the server with a fresh sign key.
async fn add_device(state: &GlobalState) {
    let mut sign_key = [0; 4];
    _ = getrandom::getrandom(&mut sign_key);
    let sign_key = u32::from_be_bytes(sign_key) >> 1;

    _ = state.nvs.set(&NVS_SIGN_KEY, sign_key).await;
    crate::state::SETTINGS.write(|s| s.sign_key = sign_key);
    crate::state::SESSION.write(|s| s.trust_server = true);

    let packet = crate::structs::TimerPacket {
        tag: None,
        data: crate::structs::TimerPacketInner::Add {
            firmware: crate::version::FIRMWARE.into(),
            sign_key,
        },
    };
    if crate::ws::send(packet).is_err() {
        log::error!("Add packet dropped: ws queue full");
    }
}

async fn submit_config_item(state: &GlobalState, sel: usize) {
    let auto_setup_disabled = || {
        let disabled = !crate::state::SESSION.read(|s| s.auto_setup);
        if disabled {
            state
                .ui
                .update(|u| u.error_text = Some("AutoSetup Mode Disabled".into()));
        }
        disabled
    };

    let menu = match sel {
        0 => {
            // reset settings
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
        2 | 3 if auto_setup_disabled() => return,
        2 => Some(MenuScene::Signing),
        3 => Some(MenuScene::Unsigning),
        #[cfg(feature = "v4")]
        CONFIG_MENU_BUZZER_IDX => Some(MenuScene::BuzzerVolume),
        CONFIG_MENU_ERROR_LOG_IDX => Some(MenuScene::ErrorLog),
        _ => None, // Exit
    };

    state.ui.update(|u| {
        if menu == Some(MenuScene::ErrorLog) {
            close_error_log_entry(u);
            u.selected_error_log_item = 0;
        }
        u.menu_scene = menu;
        u.selected_config_menu = None;
    });
}

/// First button down: start inspection. Returns true if it started.
fn inspection_start(state: &GlobalState) -> bool {
    if state.should_skip_other_actions() || !state.solve.read(|s| s.use_inspection()) {
        return false;
    }

    // a stackmat must be reset (showing 0) first; the pads have no display of their own to reset
    #[cfg(not(feature = "timer-func"))]
    if state.timer.try_get() != Some(0) {
        log::warn!("Skipping inspection start because current timer time is not 0");
        return false;
    }

    state.solve.update(|s| {
        if s.scene < Scene::Inspection && s.inspection_start.is_none() && s.solve_time.is_none() {
            s.inspection_start = Some(Instant::now());
            s.scene = Scene::Inspection;
            return true;
        }
        false
    })
}

fn inspection_hold_stop(state: &GlobalState) -> bool {
    if state.should_skip_other_actions() {
        return false;
    }

    state.solve.update(|s| {
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
    })
}

/// Fourth button hold: DNF during inspection, or toggle DNF on an unconfirmed time.
fn dnf(state: &GlobalState) -> bool {
    if state.should_skip_other_actions() {
        return false;
    }

    state.solve.update(|s| {
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
    })
}

/// Fourth button release: cycle the +2 penalty (0 → +2 → … → +16 → 0) on an unconfirmed time.
fn penalty(state: &GlobalState) {
    if state.should_skip_other_actions() {
        return;
    }

    state.solve.update(|s| {
        if s.scene == Scene::Finished && !s.time_confirmed {
            let old = s.penalty.unwrap_or(0);
            s.penalty = Some(if old >= 16 || old == -1 { 0 } else { old + 2 });
        }
    });
}

/// Second button held: show how long (0..=3 s) until the delegate request is sent.
fn delegate_progress(state: &GlobalState, ms: u64) {
    if state.should_skip_other_actions() {
        return;
    }

    state.solve.update(|s| {
        if !s.delegate_used && s.current_competitor.is_some() && s.solve_group.is_some() {
            s.delegate_hold = Some((ms / 1000).min(3) as u8);
        }
    });
}

/// Second button held for 3 s: queue a delegate solve for [`delegate_loop`].
fn delegate_send(state: &GlobalState) {
    if state.should_skip_other_actions() || DELEGATE_PENDING.load(Ordering::Relaxed) {
        return;
    }

    #[cfg(not(feature = "e2e"))]
    if !crate::state::SESSION.read(|s| s.trust_server) {
        log::error!("Skipping delegate hold. Server not trusted!");
        return;
    }

    let sign_key = crate::state::SETTINGS.read(|s| s.sign_key);
    let packet = state.solve.update(|s| {
        if s.delegate_used {
            return None;
        }
        let (Some(competitor_id), Some(group)) = (s.current_competitor, s.solve_group.as_ref())
        else {
            log::error!("Delegate hold: competitor or solve_group none!");
            return None;
        };
        let group_id = group.group_id.clone();

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
            competitor_id,
            judge_id: s.current_judge.unwrap_or(0),
            timestamp: current_epoch(),
            session_id,
            delegate: true,
            inspection_time,
            group_id,
            sign_key,
        })
    });

    if let Some(packet) = packet {
        DELEGATE_PENDING.store(true, Ordering::Relaxed);
        DELEGATE_REQUEST.signal(packet);
    }
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
                    s.penalty = Some(resp.penalty.unwrap_or(s.penalty.unwrap_or(0) as i64) as i8);
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
