use alloc::{rc::Rc, string::String};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Instant;
use esp_hal_wifimanager::Nvs;

use crate::utils::signaled_mutex::SignaledMutex;

pub static mut EPOCH_BASE: u64 = 1431212400;
pub static mut SLEEP_STATE: bool = false;

#[inline(always)]
pub fn current_epoch() -> u64 {
    unsafe { EPOCH_BASE + Instant::now().as_secs() }
}

#[inline(always)]
pub fn sleep_state() -> bool {
    unsafe { SLEEP_STATE }
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

    pub nvs: Nvs,
}

impl GlobalStateInner {
    pub fn new(nvs: &Nvs) -> Self {
        Self {
            state: SignaledMutex::new(SignaledGlobalStateInner::new()),
            timer_signal: Signal::new(),

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

    pub delegate_hold: Option<u8>,
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

            delegate_hold: None,
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

    pub fn reset_solve_state(&mut self) {
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
    }
}
