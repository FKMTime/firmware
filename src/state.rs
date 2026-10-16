use crate::{
    structs::{
        BleDisplayDevice, DisplayText, MAX_BLE_DEVICES, MAX_POSSIBLE_GROUPS, MessageText,
        PossibleGroup, SessionId,
    },
    utils::bounded::{BoundedVec, CappedVec},
};
use core::cell::RefCell;
use core::hash::{Hash, Hasher};
use embassy_sync::{
    blocking_mutex::{
        Mutex as BlockingMutex,
        raw::{CriticalSectionRawMutex, NoopRawMutex, RawMutex},
    },
    mutex::Mutex,
    signal::Signal,
    watch::Watch,
};
use embassy_time::{Duration, Instant};
use esp_hal::aes::Aes;
use esp_hal_wifimanager::Nvs;
use serde::{Deserialize, Serialize};

pub type Cs = CriticalSectionRawMutex;

/// Wakes the display (redraw) and resets its idle-sleep timer. Single consumer: the lcd task.
pub static CHANGED: Signal<Cs, ()> = Signal::new();

/// Redraw + activity without a state change (button press, wake).
pub fn notify() {
    CHANGED.signal(());
}

/// One domain of shared state. Access is synchronous (no `.await` inside the closure),
/// so a lock can never be held across a network round trip.
///
/// Never access a domain from inside a closure of the *same* domain (the inner `RefCell`
/// borrow would panic); different domains may nest (see [`GlobalStateInner::view`]).
///
/// `M = NoopRawMutex` (default) costs nothing and keeps interrupts on: every task runs on the
/// one embassy executor and no interrupt handler touches these domains, so a closure can't be
/// interrupted by another reader. Use `Cs` for statics read from interrupt-ish paths.
pub struct Shared<T, M: RawMutex = NoopRawMutex>(BlockingMutex<M, RefCell<T>>);

impl<T, M: RawMutex> Shared<T, M> {
    pub const fn new(value: T) -> Self {
        Self(BlockingMutex::new(RefCell::new(value)))
    }

    pub fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.0.lock(|v| f(&v.borrow()))
    }

    /// Changes the value without waking the display (e.g. battery readings).
    pub fn write<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        self.0.lock(|v| f(&mut v.borrow_mut()))
    }
}

impl<T: Hash, M: RawMutex> Shared<T, M> {
    /// Changes the value and wakes the display if it actually changed.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let (changed, r) = self.0.lock(|v| {
            let mut v = v.borrow_mut();
            let before = hash_of(&*v);
            let r = f(&mut v);
            (hash_of(&*v) != before, r)
        });
        if changed {
            notify();
        }
        r
    }
}

/// FNV-1a: tiny, no_std, good enough to detect a change (only runs on updates, never on reads).
fn hash_of<T: Hash>(value: &T) -> u64 {
    struct Fnv1a(u64);
    impl Hasher for Fnv1a {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for &b in bytes {
                self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    let mut h = Fnv1a(0xcbf2_9ce4_8422_2325);
    value.hash(&mut h);
    h.finish()
}

/// A fresh random (v4) uuid, formatted without allocating.
pub fn new_session_id() -> SessionId {
    let mut buf = [0; uuid::fmt::Hyphenated::LENGTH];
    SessionId::truncated(uuid::Uuid::new_v4().hyphenated().encode_lower(&mut buf))
}

#[derive(Debug, PartialEq, Clone, Copy, Hash)]
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

impl Scene {
    pub fn can_be_lcd_overwritten(&self) -> bool {
        matches!(
            self,
            Scene::WaitingForCompetitor | Scene::GroupSelect | Scene::CompetitorInfo
        )
    }

    pub fn to_index(self) -> usize {
        self as usize
    }

    pub fn can_sleep(&self) -> bool {
        !matches!(self, Scene::Update | Scene::WifiConnect | Scene::AutoSetupWait)
    }
}

impl PartialOrd for Scene {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.to_index().cmp(&other.to_index()))
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Hash)]
pub enum MenuScene {
    Signing,
    Unsigning,
    BtDisplay,
    ErrorLog,
    #[cfg(feature = "v4")]
    BuzzerVolume,
}

#[derive(Debug, PartialEq, Clone, Copy, Hash)]
pub enum ErrorLogEntryStage {
    #[cfg(feature = "v4")]
    Qr,
    // Shared stage used on both variants for post-selection detail view.
    Details,
}

/// The current attempt: one source of truth for scene, competitor, group and times.
#[derive(Debug, Clone)]
pub struct Solve {
    pub scene: Scene,
    pub inspection_start: Option<Instant>,
    pub inspection_end: Option<Instant>,
    pub solve_time: Option<u64>,
    pub penalty: Option<i8>,
    pub session_id: Option<SessionId>,
    pub time_confirmed: bool,
    pub solve_group: Option<PossibleGroup>,
    pub possible_groups: CappedVec<PossibleGroup, MAX_POSSIBLE_GROUPS>,
    pub group_selected_idx: usize,
    pub current_competitor: Option<u64>,
    pub current_judge: Option<u64>,
    pub competitor_display: Option<DisplayText>,
    pub delegate_used: bool,
    pub delegate_hold: Option<u8>,
    /// Stackmat stops the timer at the group's limit once; cleared after it fired.
    pub limit_armed: bool,
}

impl Hash for Solve {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.scene.hash(h);
        self.inspection_start.map(|i| i.as_ticks()).hash(h);
        self.inspection_end.map(|i| i.as_ticks()).hash(h);
        self.solve_time.hash(h);
        self.penalty.hash(h);
        self.session_id.hash(h);
        self.time_confirmed.hash(h);
        self.solve_group.hash(h);
        self.possible_groups.hash(h);
        self.group_selected_idx.hash(h);
        self.current_competitor.hash(h);
        self.current_judge.hash(h);
        self.competitor_display.hash(h);
        self.delegate_used.hash(h);
        self.delegate_hold.hash(h);
    }
}

impl Solve {
    const fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,
            inspection_start: None,
            inspection_end: None,
            solve_time: None,
            penalty: None,
            session_id: None,
            time_confirmed: false,
            solve_group: None,
            possible_groups: CappedVec::new(),
            group_selected_idx: 0,
            current_competitor: None,
            current_judge: None,
            competitor_display: None,
            delegate_used: false,
            delegate_hold: None,
            limit_armed: false,
        }
    }

    /// Back to waiting for the next competitor (`delegate_hold` survives, like before).
    pub fn reset(&mut self) {
        let delegate_hold = self.delegate_hold;
        *self = Self::new();
        self.scene = Scene::WaitingForCompetitor;
        self.delegate_hold = delegate_hold;
    }

    /// Selects a group; arms its time limit for the stackmat.
    pub fn set_group(&mut self, group: PossibleGroup) {
        self.limit_armed = group.limit.is_some();
        self.solve_group = Some(group);
    }

    /// Time limit the stackmat should stop at (hot path: a field read).
    pub fn time_limit(&self) -> Option<u64> {
        self.solve_group.as_ref().and_then(|g| g.limit).filter(|_| self.limit_armed)
    }

    pub fn use_inspection(&self) -> bool {
        self.solve_group.as_ref().is_none_or(|g| g.use_inspection)
    }

    /// Inspection duration so far (running) or in total (ended).
    pub fn inspection_ms(&self) -> Option<u64> {
        let start = self.inspection_start?;
        let end = self.inspection_end.unwrap_or_else(Instant::now);
        Some(end.saturating_duration_since(start).as_millis())
    }

    pub fn to_saved(&self) -> Option<SavedGlobalState> {
        Some(SavedGlobalState {
            session_id: self.session_id.clone()?,
            current_competitor: self.current_competitor?,
            penalty: self.penalty.unwrap_or(0),
            solve_time: self.solve_time?,
            inspection_time: self.inspection_end.and(self.inspection_ms()),
            solve_epoch: current_epoch(),
            solve_group: self.solve_group.clone()?,
        })
    }

    pub fn restore(&mut self, saved: SavedGlobalState) {
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

    #[cfg(feature = "e2e")]
    pub fn snapshot_data(&self) -> crate::structs::SnapshotData {
        crate::structs::SnapshotData {
            scene: self.scene.to_index(),
            inspection_time: self.inspection_end.and(self.inspection_ms()),
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

/// Menus, cursors and messages shown on top of the scene (buttons write, display reads).
#[derive(Debug, Clone, Hash)]
pub struct Ui {
    pub menu_scene: Option<MenuScene>,
    pub selected_config_menu: Option<usize>,
    pub selected_error_log_item: usize,
    pub selected_error_log_entry: Option<usize>,
    pub error_log_entry_stage: Option<ErrorLogEntryStage>,
    pub error_log_details_scroll: usize,
    pub discovered_bluetooth_devices: BoundedVec<BleDisplayDevice, MAX_BLE_DEVICES>,
    pub selected_bluetooth_item: usize,
    pub error_text: Option<MessageText>,
    pub custom_message: Option<(DisplayText, DisplayText)>,
}

impl Ui {
    const fn new() -> Self {
        Self {
            menu_scene: None,
            selected_config_menu: None,
            selected_error_log_item: 0,
            selected_error_log_entry: None,
            error_log_entry_stage: None,
            error_log_details_scroll: 0,
            discovered_bluetooth_devices: BoundedVec::new(),
            selected_bluetooth_item: 0,
            error_text: None,
            custom_message: None,
        }
    }
}

/// Connection status icons (`None` = not known yet) and battery.
#[derive(Debug, Clone, Copy, Default, Hash)]
pub struct Status {
    pub wifi_connected: Option<bool>,
    pub server_connected: Option<bool>,
    pub stackmat_connected: Option<bool>,
    pub device_added: Option<bool>,
}

/// Battery reading; written without waking the display (shown on the next redraw).
#[derive(Debug, Clone, Copy, Default)]
pub struct Battery {
    pub percent: u8,
    #[cfg(feature = "v4")]
    pub charging: bool,
    #[cfg(feature = "bat_dev_lcd")]
    pub current_read: Option<f32>,
    #[cfg(feature = "bat_dev_lcd")]
    pub avg_read: Option<f32>,
}

/// What the server told us for this connection (reset on every reconnect) plus its clock.
#[derive(Debug, Clone, Copy)]
pub struct Session {
    pub trust_server: bool,
    pub fkm_token: i32,
    pub secure_rfid: bool,
    pub auto_setup: bool,
    /// Server epoch minus uptime (seconds); 0 = server didn't send EpochTime yet.
    pub epoch_base: u64,
}

impl Session {
    pub fn reset_connection(&mut self) {
        *self = Self {
            epoch_base: self.epoch_base,
            ..Self::new()
        };
    }

    const fn new() -> Self {
        Self {
            trust_server: false,
            fkm_token: 0,
            secure_rfid: false,
            auto_setup: false,
            epoch_base: 0,
        }
    }
}

/// Device settings (NVS / server device settings).
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub sign_key: u32,
    pub sound_enabled: bool,
    #[cfg(feature = "v4")]
    pub buzzer_volume: u8,
}

impl Settings {
    const fn new() -> Self {
        Self {
            sign_key: 0,
            sound_enabled: true,
            #[cfg(feature = "v4")]
            buzzer_volume: crate::consts::BUZZER_VOLUME_DEFAULT,
        }
    }
}

pub static SESSION: Shared<Session, Cs> = Shared::new(Session::new());
pub static SETTINGS: Shared<Settings, Cs> = Shared::new(Settings::new());

/// Sets the epoch from the server's `current_epoch` (seconds).
pub fn set_epoch(current_epoch: u64) {
    let base = current_epoch.saturating_sub(Instant::now().as_secs());
    SESSION.write(|s| s.epoch_base = base);
}

/// True after the server sent its time.
#[cfg_attr(feature = "e2e", allow(dead_code))]
pub fn epoch_known() -> bool {
    SESSION.read(|s| s.epoch_base) != 0
}

pub fn current_epoch() -> u64 {
    SESSION.read(|s| s.epoch_base).saturating_add(Instant::now().as_secs())
}

#[cfg(feature = "e2e")]
#[derive(Default)]
pub struct End2End {
    pub buttons_sig: Signal<Cs, (usize, u64)>,
    pub card_scan_sig: Signal<Cs, u128>,
    pub stackmat_sig: Signal<Cs, (crate::utils::stackmat::StackmatTimerState, u64)>,
}

pub type GlobalState = &'static GlobalStateInner;
pub struct GlobalStateInner {
    pub solve: Shared<Solve>,
    pub ui: Shared<Ui>,
    pub status: Shared<Status>,
    pub battery: Shared<Battery>,

    /// Running timer value in ms (stackmat → display, BLE display, logger).
    pub timer: Watch<Cs, u64, 2>,
    /// Buttons ask the stackmat task to stop the timer (one-shot, single consumer).
    pub timer_stop_signal: Signal<Cs, ()>,
    /// OTA progress in percent (ws → display).
    pub update_progress: Signal<Cs, u8>,
    /// Card sign/unsign result (rfid → display).
    pub sign_unsign_progress: Signal<Cs, bool>,
    /// First battery reading is known (battery task → boot screen).
    pub battery_ready: Signal<Cs, ()>,
    #[cfg(feature = "v4")]
    pub buzzer_sound_test: Signal<Cs, ()>,

    pub nvs: &'static Nvs,
    pub aes: Mutex<Cs, Aes<'static>>,

    #[cfg(feature = "e2e")]
    pub e2e: End2End,
}

impl GlobalStateInner {
    pub fn new(nvs: &'static Nvs, aes: esp_hal::peripherals::AES<'static>) -> Self {
        Self {
            solve: Shared::new(Solve::new()),
            ui: Shared::new(Ui::new()),
            status: Shared::new(Status {
                wifi_connected: None,
                server_connected: None,
                stackmat_connected: None,
                device_added: None,
            }),
            battery: Shared::new(Battery {
                percent: 0,
                #[cfg(feature = "v4")]
                charging: false,
                #[cfg(feature = "bat_dev_lcd")]
                current_read: None,
                #[cfg(feature = "bat_dev_lcd")]
                avg_read: None,
            }),
            timer: Watch::new(),
            timer_stop_signal: Signal::new(),
            update_progress: Signal::new(),
            sign_unsign_progress: Signal::new(),
            battery_ready: Signal::new(),
            #[cfg(feature = "v4")]
            buzzer_sound_test: Signal::new(),
            nvs,
            aes: Mutex::new(Aes::new(aes)),
            #[cfg(feature = "e2e")]
            e2e: End2End::default(),
        }
    }

    /// Back to waiting for a competitor: clears the attempt, the saved copy and the locale.
    pub async fn reset_solve(&self, clear_saved: bool) {
        self.solve.update(Solve::reset);
        self.timer.sender().clear();
        if clear_saved {
            SavedGlobalState::clear_saved_global_state(self.nvs).await;
        }
        crate::translations::restore_default_locale();
    }

    /// e2e: back to a clean attempt and no messages.
    #[cfg(feature = "e2e")]
    pub fn hard_reset(&self) {
        self.solve.update(|s| {
            s.reset();
            s.delegate_hold = None;
        });
        self.timer.sender().clear();
        self.ui.update(|u| {
            u.error_text = None;
            u.custom_message = None;
        });
    }

    /// Read-only view of everything the display draws (borrowed, no copy of the lists).
    pub fn view<R>(&self, f: impl FnOnce(&View) -> R) -> R {
        let (status, battery) = (self.status.read(|s| *s), self.battery.read(|b| *b));
        self.solve.read(|solve| {
            self.ui.read(|ui| {
                f(&View {
                    solve,
                    ui,
                    status,
                    battery,
                })
            })
        })
    }

    /// Buttons/cards are ignored while an error, a lost connection or a setup scene is shown.
    pub fn should_skip_other_actions(&self) -> bool {
        if self.ui.read(|u| u.error_text.is_some()) {
            return true;
        }

        let scene = self.solve.read(|s| s.scene);
        if scene.can_be_lcd_overwritten() {
            let status = self.status.read(|s| *s);
            if status.server_connected == Some(false) || status.stackmat_connected == Some(false) {
                return true;
            }
        }

        scene <= Scene::MdnsWait
    }
}

pub struct View<'a> {
    pub solve: &'a Solve,
    pub ui: &'a Ui,
    pub status: Status,
    #[cfg_attr(not(feature = "v4"), allow(dead_code))]
    pub battery: Battery,
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

/// e2e/qa runs never persist an unfinished solve.
const PERSIST_SOLVES: bool = !cfg!(any(feature = "e2e", feature = "qa"));

impl SavedGlobalState {
    pub async fn from_nvs(nvs: &Nvs) -> Option<Self> {
        if !PERSIST_SOLVES {
            return None;
        }
        while !epoch_known() {
            embassy_time::Timer::after_millis(5).await;
        }

        let res = nvs.get::<alloc::vec::Vec<u8>>(&crate::consts::NVS_SAVED_STATE).await.ok()?;
        let res: SavedGlobalState = serde_json::from_slice(&res).ok()?;

        // 6hours
        if current_epoch().saturating_sub(res.solve_epoch) > 6 * 60 * 60 {
            log::error!("REMOVE SOLVE: {:?} {:?}", current_epoch(), res.solve_epoch);
            return None;
        }

        Some(res)
    }

    pub async fn to_nvs(&self, nvs: &Nvs) {
        if !PERSIST_SOLVES {
            return;
        }
        let res = serde_json::to_vec(&self);
        if let Ok(vec) = res {
            // set replaces the old value atomically (deleting first could lose it on power loss)
            let res = nvs.set(&crate::consts::NVS_SAVED_STATE, vec.as_slice()).await;
            if let Err(e) = res {
                log::error!("{e:?} Faile to write to nvs! (SAVED_STATE {})", vec.len());
                crate::log_once!(crate::utils::error_log::codes::NVS_SAVED_STATE_WRITE_FAILED);
            }
        }
    }

    pub async fn clear_saved_global_state(nvs: &Nvs) {
        if !PERSIST_SOLVES {
            return;
        }
        let res = nvs.delete(&crate::consts::NVS_SAVED_STATE).await;
        if let Err(e) = res {
            log::error!("{e:?} Faile to delete nvs key! (SAVED_STATE)",);
            crate::log_once!(crate::utils::error_log::codes::NVS_SAVED_STATE_DELETE_FAILED);
        }
    }
}
