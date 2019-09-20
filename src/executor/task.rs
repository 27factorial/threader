use super::ExecutorHandle;

use {
    crossbeam::{
        self,
        deque::Injector,
        utils::{Backoff, CachePadded},
    },
    futures::{future::Future, task::ArcWake},
    std::{
        cell::{Cell, UnsafeCell},
        fmt,
        ops::{Deref, DerefMut, Drop},
        pin::Pin,
        ptr::NonNull,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Weak,
        },
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
    pub(super) fn new<F>(future: F, injector: &'static Injector<Arc<Task>>) -> Task
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let inner = Inner::new(Box::pin(future), injector);

        Task { inner }
    }

    /// Creates a new `Task` containing the provided future and wraps it in an Arc.
    pub(super) fn arc<F>(future: F, injector: &'static Injector<Arc<Task>>) -> Arc<Task>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Arc::new(Task::new(future, injector))
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
                "threader::task::Task::finish(): Task was rescheduled after finish() was called!"
            );
        }
    }
}

impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        if !arc_self.is_complete() {
            let cloned = Arc::clone(arc_self);
            arc_self.inner.injector.push(cloned);
        }
    }
}

/// A zero-sized struct indicating that the current future is in use.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(super) struct FutureInUse;

/// A wrapper for the future contained inside of a `Task`.
/// It is essentially a wrapper around `UnsafeCell<ExecutorFuture>`
/// that ensures unique access to the underlying future.
struct Inner {
    complete: AtomicBool,
    flag: AtomicBool,
    future: UnsafeCell<ExecutorFuture>,
    injector: &'static Injector<Arc<Task>>,
}

impl Inner {
    /// Creates a new `Inner` instance with the provided future.
    /// the `flag` field is `false` until `Inner::future()` is called.
    fn new(future: ExecutorFuture, injector: &'static Injector<Arc<Task>>) -> Inner {
        Inner {
            complete: AtomicBool::new(false),
            flag: AtomicBool::new(false),
            future: UnsafeCell::new(future),
            injector,
        }
    }

    /// Returns a unique guard to the contained future, spinning if
    /// the future is currently in use.
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
