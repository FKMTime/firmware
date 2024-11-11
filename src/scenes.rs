use alloc::{rc::Rc, string::String};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex, RawMutex},
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};

use crate::arc::Arc;

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
    CompetitorInfo(String),
    Inspection,
    Timer,
    Finished,
    Error {
        msg: alloc::string::String,
    },
}

impl Scene {
    pub fn can_be_lcd_overwritten(&self) -> bool {
        match self {
            Scene::WifiConnect => false,
            Scene::AutoSetupWait => false,
            Scene::MdnsWait => false,
            Scene::WaitingForCompetitor => true,
            Scene::CompetitorInfo(_) => true,
            Scene::Inspection => false,
            Scene::Timer => false,
            Scene::Finished => false,
            Scene::Error { .. } => false,
        }
    }

    pub fn to_index(&self) -> usize {
        match self {
            Scene::WifiConnect => 0,
            Scene::AutoSetupWait => 1,
            Scene::MdnsWait => 2,
            Scene::WaitingForCompetitor => 3,
            Scene::CompetitorInfo(_) => 4,
            Scene::Inspection => 5,
            Scene::Timer => 6,
            Scene::Finished => 7,
            Scene::Error { .. } => 8,
        }
    }
}

impl PartialOrd for Scene {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.to_index().cmp(&other.to_index()))
    }
}

pub struct SignaledMutex<M: RawMutex, T> {
    inner: Mutex<M, T>,
    update_sig: Signal<M, ()>,
}

#[allow(dead_code)]
impl<M: RawMutex, T> SignaledMutex<M, T> {
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

    pub async fn lock(&self) -> SignaledMutexGuard<'_, M, T> {
        SignaledMutexGuard {
            update_sig: &self.update_sig,
            inner_guard: self.inner.lock().await,
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

pub struct SignaledMutexGuard<'a, M: RawMutex, T> {
    update_sig: &'a Signal<M, ()>,
    inner_guard: MutexGuard<'a, M, T>,
}

impl<'a, M: RawMutex, T> Drop for SignaledMutexGuard<'a, M, T> {
    fn drop(&mut self) {
        self.update_sig.signal(()); // signal value
    }
}

impl<'a, M: RawMutex, T> core::ops::Deref for SignaledMutexGuard<'a, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner_guard.deref()
    }
}

impl<'a, M: RawMutex, T> core::ops::DerefMut for SignaledMutexGuard<'a, M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner_guard.deref_mut()
    }
}

//pub type GlobalState
pub type GlobalState = Arc<GlobalStateInner>;

pub struct GlobalStateInner {
    pub state: SignaledMutex<CriticalSectionRawMutex, SignaledGlobalStateInner>,
    pub timer_signal: Signal<CriticalSectionRawMutex, Option<u64>>,
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

#[derive(Debug, Clone)]
pub struct SignaledGlobalStateInner {
    pub scene: Scene,
    pub device_added: Option<bool>,
    pub server_connected: Option<bool>,
    pub stackmat_connected: Option<bool>,
    pub current_competitor: Option<u128>,
    pub test_hold: Option<u64>,
}

impl SignaledGlobalStateInner {
    pub fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,
            device_added: None,
            server_connected: None,
            stackmat_connected: None,
            current_competitor: None,
            test_hold: None,
        }
    }
}
