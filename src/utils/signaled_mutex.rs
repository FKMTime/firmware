use core::hash::{Hash, Hasher};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};

/// Async mutex that signals waiters when a `lock()` guard actually changed the value.
///
/// Change detection hashes the value on the first mutable access and again on drop,
/// so read-only locks cost nothing and no copy of the value is kept.
pub struct SignaledMutex<M: RawMutex, T: Hash> {
    inner: Mutex<M, T>,
    update_sig: Signal<M, ()>,
}

#[allow(dead_code)]
impl<M: RawMutex, T: Hash> SignaledMutex<M, T> {
    pub fn new(initial: T) -> Self {
        Self {
            inner: Mutex::new(initial),
            update_sig: Signal::new(),
        }
    }

    pub async fn wait(&self) {
        self.update_sig.wait().await;
    }

    pub fn signal(&self) {
        self.update_sig.signal(());
    }

    pub fn signal_reset(&self) {
        self.update_sig.reset();
    }

    pub fn signalled(&self) -> bool {
        self.update_sig.signaled()
    }

    pub async fn lock(&self) -> SignaledMutexGuard<'_, M, T> {
        SignaledMutexGuard {
            update_sig: &self.update_sig,
            inner_guard: self.inner.lock().await,
            hash_before: None,
        }
    }

    pub async fn wait_lock(&self) -> MutexGuard<'_, M, T> {
        self.update_sig.wait().await;
        self.inner.lock().await
    }

    /// Locks without change detection (callers signal manually if needed).
    pub async fn value(&self) -> MutexGuard<'_, M, T> {
        self.inner.lock().await
    }
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = Fnv1a(0xcbf2_9ce4_8422_2325);
    value.hash(&mut hasher);
    hasher.finish()
}

/// FNV-1a: tiny, no_std, good enough to detect a change (not for adversarial input).
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

pub struct SignaledMutexGuard<'a, M: RawMutex, T: Hash> {
    update_sig: &'a Signal<M, ()>,
    inner_guard: MutexGuard<'a, M, T>,
    /// Hash before the first mutable access (`None` = not mutated).
    hash_before: Option<u64>,
}

impl<M: RawMutex, T: Hash> Drop for SignaledMutexGuard<'_, M, T> {
    fn drop(&mut self) {
        if let Some(before) = self.hash_before
            && hash_of(&*self.inner_guard) != before
        {
            self.update_sig.signal(()); // signal value change (if actually changed)
        }
    }
}

impl<M: RawMutex, T: Hash> core::ops::Deref for SignaledMutexGuard<'_, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner_guard.deref()
    }
}

impl<M: RawMutex, T: Hash> core::ops::DerefMut for SignaledMutexGuard<'_, M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if self.hash_before.is_none() {
            self.hash_before = Some(hash_of(&*self.inner_guard));
        }
        self.inner_guard.deref_mut()
    }
}
