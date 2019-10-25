mod waker;

use super::Shared;
use crate::utils::wrapper::Wrapper;
use crossbeam::{deque::Injector, utils::Backoff};
use futures::{
    self,
    future::Future,
    task::{ArcWake, Waker},
};
use std::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut, Drop},
    pin::Pin,
    ptr::{self, NonNull},
    sync::{
        atomic::{self, AtomicBool, AtomicUsize, Ordering},
        Arc, Weak,
    },
};

pub(crate) type ExecutorFuture = dyn Future<Output = ()> + Send + 'static;

pub fn waker(task: &Task) -> Waker {}

/// A task that can be run on an executor.
#[derive(Debug)]
pub struct Task {
    inner: Arc<Inner<ExecutorFuture>>,
}

impl Task {
    /// Creates a new `Task` containing the provided future.
    pub(super) fn new<F>(future: F, shared: Weak<Shared>) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut inner = Inner::new(future, shared);

        Self { inner }
    }

    /// Returns the future that is contained in the `inner` field of this
    /// `Task`, spinning if the future is currently being used.
    pub(super) fn future(&self) -> Pin<FutureRef<'_>> {
        self.inner.future()
    }

    /// Checks if this `Task` is complete.
    pub(super) fn is_complete(&self) -> bool {
        self.inner.complete.load(Ordering::Acquire)
    }

    /// Sets this `Task`'s `complete` flag to true, making sure that it can not be rescheduled onto
    /// an executor again. This is called when the executor receives `Poll::Ready(())` from the
    /// inner future.
    pub(super) fn complete(&self) {
        // sanity check, this could be replaced with a simple store operation.
        if self.inner.complete.swap(true, Ordering::AcqRel) {
            panic!(
                "threader::task::Task::complete(): Task was rescheduled after complete() was called!"
            );
        }
    }

    fn from_inner(inner: Arc<Inner<ExecutorFuture>>) -> Self {
        Self { inner }
    }
}

/// A wrapper for the future contained inside of a `Task`.
/// It is essentially a wrapper around `UnsafeCell<ExecutorFuture>`
/// that ensures unique access to the underlying future. This can only
/// be constructed with an `ExecutorFuture`, but needs to be generic
/// to construct internally.
struct Inner<F: ?Sized> {
    shared: Weak<Shared>,
    complete: AtomicBool,
    flag: AtomicBool,
    future: UnsafeCell<F>,
}

impl<F> Inner<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    /// Creates a new `Inner` instance with the provided future.
    /// the `flag` field is `false` until `Inner::future()` is called.
    fn new(future: F, shared: Weak<Shared>) -> Arc<Inner<ExecutorFuture>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Arc::new(Self {
            shared,
            complete: AtomicBool::new(false),
            flag: AtomicBool::new(false),
            future: UnsafeCell::new(future),
        })
    }
}

impl Inner<ExecutorFuture> {
    /// Returns a unique guard to the contained future, spinning in
    /// an exponential backoff loop if the future is currently in use.
    fn future(&self) -> Pin<FutureRef<'_>> {
        let backoff = Backoff::new();
        while self.flag.compare_and_swap(false, true, Ordering::AcqRel) {
            backoff.snooze();
        }

        let future = NonNull::new(self.future.get())
            .expect("threader::task::Inner::future(): future was null!");

        unsafe {
            // SAFETY: The future is stored behind a pointer
            // to a heap object, so even moving this will not
            // move the future, so it is pinned in memory.
            Pin::new_unchecked(FutureRef {
                flag: &self.flag,
                future,
            })
        }
    }
}

impl ArcWake for Inner<ExecutorFuture> {
    fn wake(self: Arc<Self>) {
        if let Some(shared) = self.shared.upgrade() {
            let task = Task::from_inner(self);
            shared.injector.push(task);
        }
    }

    fn wake_by_ref(arc_self: &Arc<Self>) {
        ArcWake::wake(Arc::clone(arc_self));
    }
}

impl<F: ?Sized> fmt::Debug for Inner<F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Inner")
            .field("flag", &self.flag)
            .field("future", &"UnsafeCell { .. }")
            .finish()
    }
}

// SAFETY: This impl is safe because of the way that Inner works.
// Since it does not allow multiple references to the underlying
// future, there's no possibility of a data race.
unsafe impl<F: ?Sized> Sync for Inner<F> {}

/// A unique guard to this future. Implements `Deref` and `DerefMut`
/// with a `Target` of `ExecutorFuture`. Its drop implementation also releases
/// unique access.
#[derive(Debug)]
pub(super) struct FutureRef<'a> {
    flag: &'a AtomicBool,
    future: NonNull<ExecutorFuture>,
}

impl<'a> Deref for FutureRef<'a> {
    type Target = ExecutorFuture;

    fn deref(&self) -> &ExecutorFuture {
        unsafe { self.future.as_ref() }
    }
}

impl<'a> DerefMut for FutureRef<'a> {
    fn deref_mut(&mut self) -> &mut ExecutorFuture {
        unsafe { self.future.as_mut() }
    }
}

impl<'a> Drop for FutureRef<'a> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}
