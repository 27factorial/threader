use crate::utils::debug_unreachable::debug_unreachable;
use crossbeam::utils::Backoff;
use parking_lot::{Condvar, Mutex};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub fn new2() -> (ThreadParker, ThreadUnparker) {
    let parker = ThreadParker::new();
    let unparker = parker.unparker();
    (parker, unparker)
}

pub struct ThreadParker {
    inner: Arc<Inner>,
}

impl ThreadParker {
    pub fn new() -> ThreadParker {
        ThreadParker {
            inner: Arc::new(Inner::new()),
        }
    }

    pub fn unparker(&self) -> ThreadUnparker {
        ThreadUnparker {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn park(&self) {
        self.inner.park();
    }
}

pub struct ThreadUnparker {
    inner: Arc<Inner>,
}

impl ThreadUnparker {
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
            _ => debug_unreachable(),
        }

        // At this point, we know the state is NOTIFIED, and
        // we're woken up, so we can set this to UNPARKED.
        self.notified.store(UNPARKED, Ordering::Release);
    }

    fn unpark(&self) {
        let old = self.notified.swap(NOTIFY, Ordering::AcqRel);

        // There's a short period when the parker is waiting
        // to lock the mutex and before it is unlocked when
        // waiting for the cvar. This accounts for that
        // small period by ensuring the parker is ready
        // to be notified.
        drop(self.lock.lock());
        self.cvar.notify_one();
    }
}
