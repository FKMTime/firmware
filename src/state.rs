use alloc::{rc::Rc, string::String};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use esp_hal_wifimanager::Nvs;
use serde::{Deserialize, Serialize};

use crate::utils::signaled_mutex::SignaledMutex;

pub static mut EPOCH_BASE: u64 = 0;
pub static mut SLEEP_STATE: bool = false;
pub static mut OTA_STATE: bool = false;

#[inline(always)]
pub fn current_epoch() -> u64 {
    unsafe { EPOCH_BASE + Instant::now().as_secs() }
}

#[inline(always)]
pub fn sleep_state() -> bool {
    unsafe { SLEEP_STATE }
}

#[inline(always)]
pub fn get_ota_state() -> bool {
    unsafe { OTA_STATE }
}

#[derive(Debug, PartialEq, Clone)]
#[allow(dead_code)]
pub enum Scene {
    /// Waiting for wifi connection
    WifiConnect,

    /// Connect to wifi to setup
    AutoSetupWait,

    /// Waiting for MDNS
    MdnsWait,

    WaitingForCompetitor,
    CompetitorInfo,
    Inspection,
    Timer,
    Finished,
}

impl Scene {
    pub fn can_be_lcd_overwritten(&self) -> bool {
        match self {
            Scene::WifiConnect => false,
            Scene::AutoSetupWait => false,
            Scene::MdnsWait => false,
            Scene::WaitingForCompetitor => true,
            Scene::CompetitorInfo => true,
            Scene::Inspection => false,
            Scene::Timer => false,
            Scene::Finished => false,
        }
    }

    pub fn to_index(&self) -> usize {
        match self {
            Scene::WifiConnect => 0,
            Scene::AutoSetupWait => 1,
            Scene::MdnsWait => 2,
            Scene::WaitingForCompetitor => 3,
            Scene::CompetitorInfo => 4,
            Scene::Inspection => 5,
            Scene::Timer => 6,
            Scene::Finished => 7,
        }
    }
}

impl PartialOrd for Scene {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.to_index().cmp(&other.to_index()))
    }
}

//pub type GlobalState
pub type GlobalState = Rc<GlobalStateInner>;

pub struct GlobalStateInner {
    pub state: SignaledMutex<CriticalSectionRawMutex, SignaledGlobalStateInner>,
    pub timer_signal: Signal<CriticalSectionRawMutex, u64>,
    pub show_battery: Signal<CriticalSectionRawMutex, u8>,

    pub nvs: Nvs,
}

impl GlobalStateInner {
    pub fn new(nvs: &Nvs) -> Self {
        Self {
            state: SignaledMutex::new(SignaledGlobalStateInner::new()),
            timer_signal: Signal::new(),
            show_battery: Signal::new(),

            nvs: nvs.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignaledGlobalStateInner {
    pub scene: Scene,

    pub inspection_start: Option<Instant>,
    pub inspection_end: Option<Instant>,
    pub solve_time: Option<u64>,
    pub last_solve_time: Option<u64>,
    pub penalty: Option<i8>,
    pub session_id: Option<String>,
    pub time_confirmed: bool,

    pub use_inspection: bool,
    pub secondary_text: Option<String>,
    pub error_text: Option<String>,

    pub device_added: Option<bool>,
    pub server_connected: Option<bool>,
    pub stackmat_connected: Option<bool>,
    pub current_competitor: Option<u64>,
    pub current_judge: Option<u64>,
    pub competitor_display: Option<String>,

    pub delegate_used: bool,
    pub delegate_hold: Option<u8>,
    pub ota_update: Option<u8>,

    #[cfg(feature = "bat_dev_lcd")]
    pub current_bat_read: Option<f32>,

    #[cfg(feature = "bat_dev_lcd")]
    pub avg_bat_read: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGlobalState {
    pub inspection_time: Option<u64>,
    pub solve_time: u64,
    pub penalty: i8,
    pub session_id: String,
    pub current_competitor: u64,
    pub solve_epoch: u64,
}

impl SignaledGlobalStateInner {
    pub fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,

            inspection_start: None,
            inspection_end: None,
            solve_time: None,
            last_solve_time: None,
            penalty: None,
            session_id: None,
            time_confirmed: false,

            use_inspection: true,
            secondary_text: None,

            error_text: None,
            device_added: None,
            server_connected: None,
            stackmat_connected: None,
            current_competitor: None,
            current_judge: None,
            competitor_display: None,

            delegate_used: false,
            delegate_hold: None,
            ota_update: None,

            #[cfg(feature = "bat_dev_lcd")]
            current_bat_read: None,

            #[cfg(feature = "bat_dev_lcd")]
            avg_bat_read: None,
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
        self.last_solve_time = self.solve_time;

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

        if let Some(nvs) = save_nvs {
            SavedGlobalState::clear_saved_global_state(nvs).await;
        }
    }

    pub fn to_saved_global_state(&self) -> Option<SavedGlobalState> {
        log::debug!("TO_SAVED_GLOBAL_STATE: {self:?}");

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
        })
    }

    pub fn parse_saved_state(&mut self, saved: SavedGlobalState) {
        self.session_id = Some(saved.session_id);
        self.penalty = Some(saved.penalty);
        self.solve_time = Some(saved.solve_time);
        self.current_competitor = Some(saved.current_competitor);

        if let Some(inspection_time) = saved.inspection_time {
            let now = Instant::now();
            self.inspection_end = now.checked_add(Duration::from_millis(inspection_time));
            self.inspection_start = Some(now);
        }

        if saved.solve_time > 0 {
            self.scene = Scene::Finished;
        }
    }
}

impl SavedGlobalState {
    pub async fn from_nvs(nvs: &Nvs) -> Option<Self> {
        while unsafe { EPOCH_BASE == 0 } {
            Timer::after_millis(5).await;
        }

        let mut buf = [0; 1024];
        nvs.get_key(b"SAVED_GLOBAL_STATE", &mut buf).await.ok()?;
        let end_pos = buf.iter().position(|&x| x == 0x00).unwrap_or(buf.len());
        let res: SavedGlobalState = serde_json::from_slice(&buf[..end_pos]).ok()?;

        // 6hours
        if current_epoch() - res.solve_epoch > 6 * 60 * 60 {
            log::error!("REMOVE SOLVE: {:?} {:?}", current_epoch(), res.solve_epoch);
            return None;
        }

        Some(res)
    }

    pub async fn to_nvs(&self, nvs: &Nvs) {
        let res = serde_json::to_vec(&self);
        if let Ok(vec) = res {
            _ = nvs.invalidate_key(b"SAVED_GLOBAL_STATE").await;
            let res = nvs.append_key(b"SAVED_GLOBAL_STATE", &vec).await;
            if let Err(e) = res {
                log::error!(
                    "{e:?} Faile to write to nvs! (SAVED_GLOBAL_STATE {})",
                    vec.len()
                );
            }
        }
    }

    pub async fn clear_saved_global_state(nvs: &Nvs) {
        let res = nvs.invalidate_key(b"SAVED_GLOBAL_STATE").await;
        if let Err(e) = res {
            log::error!("{e:?} Faile to invalidate nvs key! (SAVED_GLOBAL_STATE)",);
        }
    }
}
