mod background;
pub mod observer;

use crossbeam::queue::SegQueue;
use futures::{
    self,
    task::{AtomicWaker, Waker},
};
use mio::{Evented, Events, Poll, PollOpt, Ready, Token};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
    time::Duration,
    usize,
};

static DEFAULT_REACTOR: Lazy<Reactor> = Lazy::new(Reactor::new);

pub fn register<E: Evented>(
    resource: &E,
    interest: Ready,
    opts: PollOpt,
) -> io::Result<Arc<IoWaker>> {
    DEFAULT_REACTOR.register(resource, interest, opts)
}

pub fn handle() -> Handle {
    DEFAULT_REACTOR.handle()
}

// This is kind of arbitrary, but can be ignored with
// Reactor::with_capacity()
const MAX_EVENTS: usize = 2048;

/// The reactor. This is the part of the thread pool
/// that drives IO resources using the system selector.
#[derive(Debug)]
pub struct Reactor {
    shared: Arc<Shared>,
    events: Events,
}

impl Reactor {
    /// Creates a new `Reactor` with the default event
    /// capacity.
    pub fn new() -> Self {
        Self::new_priv(None)
    }

    /// Creates a new `Reactor` with the given event
    /// capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        assert_ne!(
            capacity, 0,
            "Can not create a reactor which polls for 0 events."
        );
        Self::new_priv(Some(capacity))
    }

    /// Registers a new IO resource with this reactor.
    pub fn register<E: Evented>(
        &self,
        resource: &E,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<Arc<IoWaker>> {
        let token = match self.shared.tokens.pop() {
            Ok(token) => token,
            Err(_) => {
                let id = self.shared.current_token.fetch_add(1, Ordering::SeqCst);

                if id == usize::MAX {
                    panic!(
                        "Registered more than {} Evented types! How did you manage that?",
                        usize::MAX - 1
                    );
                }

                Token(id)
            }
        };

        let io_waker = Arc::new(IoWaker {
            token,
            readiness: AtomicUsize::new(0),
            read_waker: AtomicWaker::new(),
            write_waker: AtomicWaker::new(),
        });

        self.shared.poll.register(resource, token, interest, opts)?;
        self.shared
            .resources
            .lock()
            .insert(token, Arc::clone(&io_waker));

        Ok(io_waker)
    }

    pub fn reregister<E: Evented>(
        &self,
        resource: &E,
        io_waker: &IoWaker,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        self.shared
            .poll
            .reregister(resource, io_waker.token, interest, opts)
    }

    /// Deregisters an IO resource with this reactor.
    pub fn deregister<E: Evented>(&self, resource: &E, io_waker: &IoWaker) -> io::Result<()> {
        self.shared.poll.deregister(resource)?;
        self.shared.resources.lock().remove(&io_waker.token);
        self.shared.tokens.push(io_waker.token);

        Ok(())
    }

    /// Creates a new handle to this reactor.
    pub fn handle(&self) -> Handle {
        Handle(Arc::downgrade(&self.shared))
    }

    /// Polls the reactor once, notifying IO resources if any events
    /// were detected. This is usually done in a loop, and will most
    /// likely not return an error unless there was an error with
    /// the system selector.
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<usize> {
        let events_polled = self.shared.poll.poll(&mut self.events, timeout)?;
        let resources = self.shared.resources.lock();

        for event in self.events.iter() {
            let token = event.token();

            if let Some(resource) = resources.get(&token) {
                resource.wake_if_ready(event.readiness());
            }
        }

        self.events.clear();

        Ok(events_polled)
    }

    // a private function for reducing code duplication.
    fn new_priv(capacity: Option<usize>) -> Reactor {
        let capacity = capacity.unwrap_or(MAX_EVENTS);
        let poll = Poll::new().expect("Failed to create new Poll instance!");

        let shared = Arc::new(Shared {
            poll,
            tokens: SegQueue::new(),
            current_token: AtomicUsize::new(0),
            resources: Mutex::new(HashMap::new()),
        });

        Self {
            shared,
            events: Events::with_capacity(capacity),
        }
    }
}

/// A handle to the reactor. Can be used to register and
/// deregister resources from other threads.
#[derive(Debug, Clone)]
pub struct Handle(Weak<Shared>);

impl Handle {
    /// Registers a new IO resource with this handle.
    pub fn register<E: Evented>(
        &self,
        resource: &E,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<Arc<IoWaker>> {
        match self.0.upgrade() {
            Some(inner) => {
                let token = match inner.tokens.pop() {
                    Ok(token) => token,
                    Err(_) => {
                        let id = inner.current_token.fetch_add(1, Ordering::SeqCst);

                        if id == usize::MAX {
                            panic!(
                                "Registered more than {} resources! How did you manage that?",
                                usize::MAX - 1
                            );
                        }

                        Token(id)
                    }
                };

                let io_waker = Arc::new(IoWaker {
                    token,
                    readiness: AtomicUsize::new(0),
                    read_waker: AtomicWaker::new(),
                    write_waker: AtomicWaker::new(),
                });

                inner.poll.register(resource, token, interest, opts)?;
                inner.resources.lock().insert(token, Arc::clone(&io_waker));

                Ok(io_waker)
            }
            None => Err(io::Error::new(io::ErrorKind::Other, "No Reactor")),
        }
    }

    pub fn reregister<E: Evented>(
        &self,
        resource: &E,
        io_waker: &IoWaker,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        match self.0.upgrade() {
            Some(inner) => inner
                .poll
                .reregister(resource, io_waker.token, interest, opts),
            None => Err(io::Error::new(io::ErrorKind::Other, "No Reactor")),
        }
    }

    /// Stops tracking notifications from the provided IO resource.
    pub fn deregister<E: Evented>(&self, resource: &E, io_waker: &IoWaker) -> io::Result<()> {
        match self.0.upgrade() {
            Some(inner) => {
                inner.poll.deregister(resource)?;
                inner.resources.lock().remove(&io_waker.token);
                inner.tokens.push(io_waker.token);

                Ok(())
            }
            None => Err(io::Error::new(io::ErrorKind::Other, "No Reactor")),
        }
    }
}

#[derive(Debug)]
struct Shared {
    poll: Poll,
    tokens: SegQueue<Token>,
    current_token: AtomicUsize,
    resources: Mutex<HashMap<Token, Arc<IoWaker>>>,
}

/// A struct that associates a resource with two wakers which
/// correspond to read and write operations.
#[derive(Debug)]
pub struct IoWaker {
    token: Token,
    readiness: AtomicUsize,
    read_waker: AtomicWaker,
    write_waker: AtomicWaker,
}

impl IoWaker {
    /// Checks the ready value, waking the read_waker and write_waker
    /// as necessary.
    fn wake_if_ready(&self, ready: Ready) {
        self.readiness.fetch_or(ready.as_usize(), Ordering::SeqCst);

        if ready.is_readable() {
            self.read_waker.wake();
        }

        if ready.is_writable() {
            self.write_waker.wake();
        }
    }

    /// Registers a new reading waker.
    fn register_read(&self, waker: &Waker) {
        self.read_waker.register(waker)
    }

    /// Registers a new writing waker.
    fn register_write(&self, waker: &Waker) {
        self.write_waker.register(waker)
    }

    /// Clears the read readiness of this IoWaker.
    fn clear_read(&self) {
        self.readiness
            .fetch_and(!Ready::readable().as_usize(), Ordering::SeqCst);
    }

    /// Clears the write readiness of this IoWaker.
    fn clear_write(&self) {
        self.readiness
            .fetch_and(!Ready::writable().as_usize(), Ordering::SeqCst);
    }
}
