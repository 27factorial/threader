use super::Shared;
use crossbeam::{deque::Injector, utils::Backoff};
use futures::{future::Future, task::ArcWake};
use std::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut, Drop},
    pin::Pin,
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
};

/// A type representing a future with no output and a 'static lifetime.
type ExecutorFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// A task that can be run on an executor.
#[derive(Debug)]
pub(super) struct Task {
    inner: Inner,
}

impl Task {
    /// Creates a new `Task` containing the provided future.
    pub(super) fn new<F>(future: F, shared: Weak<Shared>) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let inner = Inner::new(Box::pin(future), shared);

        Self { inner }
    }

    /// Creates a new `Task` containing the provided future and wraps it in an Arc.
    pub(super) fn arc<F>(future: F, shared: Weak<Shared>) -> Arc<Self>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Arc::new(Self::new(future, shared))
    }

    /// Returns the future that is contained in the `inner` field of this
    /// `Task`, spinning if the future is currently being used.
    pub(super) fn future(&self) -> FutureRef<'_> {
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
}

// TODO: replace this with a more suitable implementation.
impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        if !arc_self.is_complete() {
            if let Some(shared) = arc_self.inner.shared.upgrade() {
                let cloned = Arc::clone(arc_self);
                shared.injector.push(cloned);
            }
        }
    }
}

/// A wrapper for the future contained inside of a `Task`.
/// It is essentially a wrapper around `UnsafeCell<ExecutorFuture>`
/// that ensures unique access to the underlying future.
struct Inner {
    complete: AtomicBool,
    flag: AtomicBool,
    future: UnsafeCell<ExecutorFuture>,
    shared: Weak<Shared>,
}

impl Inner {
    /// Creates a new `Inner` instance with the provided future.
    /// the `flag` field is `false` until `Inner::future()` is called.
    fn new(future: ExecutorFuture, shared: Weak<Shared>) -> Self {
        Self {
            complete: AtomicBool::new(false),
            flag: AtomicBool::new(false),
            future: UnsafeCell::new(future),
            shared,
        }
    }

    /// Returns a unique guard to the contained future, spinning in
    /// an exponential backoff loop if the future is currently in use.
    fn future(&self) -> FutureRef<'_> {
        use Ordering::{AcqRel, Acquire};

        let backoff = Backoff::new();
        while let Err(_) = self.flag.compare_exchange(false, true, AcqRel, Acquire) {
            backoff.snooze();
        }

        let future = NonNull::new(self.future.get())
            .expect("threader::task::Inner::future(): future was null!");

        FutureRef {
            flag: &self.flag,
            future,
        }
    }
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Inner")
            .field("flag", &self.flag)
            .field("future", &"UnsafeCell { data: ExecutorFuture { .. } }")
            .finish()
    }
}

// SAFETY: This impl is safe because of the way that Inner works.
// Since it does not allow multiple references to the underlying
// future, there's no possibility of a data race.
unsafe impl Sync for Inner {}

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
