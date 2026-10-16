//! What the display shows, decided once for both hardware revisions; the per-revision
//! renderers (`lcd_v3`, `lcd_v4`) only draw it.

use crate::{
    consts::INSPECTION_TIME_PLUS2,
    state::{ErrorLogEntryStage, MenuScene, Scene, Solve, Status, Ui},
    translations::TranslationKey,
};
use embassy_time::Instant;

pub enum Screen<'a> {
    Error(&'a str),
    Custom(&'a str, &'a str),
    ConfigMenu(usize),
    #[cfg(feature = "v4")]
    BuzzerVolume,
    ErrorLogList(usize),
    ErrorLogEntry {
        idx: usize,
        #[cfg_attr(feature = "v3", allow(dead_code))]
        stage: Option<&'a ErrorLogEntryStage>,
        #[cfg_attr(feature = "v3", allow(dead_code))]
        scroll: usize,
    },
    /// `true` = signing, `false` = unsigning
    Signing(bool),
    BtDevices(usize),
    WifiLost,
    ServerLost,
    DeviceNotAdded,
    StackmatLost,
    /// seconds remaining until the delegate is called (0 = waiting for the delegate)
    Delegate(u8),
    WifiConnect,
    AutoSetup,
    MdnsWait,
    /// selected group name (empty if the selection is out of range)
    GroupSelect(&'a str),
    WaitingForCompetitor(Option<u64>),
    CompetitorInfo,
    Inspection(Instant),
    Timer,
    Finished(Finished),
    Update,
}

pub struct Finished {
    pub time: u64,
    pub penalty: i8,
    /// inspection time, only when it is worth showing (over the +2 threshold)
    pub inspection: Option<u64>,
    /// translation key of the status line
    pub status: Option<usize>,
}

/// The priority chain: error > custom message > config menu > menu scene > connection
/// overwrites > delegate > scene.
pub fn screen<'a>(s: &'a Solve, ui: &'a Ui, status: &Status) -> Screen<'a> {
    if let Some(ref error) = ui.error_text {
        return Screen::Error(error);
    }
    if let Some((ref l1, ref l2)) = ui.custom_message {
        return Screen::Custom(l1, l2);
    }
    if let Some(sel) = ui.selected_config_menu {
        return Screen::ConfigMenu(sel);
    }

    match ui.menu_scene {
        Some(MenuScene::Signing | MenuScene::Unsigning) => {
            return Screen::Signing(ui.menu_scene == Some(MenuScene::Signing));
        }
        Some(MenuScene::BtDisplay) => return Screen::BtDevices(ui.selected_bluetooth_item),
        Some(MenuScene::ErrorLog) => {
            return match ui.selected_error_log_entry {
                Some(idx) => Screen::ErrorLogEntry {
                    idx,
                    stage: ui.error_log_entry_stage.as_ref(),
                    scroll: ui.error_log_details_scroll,
                },
                None => Screen::ErrorLogList(ui.selected_error_log_item),
            };
        }
        #[cfg(feature = "v4")]
        Some(MenuScene::BuzzerVolume) => return Screen::BuzzerVolume,
        None => {}
    }

    if s.scene.can_be_lcd_overwritten() {
        if status.server_connected == Some(false) {
            return if status.wifi_connected == Some(false) {
                Screen::WifiLost
            } else {
                Screen::ServerLost
            };
        }
        if status.device_added == Some(false) {
            return Screen::DeviceNotAdded;
        }
        if status.stackmat_connected == Some(false) {
            return Screen::StackmatLost;
        }
    }

    if let Some(held) = s.delegate_hold {
        return Screen::Delegate(3u8.saturating_sub(held));
    }

    match s.scene {
        Scene::WifiConnect => Screen::WifiConnect,
        Scene::AutoSetupWait => Screen::AutoSetup,
        Scene::MdnsWait => Screen::MdnsWait,
        Scene::GroupSelect => Screen::GroupSelect(
            s.possible_groups
                .get(s.group_selected_idx)
                .map_or("", |g| g.name.as_str()),
        ),
        Scene::WaitingForCompetitor => Screen::WaitingForCompetitor(s.solve_time),
        Scene::CompetitorInfo => Screen::CompetitorInfo,
        Scene::Inspection => Screen::Inspection(s.inspection_start.unwrap_or_else(Instant::now)),
        Scene::Timer => Screen::Timer,
        Scene::Finished => {
            let inspection = match (s.inspection_start, s.inspection_end) {
                (Some(start), Some(end)) => Some(end.saturating_duration_since(start).as_millis()),
                _ => None,
            };

            let status = if !s.time_confirmed {
                Some(TranslationKey::CONFIRM_TIME)
            } else if s.current_judge.is_none() {
                Some(TranslationKey::SCAN_JUDGE_CARD)
            } else if s.current_competitor.is_some() {
                Some(TranslationKey::SCAN_COMPETITOR_CARD)
            } else {
                None
            };

            Screen::Finished(Finished {
                time: s.solve_time.unwrap_or(0),
                penalty: s.penalty.unwrap_or(0),
                inspection: inspection
                    .filter(|&t| s.use_inspection() && t > INSPECTION_TIME_PLUS2),
                status,
            })
        }
        Scene::Update => Screen::Update,
    }
}
