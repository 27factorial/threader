use parking_lot::{Condvar, Mutex};
use std::sync::{
    atomic::{AtomicBool, Ordering},
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
///     let (parker, unparker) = thread_parker::new2();
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

struct Inner {
    notified: AtomicBool,
    lock: Mutex<()>,
    cvar: Condvar,
}

impl Inner {
    fn new() -> Self {
        Self {
            notified: AtomicBool::new(false),
            lock: Mutex::new(()),
            cvar: Condvar::new(),
        }
    }

    fn park(&self) {
        if !self.notified.load(Ordering::Acquire) {
            self.cvar.wait(&mut self.lock.lock());
        }

        self.notified
            .compare_and_swap(false, true, Ordering::Release);
    }

    fn unpark(&self) {
        self.notified.swap(true, Ordering::AcqRel);

        // There's a short period when the parker is waiting
        // to lock the mutex and before it is unlocked when
        // waiting for the cvar. This accounts for that
        // small period by ensuring the parker is ready
        // to be notified.
        drop(self.lock.lock());
        self.cvar.notify_one();
    }
}
