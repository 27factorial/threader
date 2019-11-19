use super::{Handle, IoWaker, Reactor};
use mio::{Evented, PollOpt, Ready, Registration, SetReadiness};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

/// A handle to a reactor running on a background thread. This
/// can be used to ensure that the reactor is being constantly
/// polled without blocking another thread.
pub struct Background {
    join_handle: Option<JoinHandle<io::Result<()>>>,
    reactor_handle: Handle,
    wakeup: SetReadiness,
    shutdown: Arc<AtomicBool>,
    _reg: Registration,
}

impl Background {
    /// Creates a new background thread which polls the given
    /// reactor. All errors returned during the creation of
    /// the `ReactorThread` will be propagated.
    pub fn new(mut reactor: Reactor) -> io::Result<Self> {
        let (reg, wakeup) = Registration::new2();
        let reactor_handle = reactor.handle();

        reactor.register(&reg, Ready::readable(), PollOpt::edge())?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);

        let join_handle = Some(thread::Builder::new().spawn(move || loop {
            if thread_shutdown.load(Ordering::Relaxed) {
                return Ok(());
            } else {
                reactor.poll(None)?;
            }
        })?);

        Ok(Self {
            join_handle,
            reactor_handle,
            wakeup,
            shutdown,
            _reg: reg,
        })
    }

    /// Returns a handle to inner reactor.
    pub fn handle(&self) -> Handle {
        self.reactor_handle.clone()
    }

    /// Registers a new IO resource with this reactor.
    pub fn register<E: Evented>(
        &self,
        resource: &E,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<Arc<IoWaker>> {
        self.reactor_handle.register(resource, interest, opts)
    }

    pub fn reregister<E: Evented>(
        &self,
        resource: &E,
        io_waker: &IoWaker,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        self.reactor_handle
            .reregister(resource, io_waker, interest, opts)
    }

    /// Stops tracking notifications from the provided IO resource.
    pub fn deregister<E: Evented>(&self, resource: &E, io_waker: &IoWaker) -> io::Result<()> {
        self.reactor_handle.deregister(resource, io_waker)
    }

    /// Shuts down the thread where the reactor is being polled,
    /// panicking if the reactor can not be woken up, and returning
    /// any errors which happened in the thread where the reactor was
    /// being polled.
    pub fn shutdown_now(&mut self) -> io::Result<()> {
        self.shutdown.store(true, Ordering::Relaxed);
        self.wakeup
            .set_readiness(Ready::readable())
            .expect("could not set wakeup readiness");
        self.join_handle
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "reactor thread panicked"))?
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        let _ = self.shutdown_now();
    }
}
