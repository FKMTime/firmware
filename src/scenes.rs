use alloc::rc::Rc;
use embassy_sync::{
    blocking_mutex::raw::{NoopRawMutex, RawMutex},
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};

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
    CompetitorInfo(u128),
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
}

pub struct SignaledNoopMutex<T> {
    inner: Mutex<NoopRawMutex, T>,
    update_sig: Signal<NoopRawMutex, ()>,
}

#[allow(dead_code)]
impl<T> SignaledNoopMutex<T> {
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

    pub async fn lock(&self) -> SignaledNoopMutexGuard<'_, T> {
        SignaledNoopMutexGuard {
            update_sig: &self.update_sig,
            inner_guard: self.inner.lock().await,
        }
    }

    pub async fn wait_lock(&self) -> MutexGuard<'_, NoopRawMutex, T> {
        self.update_sig.wait().await;
        self.inner.lock().await
    }

    pub async fn value(&self) -> MutexGuard<'_, NoopRawMutex, T> {
        self.inner.lock().await
    }
}

pub struct SignaledNoopMutexGuard<'a, T> {
    update_sig: &'a Signal<NoopRawMutex, ()>,
    inner_guard: MutexGuard<'a, NoopRawMutex, T>,
}

impl<'a, T> Drop for SignaledNoopMutexGuard<'a, T> {
    fn drop(&mut self) {
        self.update_sig.signal(()); // signal value
    }
}

impl<'a, T> core::ops::Deref for SignaledNoopMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner_guard.deref()
    }
}

impl<'a, T> core::ops::DerefMut for SignaledNoopMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner_guard.deref_mut()
    }
}

//pub type GlobalState
pub type GlobalState = Rc<GlobalStateInner>;

pub struct GlobalStateInner {
    pub state: SignaledNoopMutex<SignaledGlobalStateInner>,
    pub timer_signal: Signal<NoopRawMutex, Option<u64>>,
}

impl GlobalStateInner {
    pub fn new() -> Self {
        Self {
            state: SignaledNoopMutex::new(SignaledGlobalStateInner::new()),
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
    pub server_connected: Option<bool>,
    pub stackmat_connected: Option<bool>,
    pub current_competitor: Option<u128>,
}

impl SignaledGlobalStateInner {
    pub fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,
            server_connected: None,
            stackmat_connected: None,
            current_competitor: None,
        }
    }
}
