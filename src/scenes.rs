use embassy_sync::{blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex}, mutex::{Mutex, MutexGuard}, signal::Signal};

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

    pub async fn lock(&self) -> MutexGuard<'_, NoopRawMutex, T> {
        self.inner.lock().await
    }

    pub async fn wait_lock(&self) -> MutexGuard<'_, NoopRawMutex, T> {
        self.update_sig.wait().await;
        self.inner.lock().await
    }
}

pub struct GlobalState {
    pub scene: SignaledNoopMutex<Scene>,
    pub server_connected: SignaledNoopMutex<Option<bool>>
    //pub server_connected: Option<bool>,
}

impl GlobalState {
    pub fn new() -> Self {
        Self {
            scene: SignaledNoopMutex::new(Scene::WifiConnect),
            server_connected: SignaledNoopMutex::new(None)
        }
    }
}

impl core::fmt::Debug for GlobalState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("GlobalState TODO")?;
        Ok(())
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
