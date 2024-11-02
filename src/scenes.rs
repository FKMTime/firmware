use alloc::rc::Rc;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::{Mutex, MutexGuard}, signal::Signal};

#[derive(Debug, PartialEq, Clone)]
#[allow(dead_code)]
pub enum Scene {
    /// Waiting for wifi connection
    WifiConnect,

    /// Connect to wifi to setup
    AutoSetupWait,

    /// Waiting for MDNS
    MdnsWait,

    WaitingForCompetitor {
        time: Option<u64>,
    },
    CompetitorInfo(/* Competitor info struct? */),
    Inspection {
        start_time: u64,
    },
    Timer {
        inspection_time: u64,
    },
    Finished {
        inspection_time: u64,
        solve_time: u64,
    },
    Error {
        msg: alloc::string::String,
    },
}

pub struct SignaledNoopMutex<T> {
    inner: Mutex<NoopRawMutex, T>,
    update_sig: Signal<NoopRawMutex, ()>
}

#[allow(dead_code)]
impl<T> SignaledNoopMutex<T> {
    pub fn new(initial: T) -> Self {
        let sig = Signal::new();
        sig.signal(());

        Self {
            inner: Mutex::new(initial),
            update_sig: sig
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
            inner_guard: self.inner.lock().await
        }
    }

    pub async fn wait_lock(&self) -> MutexGuard<'_, NoopRawMutex, T> {
        self.update_sig.wait().await;
        self.inner.lock().await
    }
}

pub struct SignaledNoopMutexGuard<'a, T> {
    update_sig: &'a Signal<NoopRawMutex, ()>,
    inner_guard: MutexGuard<'a, NoopRawMutex, T>
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

pub type GlobalState = Rc<SignaledNoopMutex<GlobalStateInner>>;

#[derive(Debug, Clone)]
pub struct GlobalStateInner {
    pub scene: Scene,
    pub server_connected: Option<bool>
}

impl GlobalStateInner {
    pub fn new() -> Self {
        Self {
            scene: Scene::WifiConnect,
            server_connected: None
        }
    }
}

//pub static PREVIOUS_SCENE: Mutex<CriticalSectionRawMutex, Scene> = Mutex::new(Scene::WifiConnect);
/*
pub static STATE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static CURRENT_STATE: Mutex<CriticalSectionRawMutex, GlobalState> = Mutex::new(GlobalState {
    scene: Scene::WifiConnect,
    server_connected: None
});
*/
