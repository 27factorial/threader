use super::{ExecutorFuture, PollGuard};
use crate::thread_pool::Shared;
use crossbeam::utils::Backoff;
use std::{
    cell::UnsafeCell,
    mem,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
    task::{Context, Poll},
};

#[derive(Debug)]
pub(super) struct RawTask {
    ptr: *const Header,
}

impl RawTask {
    pub(super) fn new<F: ExecutorFuture>(future: F, shared: Weak<Shared>) -> Self {
        let inner = Inner::new(future, shared);
        let header = inner.header();
        mem::forget(inner);

        Self { ptr: header }
    }

    pub(super) fn lock(&self) -> PollGuard {
        unsafe {
            lock(self.ptr);
        }
        PollGuard(())
    }

    pub(super) fn poll(&self, cx: &mut Context, _: PollGuard) {
        // SAFETY: Since we pass in a guard here, and the only
        // way to obtain a PollGuard is by locking the task, we
        // have upheld the invariant of the vtable's poll fn.
        unsafe {
            let vtable_poll = (*(self.ptr)).vtable.poll;
            vtable_poll(self.ptr, cx);
        }
    }

    pub(super) fn ptr(&self) -> *const Header {
        self.ptr
    }

    pub(super) unsafe fn from_header(header: *const Header) -> Self {
        Self { ptr: header }
    }
}

impl Clone for RawTask {
    fn clone(&self) -> Self {
        unsafe {
            let vtable = (*(self.ptr)).vtable;
            (vtable.inc_refcount)(self.ptr);
        }

        RawTask { ptr: self.ptr }
    }
}

impl Drop for RawTask {
    fn drop(&mut self) {
        unsafe {
            let vtable = (*(self.ptr)).vtable;
            (vtable.drop)(self.ptr);
        }
    }
}

unsafe impl Send for RawTask {}
unsafe impl Sync for RawTask {}

#[repr(C)]
struct Inner<F>
where
    F: ExecutorFuture,
{
    header: Header,
    future: UnsafeCell<F>,
}

impl<F> Inner<F>
where
    F: ExecutorFuture,
{
    fn new(future: F, shared: Weak<Shared>) -> Arc<Self> {
        let header = Header {
            shared,
            state: AtomicUsize::new(UNLOCKED),
            vtable: VTable::new::<F>(),
        };

        Arc::new(Self {
            header,
            future: UnsafeCell::new(future),
        })
    }

    fn header(&self) -> *const Header {
        &self.header
    }
}

pub(super) struct Header {
    pub(super) shared: Weak<Shared>,
    pub(super) state: AtomicUsize,
    pub(super) vtable: &'static VTable,
}

pub(super) struct VTable {
    pub(super) inc_refcount: unsafe fn(*const Header),
    poll: unsafe fn(*const Header, &mut Context),
    drop: unsafe fn(*const Header),
}

impl VTable {
    fn new<F>() -> &'static VTable
    where
        F: ExecutorFuture,
    {
        &VTable {
            inc_refcount: inc_refcount::<F>,
            poll: poll::<F>,
            drop: drop_raw::<F>,
        }
    }
}

const UNLOCKED: usize = 0;
const LOCKED: usize = 1;
const COMPLETE: usize = 2;

// before calling this function, you *must* ensure
// that you have unique access to the contained
// future. This is usually done with the lock function.
unsafe fn poll<F>(ptr: *const Header, cx: &mut Context)
where
    F: ExecutorFuture,
{
    debug_assert!(!ptr.is_null());
    if (*ptr).state.load(Ordering::Acquire) != COMPLETE {
        let inner = &*(ptr as *const Inner<F>);
        let future = Pin::new_unchecked(&mut *inner.future.get());

        if let Poll::Ready(()) = future.poll(cx) {
            set_complete(ptr);
        } else {
            unlock(ptr);
        };
    }
}

unsafe fn inc_refcount<F>(ptr: *const Header)
where
    F: ExecutorFuture,
{
    let arc = Arc::from_raw(ptr as *const Inner<F>);
    let cloned = Arc::clone(&arc);
    mem::forget(arc);
    mem::forget(cloned);
}

unsafe fn drop_raw<F>(ptr: *const Header)
where
    F: ExecutorFuture,
{
    let arc = Arc::from_raw(ptr as *const Inner<F>);
    drop(arc);
}

unsafe fn lock(ptr: *const Header) {
    let backoff = Backoff::new();
    while (*ptr)
        .state
        .compare_and_swap(UNLOCKED, LOCKED, Ordering::AcqRel)
        != UNLOCKED
    {
        backoff.snooze();
    }
}

unsafe fn unlock(ptr: *const Header) {
    let old = (*ptr).state.swap(UNLOCKED, Ordering::Release);
    debug_assert!(old == LOCKED);
}

unsafe fn set_complete(ptr: *const Header) {
    let old = (*ptr).state.swap(COMPLETE, Ordering::Release);
    debug_assert!(old == LOCKED);
}
