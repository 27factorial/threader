use parking_lot::{Condvar, Mutex};
use std::sync::{
    atomic::{AtomicBool, Ordering},
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
            inner: Arc::new(Inner {
                notified: AtomicBool::new(false),
                lock: Mutex::new(()),
                cvar: Condvar::new(),
            }),
        }
    }

    pub fn unparker(&self) -> ThreadUnparker {
        ThreadUnparker {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn park(&self) {
        if !self
            .inner
            .notified
            .compare_and_swap(true, false, Ordering::AcqRel)
        {
            self.inner.cvar.wait(&mut self.inner.lock.lock());
        }
    }
}

pub struct ThreadUnparker {
    inner: Arc<Inner>,
}

impl ThreadUnparker {
    pub fn unpark(&self) -> bool {
        // Returns true if this thread is already notified, so we have to invert it.
        let notified = !self
            .inner
            .notified
            .compare_and_swap(false, true, Ordering::AcqRel);
        let unparked = self.inner.cvar.notify_one();

        notified || unparked
    }
}

struct Inner {
    notified: AtomicBool,
    lock: Mutex<()>,
    cvar: Condvar,
}
