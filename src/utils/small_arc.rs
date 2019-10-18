use std::{
    marker::PhantomData,
    mem,
    ops::Deref,
    ptr::{self, NonNull},
    sync::atomic::{AtomicUsize, Ordering},
};

pub struct SmallArc<T> {
    ptr: NonNull<Inner<T>>,
    _phantom: PhantomData<T>,
}

impl<T> SmallArc<T> {
    pub fn new(data: T) -> SmallArc<T> {
        let inner = Inner {
            data,
            refs: AtomicUsize::new(1),
        };

        let ptr = unsafe { NonNull::new_unchecked(&inner as *const Inner<T> as *mut Inner<T>) };
        mem::forget(inner);

        SmallArc {
            ptr,
            _phantom: PhantomData,
        }
    }

    pub fn clone(this: &SmallArc<T>) -> SmallArc<T> {
        this.inc_refs();
        SmallArc {
            ptr: this.ptr,
            _phantom: PhantomData,
        }
    }

    fn inc_refs(&self) {
        let res = unsafe { self.ptr.as_ref().refs.fetch_add(1, Ordering::Relaxed) };

        // ensure we don't have over 18 quintillion references.
        assert_ne!(res, !0usize, "Exceeded 2^64 SmallArcs.");
    }

    fn dec_refs(&self) {
        unsafe {
            self.ptr.as_ref().refs.fetch_sub(1, Ordering::Release);
        }
    }
}

unsafe impl<T: Send + Sync> Send for SmallArc<T> {}
unsafe impl<T: Send + Sync> Sync for SmallArc<T> {}

impl<T> Drop for SmallArc<T> {
    fn drop(&mut self) {
        self.dec_refs();

        unsafe {
            if self.ptr.as_ref().refs.load(Ordering::Acquire) == 0 {
                ptr::drop_in_place(self.ptr.as_ptr());
            }
        }
    }
}

impl<T> Deref for SmallArc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &self.ptr.as_ref().data }
    }
}

struct Inner<T> {
    data: T,
    refs: AtomicUsize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new() {
        SmallArc::new(5);
    }
}
