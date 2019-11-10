use crate::utils::debug_utils::debug_unreachable;
use crossbeam::utils::Backoff;
use parking_lot::{Condvar, Mutex};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Creates a new ThreadParker/ThreadUnparker pair. This is similar to creating
/// a [`ThreadParker`] and calling [`unparker`](`ThreadParker::unparker`)
/// on it.
pub fn new2() -> (ThreadParker, ThreadUnparker) {
    let parker = ThreadParker::new();
    let unparker = parker.unparker();
    (parker, unparker)
}

/// A thread parking primitive. Based on [crossbeam]'s implementation, but
/// uses the parking_lot [`Mutex`] and [`Condvar`] instead of the standard
/// library's implementations.
///
/// # Example
///
/// ```
/// use threader::utils::thread_parker;
///
/// fn main() {
///     let (parker, unparker) = thead_parker::new2();
/// }
/// ```
pub struct ThreadParker {
    inner: Arc<Inner>,
}

impl ThreadParker {
    /// Creates a new `ThreadParker` instance.
    ///
    /// # Example
    ///
    /// ```
    /// use std::{
    ///     thread,
    ///     time::Duration,
    /// };
    /// use threader::utils::thread_parker::*;
    ///
    /// fn main() {
    ///     let parker = ThreadParker::new();
    ///     let unparker = parker.unparker();
    ///
    ///     let thread = thread::spawn(move || {
    ///         thread::sleep(Duration::from_secs(5));
    ///         unparker.unpark();
    ///         println!("unparking thread");
    ///     });
    ///
    ///     parker.park();
    ///     println!("Unparked!");
    /// }
    /// ```
    pub fn new() -> ThreadParker {
        ThreadParker {
            inner: Arc::new(Inner::new()),
        }
    }

    /// Creates an [`ThreadUnparker`] which is associated with
    /// this `ThreadParker`.
    ///
    /// # Examples
    ///
    /// ```
    /// use threader::utils::thread_parker::*;
    ///
    /// fn main() {
    ///     let parker = ThreadParker::new();
    ///     let unparker = parker.unparker();
    /// }
    /// ```
    pub fn unparker(&self) -> ThreadUnparker {
        ThreadUnparker {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Parks the thread which calls this method. The thread can
    /// then be woken up by calling [`unpark`] on the associated
    /// [`ThreadUnparker`].
    pub fn park(&self) {
        self.inner.park();
    }
}

/// A thread unparking primitive. Used only to unpark threads which have
/// previously been parked with [`ThreadParker::park`].
pub struct ThreadUnparker {
    inner: Arc<Inner>,
}

impl ThreadUnparker {
    /// Unparks the [`ThreadParker`] that this unparker
    /// is associated with.
    pub fn unpark(&self) {
        self.inner.unpark();
    }
}

const UNPARKED: usize = 0;
const PARKED: usize = 1;
const NOTIFY: usize = 2;

struct Inner {
    notified: AtomicUsize,
    lock: Mutex<()>,
    cvar: Condvar,
}

impl Inner {
    fn new() -> Self {
        Self {
            notified: AtomicUsize::new(UNPARKED),
            lock: Mutex::new(()),
            cvar: Condvar::new(),
        }
    }

    fn park(&self) {
        let old = self.notified.load(Ordering::Acquire);

        match old {
            UNPARKED => {
                // We need to set ourselves to parked iff
                // we're still in the unparked state.
                // Between the load and now, the state
                // may have been changed to notify.
                if self
                    .notified
                    .compare_and_swap(UNPARKED, PARKED, Ordering::AcqRel)
                    == UNPARKED
                {
                    self.cvar.wait(&mut self.lock.lock());
                }
            }
            NOTIFY => (),
            _ => unsafe { debug_unreachable() },
        }

        // At this point, we know the state is NOTIFY, and
        // we're woken up, so we can set this to UNPARKED.
        self.notified.store(UNPARKED, Ordering::Release);
    }

    fn unpark(&self) {
        self.notified.swap(NOTIFY, Ordering::AcqRel);

        // There's a short period when the parker is waiting
        // to lock the mutex and before it is unlocked when
        // waiting for the cvar. This accounts for that
        // small period by ensuring the parker is ready
        // to be notified.
        drop(self.lock.lock());
        self.cvar.notify_one();
    }
}
