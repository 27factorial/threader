use {
    crossbeam::queue::SegQueue,
    futures::task::Waker,
    mio::{Event, Evented, Events, Poll, PollOpt, Ready, Token},
    parking_lot::Mutex,
    std::{
        collections::HashMap,
        io,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Weak,
        },
        usize,
    },
};

// This is kind of arbitrary, but can be ignored with
// Reactor::with_capacity()
const MAX_EVENTS: usize = 2048;

type Result<T> = std::result::Result<T, HandleError>;

/// An error that can happen during either registration
/// or deregistration.
pub enum HandleError {
    /// Indicates that the reactor that a handle
    /// references has been dropped.
    NoReactor,
    /// Indicates that some IO error occurred.
    IoError(io::Error),
}

/// The event queue. This is the part of the thread pool
/// that drives IO resources using the system selector.
pub struct Reactor {
    shared: Arc<Shared>,
    events: Events,
}

impl Reactor {
    /// Creates a new `Reactor` with the default event
    /// capacity.
    pub fn new() -> io::Result<Reactor> {
        Reactor::new_priv(None)
    }

    /// Creates a new `Reactor` with the given event
    /// capacity.
    pub fn with_capacity(capacity: usize) -> io::Result<Reactor> {
        Reactor::new_priv(Some(capacity))
    }

    /// Registers a new IO resource with this reactor.
    pub fn register<E: Evented>(&self, io: &E) -> io::Result<Arc<EventHandler>> {
        let token = match self.shared.tokens.pop() {
            Ok(token) => token,
            Err(_) => {
                let id = self.shared.current_token.fetch_add(1, Ordering::AcqRel);

                if id == usize::MAX {
                    panic!(
                        "Registered more than {} Evented types! How did you manage that?",
                        usize::MAX - 1
                    );
                }

                Token(id)
            }
        };

        let ready = Ready::readable() | Ready::writable();
        let opts = PollOpt::level();
        let handler = Arc::new(EventHandler {
            token,
            state: AtomicUsize::new(0),
            waker: Mutex::new(None),
        });

        self.shared.poll.register(io, token, ready, opts)?;
        self.shared
            .scheduled
            .lock()
            .insert(token, Arc::clone(&handler));

        Ok(handler)
    }

    /// Deregisters an IO resource with this reactor.
    pub fn deregister<E: Evented>(&self, io: &E, handler: &EventHandler) -> io::Result<()> {
        self.shared.poll.deregister(io)?;
        self.shared.scheduled.lock().remove(&handler.token);
        self.shared.tokens.push(handler.token);

        Ok(())
    }

    /// Creates a new handle to this reactor.
    pub fn handle(&self) -> Handle {
        Handle(Arc::downgrade(&self.shared))
    }

    pub fn advance(&mut self) -> io::Result<()> {
        let scheduled = self.shared.scheduled.lock();

        self.shared.poll.poll(&mut self.events, None)?;

        for event in self.events.iter() {
            let token = event.token();

            if let Some(handler) = scheduled.get(&token) {
                let state = event.readiness().as_usize();
                handler.state.fetch_or(state, Ordering::AcqRel);
                handler.wake();
            }
        }

        self.events.clear();

        Ok(())
    }

    // a private function for reducing code duplication.
    fn new_priv(capacity: Option<usize>) -> io::Result<Reactor> {
        let capacity = capacity.unwrap_or(MAX_EVENTS);
        let poll = Poll::new()?;

        let shared = Arc::new(Shared {
            poll,
            tokens: SegQueue::new(),
            current_token: AtomicUsize::new(0),
            scheduled: Mutex::new(HashMap::new()),
        });

        Ok(Reactor {
            shared,
            events: Events::with_capacity(capacity),
        })
    }
}

pub struct Handle(Weak<Shared>);

impl Handle {
    /// Registers a new IO resource with this handle.
    pub fn register<E: Evented>(&self, io: &E) -> Result<Arc<EventHandler>> {
        use self::HandleError::*;

        match self.0.upgrade() {
            Some(inner) => {
                let token = match inner.tokens.pop() {
                    Ok(token) => token,
                    Err(_) => {
                        let id = inner.current_token.fetch_add(1, Ordering::AcqRel);

                        if id == usize::MAX {
                            panic!(
                                "Registered more than {} Evented types! How did you manage that?",
                                usize::MAX - 1
                            );
                        }

                        Token(id)
                    }
                };

                let ready = Ready::readable() | Ready::writable();
                let opts = PollOpt::level();
                let handler = Arc::new(EventHandler {
                    token,
                    state: AtomicUsize::new(0),
                    waker: Mutex::new(None),
                });

                inner
                    .poll
                    .register(io, token, ready, opts)
                    .map_err(|io| IoError(io))?;
                inner.scheduled.lock().insert(token, Arc::clone(&handler));

                Ok(handler)
            }
            None => Err(NoReactor),
        }
    }

    /// Deregisters an IO resource with this handle.
    pub fn deregister<E: Evented>(&self, io: &E, handler: &EventHandler) -> Result<()> {
        use self::HandleError::*;

        match self.0.upgrade() {
            Some(inner) => {
                inner.poll.deregister(io).map_err(|io| IoError(io))?;
                inner.scheduled.lock().remove(&handler.token);
                inner.tokens.push(handler.token);

                Ok(())
            }
            None => Err(NoReactor),
        }
    }
}

struct Shared {
    poll: Poll,
    tokens: SegQueue<Token>,
    current_token: AtomicUsize,
    scheduled: Mutex<HashMap<Token, Arc<EventHandler>>>,
}

pub struct EventHandler {
    token: Token,
    state: AtomicUsize,
    waker: Mutex<Option<Waker>>,
}

impl EventHandler {
    fn wake(&self) {
        // We don't want to wake if there is no waker,
        // because that means it was a spurious wakeup.
        if let Some(waker) = self.waker.lock().take() {
            waker.wake();
        }
    }
}
