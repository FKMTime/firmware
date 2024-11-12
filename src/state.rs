use crate::arc::Arc;
use alloc::string::String;
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex},
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};
use embassy_time::Instant;

pub static mut EPOCH_BASE: u64 = 1431212400;
pub fn get_current_epoch() -> u64 {
    unsafe { EPOCH_BASE + Instant::now().as_secs() }
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

pub struct SignaledMutex<M: RawMutex, T: Clone + PartialEq> {
    inner: Mutex<M, T>,
    update_sig: Signal<M, ()>,
}

#[allow(dead_code)]
impl<M: RawMutex, T: Clone + PartialEq> SignaledMutex<M, T> {
    pub fn new(initial: T) -> Self {
        let sig = Signal::new();
        //sig.signal(());

        Self {
            inner: Mutex::new(initial),
            update_sig: sig,
        }
    }

    pub async fn wait(&self) {
        self.update_sig.wait().await;
    }

    pub fn signal(&self) {
        self.update_sig.signal(());
    }

    pub fn signalled(&self) -> bool {
        self.update_sig.signaled()
    }

    pub async fn lock(&self) -> SignaledMutexGuard<'_, M, T> {
        let inner_guard = self.inner.lock().await;
        let old_value = (*inner_guard).clone();

        SignaledMutexGuard {
            update_sig: &self.update_sig,
            inner_guard,
            old_value,
        }
    }

    pub async fn wait_lock(&self) -> MutexGuard<'_, M, T> {
        self.update_sig.wait().await;
        self.inner.lock().await
    }

    pub async fn value(&self) -> MutexGuard<'_, M, T> {
        self.inner.lock().await
    }
}

pub struct SignaledMutexGuard<'a, M: RawMutex, T: Clone + PartialEq> {
    update_sig: &'a Signal<M, ()>,
    inner_guard: MutexGuard<'a, M, T>,

    old_value: T,
}

impl<'a, M: RawMutex, T: Clone + PartialEq> Drop for SignaledMutexGuard<'a, M, T> {
    fn drop(&mut self) {
        if *self.inner_guard != self.old_value {
            self.update_sig.signal(()); // signal value change (if actually changed)
        }
    }
}

impl<'a, M: RawMutex, T: Clone + PartialEq> core::ops::Deref for SignaledMutexGuard<'a, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner_guard.deref()
    }
}

impl<'a, M: RawMutex, T: Clone + PartialEq> core::ops::DerefMut for SignaledMutexGuard<'a, M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner_guard.deref_mut()
    }
}

//pub type GlobalState
pub type GlobalState = Arc<GlobalStateInner>;

pub struct GlobalStateInner {
    pub state: SignaledMutex<CriticalSectionRawMutex, SignaledGlobalStateInner>,
    pub timer_signal: Signal<CriticalSectionRawMutex, u64>,
}

impl GlobalStateInner {
    pub fn new() -> Self {
        Self {
            state: SignaledMutex::new(SignaledGlobalStateInner::new()),
            timer_signal: Signal::new(),
        }
    }

    pub async fn sig_or_update<M: RawMutex, T: Send>(&self, signal: &Signal<M, T>) -> Option<T> {
        match embassy_futures::select::select(self.state.wait(), signal.wait()).await {
            embassy_futures::select::Either::First(_) => None,
            embassy_futures::select::Either::Second(val) => Some(val),
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
