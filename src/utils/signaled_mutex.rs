use alloc::rc::Rc;
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};

/// A mutex paired with a shared change-notification signal. `lock()` signals on
/// drop; `lock_silent()` does not. Every sub-state shares the one render signal,
/// so a change in any of them wakes the renderer. There is intentionally no
/// clone/diff of `T`: signaling is the caller's intent, not a whole-struct compare.
pub struct SignaledMutex<M: RawMutex, T> {
    inner: Mutex<M, T>,
    notify: Rc<Signal<M, ()>>,
}

impl<M: RawMutex, T> SignaledMutex<M, T> {
    pub fn new(initial: T, notify: Rc<Signal<M, ()>>) -> Self {
        Self {
            inner: Mutex::new(initial),
            notify,
        }
    }

    pub fn signal(&self) {
        self.notify.signal(());
    }

    pub async fn lock(&self) -> SignaledMutexGuard<'_, M, T> {
        let inner_guard = self.inner.lock().await;

        SignaledMutexGuard {
            notify: &self.notify,
            inner_guard,
        }
    }

    pub async fn lock_silent(&self) -> MutexGuard<'_, M, T> {
        self.inner.lock().await
    }
}

pub struct SignaledMutexGuard<'a, M: RawMutex, T> {
    notify: &'a Rc<Signal<M, ()>>,
    inner_guard: MutexGuard<'a, M, T>,
}

impl<M: RawMutex, T> Drop for SignaledMutexGuard<'_, M, T> {
    fn drop(&mut self) {
        self.notify.signal(());
    }
}

impl<M: RawMutex, T> core::ops::Deref for SignaledMutexGuard<'_, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner_guard.deref()
    }
}

impl<M: RawMutex, T> core::ops::DerefMut for SignaledMutexGuard<'_, M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner_guard.deref_mut()
    }
}
