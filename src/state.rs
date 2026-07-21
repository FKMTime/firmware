use crate::consts::NVS_SAVED_STATE;
use crate::{
    structs::{BleDisplayDevice, PossibleGroup},
    utils::error_log::ErrorLogEntry,
    utils::signaled_mutex::SignaledMutex,
};
use alloc::{rc::Rc, string::String, vec::Vec};
use core::cell::Cell;
use embassy_sync::{
    blocking_mutex::{
        Mutex as BlockingMutex,
        raw::{CriticalSectionRawMutex, NoopRawMutex},
    },
    mutex::Mutex,
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal_wifimanager::Nvs;
#[cfg(feature = "v4")]
use portable_atomic::AtomicU8;
use portable_atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use serde::{Deserialize, Serialize};

/// 256-bit device secret (HMAC key). Zero = unset. Behind a sync critical-section
/// lock (HMAC runs in a sync context); access via `with_sign_key` so the key never
/// escapes the lock.
pub static SIGN_KEY: BlockingMutex<CriticalSectionRawMutex, Cell<[u8; 32]>> =
    BlockingMutex::new(Cell::new([0u8; 32]));
/// True when TLS pin matches (or was just set on Add for this session's peer).
pub static TRUST_SERVER: AtomicBool = AtomicBool::new(false);
/// SHA-256 of the last successful TLS peer end-entity cert (for pin-on-Add);
/// `None` until a handshake captures one.
pub static LAST_PEER_CERT_FP: BlockingMutex<CriticalSectionRawMutex, Cell<Option<[u8; 32]>>> =
    BlockingMutex::new(Cell::new(None));
/// Pinned TLS fingerprint from NVS (`None` = not pinned yet).
pub static TLS_PIN: BlockingMutex<CriticalSectionRawMutex, Cell<Option<[u8; 32]>>> =
    BlockingMutex::new(Cell::new(None));
pub static FKM_TOKEN: AtomicI32 = AtomicI32::new(0);
pub static SECURE_RFID: AtomicBool = AtomicBool::new(false);
pub static AUTO_SETUP: AtomicBool = AtomicBool::new(false);

#[inline(always)]
pub fn trust_server() -> bool {
    TRUST_SERVER.load(Ordering::Relaxed)
}
#[inline(always)]
pub fn set_trust_server(v: bool) {
    TRUST_SERVER.store(v, Ordering::Relaxed);
}
#[inline(always)]
pub fn secure_rfid() -> bool {
    SECURE_RFID.load(Ordering::Relaxed)
}
#[inline(always)]
pub fn set_secure_rfid(v: bool) {
    SECURE_RFID.store(v, Ordering::Relaxed);
}
#[inline(always)]
pub fn auto_setup() -> bool {
    AUTO_SETUP.load(Ordering::Relaxed)
}
#[inline(always)]
pub fn set_auto_setup(v: bool) {
    AUTO_SETUP.store(v, Ordering::Relaxed);
}
#[inline(always)]
pub fn fkm_token() -> i32 {
    FKM_TOKEN.load(Ordering::Relaxed)
}
#[inline(always)]
pub fn set_fkm_token(v: i32) {
    FKM_TOKEN.store(v, Ordering::Relaxed);
}

/// Run `f` with a borrow of the device secret under a short critical section,
/// so the key never escapes the lock.
#[inline]
pub fn with_sign_key<R>(f: impl FnOnce(&[u8; 32]) -> R) -> R {
    SIGN_KEY.lock(|cell| {
        let key = cell.get();
        f(&key)
    })
}

#[inline]
pub fn sign_key_is_set() -> bool {
    with_sign_key(|key| key.iter().any(|&b| b != 0))
}

/// Hex (lowercase) of SIGN_KEY for Add / wire format.
pub fn sign_key_hex() -> alloc::string::String {
    with_sign_key(|key| {
        let mut out = alloc::string::String::with_capacity(64);
        for &b in key {
            use core::fmt::Write;
            let _ = write!(out, "{b:02x}");
        }
        out
    })
}

#[inline]
pub fn set_sign_key(key: [u8; 32]) {
    SIGN_KEY.lock(|cell| cell.set(key));
}

/// Run `f` with a borrow of the pinned TLS fingerprint (`None` if not pinned).
#[inline]
pub fn with_tls_pin<R>(f: impl FnOnce(&Option<[u8; 32]>) -> R) -> R {
    TLS_PIN.lock(|cell| {
        let pin = cell.get();
        f(&pin)
    })
}

#[inline]
pub fn has_tls_pin() -> bool {
    with_tls_pin(|pin| pin.is_some())
}

#[inline]
pub fn set_tls_pin(pin: [u8; 32]) {
    TLS_PIN.lock(|cell| cell.set(Some(pin)));
}

#[inline]
pub fn last_peer_cert_fp() -> Option<[u8; 32]> {
    LAST_PEER_CERT_FP.lock(|cell| cell.get())
}

#[inline]
pub fn set_last_peer_cert_fp(fp: [u8; 32]) {
    LAST_PEER_CERT_FP.lock(|cell| cell.set(Some(fp)));
}

pub static EPOCH_BASE: AtomicU64 = AtomicU64::new(0);
pub static SLEEP_STATE: AtomicBool = AtomicBool::new(false);
pub static DEEPER_SLEEP: AtomicBool = AtomicBool::new(false);
pub static OTA_STATE: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "v4")]
pub static BUZZER_VOLUME: AtomicU8 = AtomicU8::new(crate::consts::BUZZER_VOLUME_DEFAULT);

#[cfg(feature = "v4")]
#[inline(always)]
pub fn buzzer_volume() -> u8 {
    BUZZER_VOLUME.load(Ordering::Relaxed)
}

#[cfg(feature = "v4")]
#[inline(always)]
pub fn set_buzzer_volume(volume: u8) {
    BUZZER_VOLUME.store(volume, Ordering::Relaxed);
}

#[inline(always)]
pub fn current_epoch() -> u64 {
    EPOCH_BASE.load(Ordering::Relaxed) + Instant::now().as_secs()
}

#[inline(always)]
pub fn sleep_state() -> bool {
    SLEEP_STATE.load(Ordering::Relaxed)
}

#[inline(always)]
pub fn deeper_sleep_state() -> bool {
    DEEPER_SLEEP.load(Ordering::Relaxed)
}

#[inline(always)]
pub fn ota_state() -> bool {
    OTA_STATE.load(Ordering::Relaxed)
}

#[derive(Debug, PartialEq, Clone)]
#[allow(dead_code)]
pub enum Scene {
    Update,
    WifiConnect,
    AutoSetupWait,
    MdnsWait,
    WaitingForCompetitor,
    GroupSelect,
    CompetitorInfo,
    Inspection,
    Timer,
    Finished,
}

#[derive(Debug, PartialEq, Clone)]
pub enum MenuScene {
    Signing,
    Unsigning,
    BtDisplay,
    ErrorLog,
    #[cfg(feature = "v4")]
    BuzzerVolume,
}

#[derive(Debug, PartialEq, Clone)]
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

#[derive(Debug, Clone, Default)]
pub struct BatteryState {
    #[cfg(feature = "v4")]
    pub battery_status: (u8, bool),
    #[cfg(feature = "bat_dev_lcd")]
    pub current_bat_read: Option<f32>,
    #[cfg(feature = "bat_dev_lcd")]
    pub avg_bat_read: Option<f32>,
}

pub type GlobalState = Rc<GlobalStateInner>;
pub struct GlobalStateInner {
    pub state: SignaledMutex<CriticalSectionRawMutex, SignaledGlobalStateInner>,
    /// Shared render notification: every sub-state pokes it on change so the LCD
    /// (its only waiter) re-renders.
    pub render: Rc<Signal<CriticalSectionRawMutex, ()>>,
    // Unused only in the v3-without-bat_dev_lcd config, where BatteryState has no fields.
    #[allow(dead_code)]
    pub battery: Mutex<NoopRawMutex, BatteryState>,
    pub timer_signal: Signal<NoopRawMutex, u64>,
    pub timer_stop_signal: Signal<NoopRawMutex, ()>,
    pub bt_display_signal: Signal<NoopRawMutex, u64>,
    pub update_progress: Signal<CriticalSectionRawMutex, u8>,
    pub sign_unsign_progress: Signal<CriticalSectionRawMutex, bool>,
    pub ble_sig: Signal<CriticalSectionRawMutex, BleAction>,
    pub show_battery: Signal<CriticalSectionRawMutex, u8>,
    #[cfg(feature = "v4")]
    pub buzzer_sound_test: Signal<CriticalSectionRawMutex, ()>,

    pub nvs: Nvs,

    #[cfg(feature = "e2e")]
    pub e2e: End2End,
}

impl GlobalStateInner {
    pub fn new(nvs: &Nvs) -> Self {
        let render: Rc<Signal<CriticalSectionRawMutex, ()>> = Rc::new(Signal::new());
        Self {
            state: SignaledMutex::new(SignaledGlobalStateInner::new(), render.clone()),
            render,
            battery: Mutex::new(BatteryState::default()),
            timer_signal: Signal::new(),
            timer_stop_signal: Signal::new(),
            bt_display_signal: Signal::new(),
            update_progress: Signal::new(),
            sign_unsign_progress: Signal::new(),
            ble_sig: Signal::new(),
            show_battery: Signal::new(),
            #[cfg(feature = "v4")]
            buzzer_sound_test: Signal::new(),

            nvs: nvs.clone(),

            #[cfg(feature = "e2e")]
            e2e: End2End::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnState {
    pub sound_enabled: bool,
    pub device_added: Option<bool>,
    pub server_connected: Option<bool>,
    pub wifi_connected: Option<bool>,
    pub stackmat_connected: Option<bool>,
}

impl ConnState {
    pub fn new() -> Self {
        Self {
            sound_enabled: true,
            device_added: None,
            server_connected: None,
            wifi_connected: None,
            stackmat_connected: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SolveState {
    pub scene: Scene,
    pub inspection_start: Option<Instant>,
    pub inspection_end: Option<Instant>,
    pub solve_time: Option<u64>,
    pub penalty: Option<i8>,
    pub session_id: Option<String>,
    pub time_confirmed: bool,
    pub solve_group: Option<PossibleGroup>,
    pub possible_groups: Vec<PossibleGroup>,
    pub group_selected_idx: usize,
    pub current_competitor: Option<u64>,
    pub current_judge: Option<u64>,
    pub competitor_display: Option<String>,
    pub delegate_used: bool,
    pub delegate_hold: Option<u8>,
}

impl SolveState {
    pub fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,
            inspection_start: None,
            inspection_end: None,
            solve_time: None,
            penalty: None,
            session_id: None,
            time_confirmed: false,
            solve_group: None,
            possible_groups: Vec::new(),
            group_selected_idx: 0,
            current_competitor: None,
            current_judge: None,
            competitor_display: None,
            delegate_used: false,
            delegate_hold: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub menu_scene: Option<MenuScene>,
    pub selected_config_menu: Option<usize>,
    pub error_log_entries: Vec<ErrorLogEntry>,
    pub selected_error_log_item: usize,
    pub selected_error_log_entry: Option<usize>,
    pub error_log_entry_stage: Option<ErrorLogEntryStage>,
    pub error_log_details_scroll: usize,
    pub discovered_bluetooth_devices: Vec<BleDisplayDevice>,
    pub selected_bluetooth_item: usize,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            menu_scene: None,
            selected_config_menu: None,
            error_log_entries: Vec::new(),
            selected_error_log_item: 0,
            selected_error_log_entry: None,
            error_log_entry_stage: None,
            error_log_details_scroll: 0,
            discovered_bluetooth_devices: Vec::new(),
            selected_bluetooth_item: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MsgState {
    pub error_text: Option<String>,
    pub custom_message: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct SignaledGlobalStateInner {
    pub solve: SolveState,
    pub ui: UiState,
    pub conn: ConnState,
    pub msg: MsgState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGlobalState {
    pub inspection_time: Option<u64>,
    pub solve_time: u64,
    pub penalty: i8,
    pub session_id: String,
    pub current_competitor: u64,
    pub solve_epoch: u64,
    pub solve_group: PossibleGroup,
}

impl SignaledGlobalStateInner {
    pub fn new() -> Self {
        Self {
            solve: SolveState::new(),
            ui: UiState::new(),
            conn: ConnState::new(),
            msg: MsgState::default(),
        }
    }

    pub fn should_skip_other_actions(&self) -> bool {
        if self.msg.error_text.is_some() {
            return true;
        }

        if self.solve.scene.can_be_lcd_overwritten() {
            if self.conn.server_connected == Some(false) {
                return true;
            }

            if self.conn.stackmat_connected == Some(false) {
                return true;
            }
        }

        if self.solve.scene <= Scene::MdnsWait {
            return true;
        }

        false
    }

    pub fn reset_solve_state(&mut self) {
        self.solve.solve_time = None;
        self.solve.penalty = None;
        self.solve.inspection_start = None;
        self.solve.inspection_end = None;
        self.solve.current_competitor = None;
        self.solve.current_judge = None;
        self.solve.competitor_display = None;
        self.solve.session_id = None;
        self.solve.time_confirmed = false;
        self.solve.scene = Scene::WaitingForCompetitor;
        self.solve.delegate_used = false;
        self.solve.solve_group = None;
        self.solve.possible_groups.clear();
        self.solve.group_selected_idx = 0;

        crate::translations::restore_default_locale();
    }

    #[allow(dead_code)]
    pub fn hard_state_reset(&mut self) {
        self.solve.scene = Scene::WaitingForCompetitor;
        self.solve.inspection_start = None;
        self.solve.inspection_end = None;
        self.solve.solve_time = None;
        self.solve.penalty = None;
        self.solve.session_id = None;
        self.solve.time_confirmed = false;
        self.solve.solve_group = None;
        self.msg.error_text = None;
        self.solve.possible_groups.clear();
        self.solve.group_selected_idx = 0;
        self.solve.current_competitor = None;
        self.solve.current_judge = None;
        self.solve.competitor_display = None;
        self.solve.delegate_used = false;
        self.solve.delegate_hold = None;
        self.msg.custom_message = None;
    }

    pub fn to_saved_global_state(&self) -> Option<SavedGlobalState> {
        log::debug!("TO_SAVED_STATE: {self:?}");

        Some(SavedGlobalState {
            session_id: self.solve.session_id.clone()?,
            current_competitor: self.solve.current_competitor?,
            penalty: self.solve.penalty.unwrap_or(0),
            solve_time: self.solve.solve_time?,
            inspection_time: self.solve.inspection_end.map(|e| {
                (e.saturating_duration_since(self.solve.inspection_start.unwrap_or(Instant::now())))
                    .as_millis()
            }),
            solve_epoch: current_epoch(),
            solve_group: self.solve.solve_group.clone()?,
        })
    }

    pub fn parse_saved_state(&mut self, saved: SavedGlobalState) {
        log::warn!("Parsed saved state: {saved:?}");

        self.solve.session_id = Some(saved.session_id);
        self.solve.penalty = Some(saved.penalty);
        self.solve.solve_time = Some(saved.solve_time);
        self.solve.current_competitor = Some(saved.current_competitor);
        self.solve.solve_group = Some(saved.solve_group);

        if let Some(inspection_time) = saved.inspection_time {
            let now = Instant::now();
            self.solve.inspection_end = now.checked_add(Duration::from_millis(inspection_time));
            self.solve.inspection_start = Some(now);
        }

        if saved.solve_time > 0 {
            self.solve.scene = Scene::Finished;
        }
    }

    pub fn use_inspection(&self) -> bool {
        match self.solve.solve_group.as_ref().map(|r| r.use_inspection) {
            Some(true) | None => true,
            Some(false) => false,
        }
    }

    #[cfg(feature = "e2e")]
    pub fn snapshot_data(&self) -> crate::structs::SnapshotData {
        let inspection_time = self
            .solve
            .inspection_end
            .zip(self.solve.inspection_start)
            .map(|(end, start)| (end - start).as_millis());

        crate::structs::SnapshotData {
            scene: self.solve.scene.to_index(),
            inspection_time,
            penalty: self.solve.penalty,
            solve_time: self.solve.solve_time,
            current_judge: self.solve.current_judge,
            current_competitor: self.solve.current_competitor,
            group_selected_idx: self.solve.group_selected_idx,
            time_confirmed: self.solve.time_confirmed,
            possible_groups: self.solve.possible_groups.len(),
        }
    }
}

#[cfg(not(feature = "e2e"))]
impl SavedGlobalState {
    pub async fn from_nvs(nvs: &Nvs) -> Option<Self> {
        while EPOCH_BASE.load(Ordering::Relaxed) == 0 {
            Timer::after_millis(5).await;
        }

        let res = nvs.get::<Vec<u8>>(NVS_SAVED_STATE).await.ok()?;
        let res: SavedGlobalState = serde_json::from_slice(&res).ok()?;

        const SAVED_STATE_MAX_AGE_SECS: u64 = 6 * 60 * 60;
        if current_epoch() - res.solve_epoch > SAVED_STATE_MAX_AGE_SECS {
            log::error!("REMOVE SOLVE: {:?} {:?}", current_epoch(), res.solve_epoch);
            return None;
        }

        Some(res)
    }

    pub async fn to_nvs(&self, nvs: &Nvs) {
        use core::sync::atomic::{AtomicBool, Ordering};
        static SAVED_STATE_WRITE_ERR_LOGGED: AtomicBool = AtomicBool::new(false);

        let res = serde_json::to_vec(&self);
        if let Ok(vec) = res {
            _ = nvs.delete(NVS_SAVED_STATE).await;
            let res = nvs.set(NVS_SAVED_STATE, vec.as_slice()).await;
            if let Err(e) = res {
                log::error!("{e:?} Faile to write to nvs! (SAVED_STATE {})", vec.len());
                if !SAVED_STATE_WRITE_ERR_LOGGED.load(Ordering::Relaxed) {
                    crate::utils::error_log::add_error(
                        crate::utils::error_log::codes::NVS_SAVED_STATE_WRITE_FAILED,
                    )
                    .await;

                    SAVED_STATE_WRITE_ERR_LOGGED.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    pub async fn clear_saved_global_state(nvs: &Nvs) {
        use core::sync::atomic::{AtomicBool, Ordering};
        static SAVED_STATE_DELETE_ERR_LOGGED: AtomicBool = AtomicBool::new(false);

        let res = nvs.delete(NVS_SAVED_STATE).await;
        if let Err(e) = res {
            log::error!("{e:?} Faile to delete nvs key! (SAVED_STATE)",);
            if !SAVED_STATE_DELETE_ERR_LOGGED.load(Ordering::Relaxed) {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::NVS_SAVED_STATE_DELETE_FAILED,
                )
                .await;

                SAVED_STATE_DELETE_ERR_LOGGED.store(true, Ordering::Relaxed);
            }
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
