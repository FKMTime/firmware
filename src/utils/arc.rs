use alloc::boxed::Box;
use core::{ops::Deref, ptr::NonNull};
use portable_atomic::AtomicUsize;

pub struct Arc<T> {
    ptr: NonNull<ArcInner<T>>,
}

pub struct ArcInner<T> {
    count: AtomicUsize,
    data: T,
}

unsafe impl<T: Send + Sync> Send for Arc<T> {}
unsafe impl<T: Send + Sync> Sync for Arc<T> {}

#[allow(dead_code)]
impl<T> Arc<T> {
    pub fn new(data: T) -> Self {
        let ptr = Box::new(ArcInner {
            count: AtomicUsize::new(1),
            data,
        });

        Arc {
            ptr: NonNull::new(Box::into_raw(ptr)).expect("Box Pointer cant be null?"),
        }
    }

    fn inner(&self) -> &ArcInner<T> {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner().data
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {
        critical_section::with(|_cs| {
            self.inner()
                .count
                .add(1, core::sync::atomic::Ordering::Relaxed);
        });

        Arc { ptr: self.ptr }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        critical_section::with(|_cs| {
            let old = self
                .inner()
                .count
                .fetch_sub(1, core::sync::atomic::Ordering::Relaxed);

            if old == 1 {
                unsafe {
                    drop(Box::from_raw(self.ptr.as_ptr()));
                }
            }
        });
    }
}
