use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};

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

impl<M: RawMutex, T: Clone + PartialEq> Drop for SignaledMutexGuard<'_, M, T> {
    fn drop(&mut self) {
        if *self.inner_guard != self.old_value {
            self.update_sig.signal(()); // signal value change (if actually changed)
        }
    }
}

impl<M: RawMutex, T: Clone + PartialEq> core::ops::Deref for SignaledMutexGuard<'_, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner_guard.deref()
    }
}

impl<M: RawMutex, T: Clone + PartialEq> core::ops::DerefMut for SignaledMutexGuard<'_, M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner_guard.deref_mut()
    }
}
