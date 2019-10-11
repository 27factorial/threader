use super::{Handle, IoWaker, Reactor};
use mio::{Evented, PollOpt, Ready, Registration, SetReadiness};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

struct ReactorThread {
    join_handle: thread::JoinHandle<io::Result<()>>,
    reactor_handle: Handle,
    wakeup: SetReadiness,
    shutdown: Arc<AtomicBool>,
    _reg: Registration,
}

impl ReactorThread {
    pub fn new(mut reactor: Reactor) -> io::Result<Self> {
        let (reg, wakeup) = Registration::new2();
        let reactor_handle = reactor.handle();

        reactor.register(&reg, Ready::readable(), PollOpt::edge())?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);

        let join_handle = thread::Builder::new()
            .name("threader::reactor::ReactorThread background thread".into())
            .spawn(move || loop {
                if thread_shutdown.load(Ordering::Relaxed) {
                    return Ok(());
                } else {
                    reactor.poll(None)?;
                }
            })?;

        Ok(Self {
            join_handle,
            reactor_handle,
            wakeup,
            shutdown,
            _reg: reg,
        })
    }

    /// Registers a new IO resource with this handle.
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
}

impl Drop for ReactorThread {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.wakeup.set_readiness(Ready::readable());
    }
}
