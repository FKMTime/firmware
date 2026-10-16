use crate::consts::NVS_SAVED_STATE;
use crate::{
    structs::{
        BleDisplayDevice, DisplayText, MAX_BLE_DEVICES, MAX_POSSIBLE_GROUPS, MessageText,
        PossibleGroup, SessionId,
    },
    utils::{
        bounded::{BoundedVec, CappedVec},
        signaled_mutex::SignaledMutex,
    },
};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    mutex::Mutex,
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::aes::Aes;
use esp_hal_wifimanager::Nvs;
#[cfg(feature = "v4")]
use portable_atomic::AtomicU8;
use portable_atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use serde::{Deserialize, Serialize};

// Lock-free flags shared between tasks. Loads/stores are plain instructions on the C3;
// portable-atomic covers the 64-bit ones (single core, critical section fallback).
static SIGN_KEY: AtomicU32 = AtomicU32::new(0);
static TRUST_SERVER: AtomicBool = AtomicBool::new(false);
static FKM_TOKEN: AtomicI32 = AtomicI32::new(0);
static SECURE_RFID: AtomicBool = AtomicBool::new(false);
static AUTO_SETUP: AtomicBool = AtomicBool::new(false);

static GROUP_LIMIT: AtomicOptU64 = AtomicOptU64::none();

/// 0 = unknown (server didn't send EpochTime yet)
static EPOCH_BASE: AtomicU64 = AtomicU64::new(0);
static SLEEP_STATE: AtomicBool = AtomicBool::new(false);
static DEEPER_SLEEP: AtomicBool = AtomicBool::new(false);
static OTA_STATE: AtomicBool = AtomicBool::new(false);

// Live vars (logger header)
static CURRENT_TIME: AtomicOptU64 = AtomicOptU64::none();
static INSPECTION_TIME: AtomicOptU64 = AtomicOptU64::none();

#[cfg(feature = "v4")]
static BUZZER_VOLUME: AtomicU8 = AtomicU8::new(crate::consts::BUZZER_VOLUME_DEFAULT);

/// `Option<u64>` in one atomic; `u64::MAX` is `None` (never a valid time/limit).
pub struct AtomicOptU64(AtomicU64);

impl AtomicOptU64 {
    pub const fn none() -> Self {
        Self(AtomicU64::new(u64::MAX))
    }

    pub fn get(&self) -> Option<u64> {
        let v = self.0.load(Ordering::Relaxed);
        (v != u64::MAX).then_some(v)
    }

    pub fn set(&self, value: Option<u64>) {
        self.0.store(value.unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

macro_rules! atomic_accessors {
    ($($static:ident: $ty:ty => $get:ident, $set:ident;)*) => {$(
        // e2e/qa builds compile out some readers
        #[inline(always)]
        #[allow(dead_code)]
        pub fn $get() -> $ty {
            $static.load(Ordering::Relaxed)
        }

        #[inline(always)]
        #[allow(dead_code)]
        pub fn $set(value: $ty) {
            $static.store(value, Ordering::Relaxed)
        }
    )*};
}

atomic_accessors! {
    SIGN_KEY: u32 => sign_key, set_sign_key;
    TRUST_SERVER: bool => trust_server, set_trust_server;
    FKM_TOKEN: i32 => fkm_token, set_fkm_token;
    SECURE_RFID: bool => secure_rfid, set_secure_rfid;
    AUTO_SETUP: bool => auto_setup, set_auto_setup;
    SLEEP_STATE: bool => sleep_state, set_sleep_state;
    DEEPER_SLEEP: bool => deeper_sleep_state, set_deeper_sleep_state;
    OTA_STATE: bool => ota_state, set_ota_state;
}

#[cfg(feature = "v4")]
atomic_accessors! {
    BUZZER_VOLUME: u8 => buzzer_volume, set_buzzer_volume;
}

#[inline(always)]
pub fn group_limit() -> Option<u64> {
    GROUP_LIMIT.get()
}

#[inline(always)]
pub fn set_group_limit(limit: Option<u64>) {
    GROUP_LIMIT.set(limit)
}

#[inline(always)]
pub fn current_time() -> Option<u64> {
    CURRENT_TIME.get()
}

#[inline(always)]
pub fn set_current_time(time: Option<u64>) {
    CURRENT_TIME.set(time)
}

#[inline(always)]
pub fn inspection_time() -> Option<u64> {
    INSPECTION_TIME.get()
}

#[inline(always)]
pub fn set_inspection_time(time: Option<u64>) {
    INSPECTION_TIME.set(time)
}

/// A fresh random (v4) uuid, formatted without allocating.
pub fn new_session_id() -> SessionId {
    let mut buf = [0; uuid::fmt::Hyphenated::LENGTH];
    SessionId::truncated(uuid::Uuid::new_v4().hyphenated().encode_lower(&mut buf))
}

/// Sets the epoch from the server's `current_epoch` (seconds).
pub fn set_epoch(current_epoch: u64) {
    EPOCH_BASE.store(
        current_epoch.saturating_sub(Instant::now().as_secs()),
        Ordering::Relaxed,
    );
}

/// True after the server sent its time.
#[inline(always)]
#[allow(dead_code)]
pub fn epoch_known() -> bool {
    EPOCH_BASE.load(Ordering::Relaxed) != 0
}

#[inline(always)]
pub fn current_epoch() -> u64 {
    EPOCH_BASE
        .load(Ordering::Relaxed)
        .saturating_add(Instant::now().as_secs())
}

#[derive(Debug, PartialEq, Clone, Hash)]
#[allow(dead_code)]
pub enum Scene {
    Update,

    /// Waiting for wifi connection
    WifiConnect,

    /// Connect to wifi to setup
    AutoSetupWait,

    /// Waiting for MDNS
    MdnsWait,

    WaitingForCompetitor,
    GroupSelect,
    CompetitorInfo,
    Inspection,
    Timer,
    Finished,
}

#[derive(Debug, PartialEq, Clone, Hash)]
pub enum MenuScene {
    Signing,
    Unsigning,
    BtDisplay,
    ErrorLog,
    #[cfg(feature = "v4")]
    BuzzerVolume,
}

#[derive(Debug, PartialEq, Clone, Hash)]
pub enum ErrorLogEntryStage {
    #[cfg(feature = "v4")]
    Qr,
    // Shared stage used on both variants for post-selection detail view.
    Details,
}

impl Scene {
    pub fn can_be_lcd_overwritten(&self) -> bool {
        match self {
            Scene::Update => false,
            Scene::WifiConnect => false,
            Scene::AutoSetupWait => false,
            Scene::MdnsWait => false,
            Scene::WaitingForCompetitor => true,
            Scene::GroupSelect => true,
            Scene::CompetitorInfo => true,
            Scene::Inspection => false,
            Scene::Timer => false,
            Scene::Finished => false,
        }
    }

    pub fn to_index(&self) -> usize {
        match self {
            Scene::Update => 0,
            Scene::WifiConnect => 1,
            Scene::AutoSetupWait => 2,
            Scene::MdnsWait => 3,
            Scene::WaitingForCompetitor => 4,
            Scene::GroupSelect => 5,
            Scene::CompetitorInfo => 6,
            Scene::Inspection => 7,
            Scene::Timer => 8,
            Scene::Finished => 9,
        }
    }

    pub fn can_sleep(&self) -> bool {
        !matches!(
            self,
            Scene::Update | Scene::WifiConnect | Scene::AutoSetupWait
        )
    }
}

impl PartialOrd for Scene {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.to_index().cmp(&other.to_index()))
    }
}

#[cfg(feature = "e2e")]
#[derive(Default)]
pub struct End2End {
    pub buttons_sig: Signal<CriticalSectionRawMutex, (usize, u64)>,
    pub card_scan_sig: Signal<CriticalSectionRawMutex, u128>,
    pub stackmat_sig:
        Signal<CriticalSectionRawMutex, (crate::utils::stackmat::StackmatTimerState, u64)>,
}

#[cfg(feature = "e2e")]
impl End2End {
    pub fn new() -> Self {
        End2End {
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BleAction {
    Connect(BleDisplayDevice),
    StartScan,
    Unpair,
}

pub type GlobalState = &'static GlobalStateInner;
pub struct GlobalStateInner {
    pub state: SignaledMutex<CriticalSectionRawMutex, SignaledGlobalStateInner>,
    pub timer_signal: Signal<NoopRawMutex, u64>,
    pub timer_stop_signal: Signal<NoopRawMutex, ()>,
    pub bt_display_signal: Signal<NoopRawMutex, u64>,
    pub update_progress: Signal<CriticalSectionRawMutex, u8>,
    pub sign_unsign_progress: Signal<CriticalSectionRawMutex, bool>,
    pub ble_sig: crate::bluetooth::BleCommands,
    pub show_battery: Signal<CriticalSectionRawMutex, u8>,
    #[cfg(feature = "v4")]
    pub buzzer_sound_test: Signal<CriticalSectionRawMutex, ()>,

    pub nvs: &'static Nvs,
    pub aes: Mutex<NoopRawMutex, Aes<'static>>,

    #[cfg(feature = "e2e")]
    pub e2e: End2End,
}

impl GlobalStateInner {
    pub fn new(nvs: &'static Nvs, aes: esp_hal::peripherals::AES<'static>) -> Self {
        Self {
            state: SignaledMutex::new(SignaledGlobalStateInner::new()),
            timer_signal: Signal::new(),
            timer_stop_signal: Signal::new(),
            bt_display_signal: Signal::new(),
            update_progress: Signal::new(),
            sign_unsign_progress: Signal::new(),
            ble_sig: crate::bluetooth::BleCommands::new(),
            show_battery: Signal::new(),
            #[cfg(feature = "v4")]
            buzzer_sound_test: Signal::new(),

            nvs,
            aes: Mutex::new(Aes::new(aes)),

            #[cfg(feature = "e2e")]
            e2e: End2End::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignaledGlobalStateInner {
    pub scene: Scene,
    pub menu_scene: Option<MenuScene>,

    pub inspection_start: Option<Instant>,
    pub inspection_end: Option<Instant>,
    pub solve_time: Option<u64>,
    pub penalty: Option<i8>,
    pub session_id: Option<SessionId>,
    pub time_confirmed: bool,
    pub solve_group: Option<PossibleGroup>,

    pub error_text: Option<MessageText>,

    pub possible_groups: CappedVec<PossibleGroup, MAX_POSSIBLE_GROUPS>,
    pub group_selected_idx: usize,

    pub selected_config_menu: Option<usize>,
    pub selected_error_log_item: usize,
    pub selected_error_log_entry: Option<usize>,
    pub error_log_entry_stage: Option<ErrorLogEntryStage>,
    pub error_log_details_scroll: usize,

    pub discovered_bluetooth_devices: BoundedVec<BleDisplayDevice, MAX_BLE_DEVICES>,
    pub selected_bluetooth_item: usize,

    pub sound_enabled: bool,
    pub device_added: Option<bool>,
    pub server_connected: Option<bool>,
    pub wifi_connected: Option<bool>,
    pub stackmat_connected: Option<bool>,

    pub current_competitor: Option<u64>,
    pub current_judge: Option<u64>,
    pub competitor_display: Option<DisplayText>,

    pub delegate_used: bool,
    pub delegate_hold: Option<u8>,

    #[cfg(feature = "v4")]
    pub battery_status: (u8, bool),

    #[cfg(feature = "bat_dev_lcd")]
    pub current_bat_read: Option<f32>,

    #[cfg(feature = "bat_dev_lcd")]
    pub avg_bat_read: Option<f32>,

    pub custom_message: Option<(DisplayText, DisplayText)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGlobalState {
    pub inspection_time: Option<u64>,
    pub solve_time: u64,
    pub penalty: i8,
    pub session_id: SessionId,
    pub current_competitor: u64,
    pub solve_epoch: u64,
    pub solve_group: PossibleGroup,
}

impl SignaledGlobalStateInner {
    pub fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,
            menu_scene: None,

            inspection_start: None,
            inspection_end: None,
            solve_time: None,
            penalty: None,
            session_id: None,
            time_confirmed: false,
            solve_group: None,

            error_text: None,
            possible_groups: CappedVec::default(),
            group_selected_idx: 0,
            selected_config_menu: None,
            selected_error_log_item: 0,
            selected_error_log_entry: None,
            error_log_entry_stage: None,
            error_log_details_scroll: 0,
            selected_bluetooth_item: 0,
            discovered_bluetooth_devices: BoundedVec::new(),

            sound_enabled: true,
            device_added: None,
            server_connected: None,
            wifi_connected: None,
            stackmat_connected: None,
            current_competitor: None,
            current_judge: None,
            competitor_display: None,

            delegate_used: false,
            delegate_hold: None,

            #[cfg(feature = "v4")]
            battery_status: (0, false),

            #[cfg(feature = "bat_dev_lcd")]
            current_bat_read: None,

            #[cfg(feature = "bat_dev_lcd")]
            avg_bat_read: None,

            custom_message: None,
        }
    }

    pub fn should_skip_other_actions(&self) -> bool {
        if self.error_text.is_some() {
            return true;
        }

        if self.scene.can_be_lcd_overwritten() {
            if self.server_connected == Some(false) {
                return true;
            }

            if self.stackmat_connected == Some(false) {
                return true;
            }
        }

        if self.scene <= Scene::MdnsWait {
            return true;
        }

        false
    }

    pub async fn reset_solve_state(&mut self, save_nvs: Option<&Nvs>) {
        set_group_limit(None);
        set_current_time(None);
        set_inspection_time(None);
        self.solve_time = None;
        self.penalty = None;
        self.inspection_start = None;
        self.inspection_end = None;
        self.current_competitor = None;
        self.current_judge = None;
        self.competitor_display = None;
        self.session_id = None;
        self.time_confirmed = false;
        self.scene = Scene::WaitingForCompetitor;
        self.delegate_used = false;
        self.inspection_start = None;
        self.inspection_end = None;
        self.solve_group = None;
        self.possible_groups.clear();
        self.group_selected_idx = 0;

        if let Some(nvs) = save_nvs {
            SavedGlobalState::clear_saved_global_state(nvs).await;
        }

        crate::translations::restore_default_locale();
    }

    #[allow(dead_code)]
    pub async fn hard_state_reset(&mut self) {
        set_group_limit(None);
        set_current_time(None);
        set_inspection_time(None);
        self.scene = Scene::WaitingForCompetitor;
        self.inspection_start = None;
        self.inspection_end = None;
        self.solve_time = None;
        self.penalty = None;
        self.session_id = None;
        self.time_confirmed = false;
        self.solve_group = None;
        self.error_text = None;
        self.possible_groups.clear();
        self.group_selected_idx = 0;
        self.current_competitor = None;
        self.current_judge = None;
        self.competitor_display = None;
        self.delegate_used = false;
        self.delegate_hold = None;
        self.custom_message = None;
    }

    pub fn to_saved_global_state(&self) -> Option<SavedGlobalState> {
        log::debug!("TO_SAVED_STATE: {self:?}");

        Some(SavedGlobalState {
            session_id: self.session_id.clone()?,
            current_competitor: self.current_competitor?,
            penalty: self.penalty.unwrap_or(0),
            solve_time: self.solve_time?,
            inspection_time: self.inspection_end.map(|e| {
                (e.saturating_duration_since(self.inspection_start.unwrap_or(Instant::now())))
                    .as_millis()
            }),
            solve_epoch: current_epoch(),
            solve_group: self.solve_group.clone()?,
        })
    }

    pub fn parse_saved_state(&mut self, saved: SavedGlobalState) {
        log::warn!("Parsed saved state: {saved:?}");

        self.session_id = Some(saved.session_id);
        self.penalty = Some(saved.penalty);
        self.solve_time = Some(saved.solve_time);
        self.current_competitor = Some(saved.current_competitor);
        self.solve_group = Some(saved.solve_group);

        if let Some(inspection_time) = saved.inspection_time {
            let now = Instant::now();
            self.inspection_end = now.checked_add(Duration::from_millis(inspection_time));
            self.inspection_start = Some(now);
        }

        if saved.solve_time > 0 {
            self.scene = Scene::Finished;
        }
    }

    pub fn use_inspection(&self) -> bool {
        match self.solve_group.as_ref().map(|r| r.use_inspection) {
            Some(true) | None => true,
            Some(false) => false,
        }
    }

    #[cfg(feature = "e2e")]
    pub fn snapshot_data(&self) -> crate::structs::SnapshotData {
        let inspection_time = self
            .inspection_end
            .zip(self.inspection_start)
            .map(|(end, start)| end.saturating_duration_since(start).as_millis());

        crate::structs::SnapshotData {
            scene: self.scene.to_index(),
            inspection_time,
            penalty: self.penalty,
            solve_time: self.solve_time,
            current_judge: self.current_judge,
            current_competitor: self.current_competitor,
            group_selected_idx: self.group_selected_idx,
            time_confirmed: self.time_confirmed,
            possible_groups: self.possible_groups.len(),
        }
    }
}

#[cfg(not(feature = "e2e"))]
impl SavedGlobalState {
    pub async fn from_nvs(nvs: &Nvs) -> Option<Self> {
        while !epoch_known() {
            Timer::after_millis(5).await;
        }

        let res = nvs.get::<alloc::vec::Vec<u8>>(&NVS_SAVED_STATE).await.ok()?;
        let res: SavedGlobalState = serde_json::from_slice(&res).ok()?;

        // 6hours
        if current_epoch().saturating_sub(res.solve_epoch) > 6 * 60 * 60 {
            log::error!("REMOVE SOLVE: {:?} {:?}", current_epoch(), res.solve_epoch);
            return None;
        }

        Some(res)
    }

    pub async fn to_nvs(&self, nvs: &Nvs) {

        let res = serde_json::to_vec(&self);
        if let Ok(vec) = res {
            // set replaces the old value atomically (deleting first could lose it on power loss)
            let res = nvs.set(&NVS_SAVED_STATE, vec.as_slice()).await;
            if let Err(e) = res {
                log::error!("{e:?} Faile to write to nvs! (SAVED_STATE {})", vec.len());
                crate::log_once!(crate::utils::error_log::codes::NVS_SAVED_STATE_WRITE_FAILED);
            }
        }
    }

    pub async fn clear_saved_global_state(nvs: &Nvs) {

        let res = nvs.delete(&NVS_SAVED_STATE).await;
        if let Err(e) = res {
            log::error!("{e:?} Faile to delete nvs key! (SAVED_STATE)",);
            crate::log_once!(crate::utils::error_log::codes::NVS_SAVED_STATE_DELETE_FAILED);
        }
    }
}

#[cfg(feature = "e2e")]
impl SavedGlobalState {
    pub async fn from_nvs(_nvs: &Nvs) -> Option<Self> {
        None
    }

    pub async fn to_nvs(&self, _nvs: &Nvs) {}

    pub async fn clear_saved_global_state(_nvs: &Nvs) {}
}

impl core::hash::Hash for SignaledGlobalStateInner {
    /// Fields that trigger a redraw when changed (battery_status intentionally excluded).
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.scene.hash(h);
        self.menu_scene.hash(h);
        self.inspection_start.map(|i| i.as_ticks()).hash(h);
        self.inspection_end.map(|i| i.as_ticks()).hash(h);
        self.solve_time.hash(h);
        self.penalty.hash(h);
        self.session_id.hash(h);
        self.time_confirmed.hash(h);
        self.solve_group.hash(h);
        self.error_text.hash(h);
        self.possible_groups.hash(h);
        self.group_selected_idx.hash(h);
        self.selected_config_menu.hash(h);
        self.selected_error_log_item.hash(h);
        self.selected_error_log_entry.hash(h);
        self.error_log_entry_stage.hash(h);
        self.error_log_details_scroll.hash(h);
        self.discovered_bluetooth_devices.hash(h);
        self.selected_bluetooth_item.hash(h);
        self.device_added.hash(h);
        self.server_connected.hash(h);
        self.wifi_connected.hash(h);
        self.stackmat_connected.hash(h);
        self.current_competitor.hash(h);
        self.current_judge.hash(h);
        self.competitor_display.hash(h);
        self.delegate_used.hash(h);
        self.delegate_hold.hash(h);
        self.custom_message.hash(h);

        #[cfg(feature = "bat_dev_lcd")]
        {
            self.current_bat_read.map(f32::to_bits).hash(h);
            self.avg_bat_read.map(f32::to_bits).hash(h);
        }
    }
}
