use {
    crossbeam::queue::SegQueue,
    futures::{
        self,
        task::{AtomicWaker, Context, Waker},
    },
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
    /// references has already -been dropped.
    NoReactor,
    /// Indicates that some IO error occurred.
    IoError(io::Error),
}

/// The reactor. This is the part of the thread pool
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
    pub fn register<E: Evented>(&self, io: &E, interest: Ready) -> io::Result<Arc<IoHandle>> {
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
        let resource = Arc::new(IoHandle {
            token,
            state: AtomicUsize::new(0),
            read_waker: Mutex::new(None),
            write_waker: Mutex::new(None),
        });

        self.shared.poll.register(io, token, ready, opts)?;
        self.shared
            .resources
            .lock()
            .insert(token, Arc::clone(&resource));

        Ok(resource)
    }

    /// Deregisters an IO resource with this reactor.
    pub fn deregister<E: Evented>(&self, io: &E, resource: &IoHandle) -> io::Result<()> {
        self.shared.poll.deregister(io)?;
        self.shared.resources.lock().remove(&resource.token);
        self.shared.tokens.push(resource.token);

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
    pub fn poll(&mut self) -> io::Result<()> {
        let scheduled = self.shared.resources.lock();

        self.shared.poll.poll(&mut self.events, None)?;

        for event in self.events.iter() {
            let token = event.token();

            if let Some(resource) = scheduled.get(&token) {
                resource
                    .state
                    .fetch_or(event.readiness().as_usize(), Ordering::AcqRel);
                resource.wake_if_ready(event.readiness());
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
            resources: Mutex::new(HashMap::new()),
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
    pub fn register<E: Evented>(&self, io: &E) -> Result<Arc<IoHandle>> {
        use self::HandleError::*;

        match self.0.upgrade() {
            Some(inner) => {
                let token = match inner.tokens.pop() {
                    Ok(token) => token,
                    Err(_) => {
                        let id = inner.current_token.fetch_add(1, Ordering::AcqRel);

                        if id == usize::MAX {
                            panic!(
                                "Registered more than {} resources! How did you manage that?",
                                usize::MAX - 1
                            );
                        }

                        Token(id)
                    }
                };

                let ready = Ready::readable() | Ready::writable();
                let opts = PollOpt::level();
                let resource = Arc::new(IoHandle {
                    token,
                    state: AtomicUsize::new(0),
                    read_waker: Mutex::new(None),
                    write_waker: Mutex::new(None),
                });

                inner
                    .poll
                    .register(io, token, ready, opts)
                    .map_err(|io| IoError(io))?;
                inner.resources.lock().insert(token, Arc::clone(&resource));

                Ok(resource)
            }
            None => Err(NoReactor),
        }
    }

    /// Stops tracking notifications from the provided IO resource.
    pub fn deregister<E: Evented>(&self, io: &E, resource: &IoHandle) -> Result<()> {
        use self::HandleError::*;

        match self.0.upgrade() {
            Some(inner) => {
                inner.poll.deregister(io).map_err(|io| IoError(io))?;
                inner.resources.lock().remove(&resource.token);
                inner.tokens.push(resource.token);

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
    resources: Mutex<HashMap<Token, Arc<IoHandle>>>,
}

pub struct IoHandle {
    token: Token,
    state: AtomicUsize,
    read_waker: Mutex<Option<Waker>>,
    write_waker: Mutex<Option<Waker>>,
}

impl IoHandle {
    /// Checks the ready value, waking the read_waker and write_waker
    /// as necessary.
    fn wake_if_ready(&self, ready: Ready) {
        if ready.is_readable() {
            if let Some(waker) = self.read_waker.lock().take() {
                waker.wake();
            }
        }

        if ready.is_writable() {
            if let Some(waker) = self.write_waker.lock().take() {
                waker.wake();
            }
        }
    }

    /// Registers a new reading waker.
    fn register_read(&self, waker: &Waker) {
        *self.read_waker.lock() = Some(waker.clone());
    }

    /// Registers a new writing waker.
    fn register_write(&self, waker: &Waker) {
        *self.write_waker.lock() = Some(waker.clone());
    }

    fn clear_read(&self) {
        self.state
            .fetch_and(!Ready::readable().as_usize(), Ordering::AcqRel);
    }

    fn clear_write(&self) {
        self.state
            .fetch_and(!Ready::writable().as_usize(), Ordering::AcqRel);
    }
}

pub struct PollResource<E: Evented> {
    resource: E,
    io_handle: Arc<IoHandle>,
}

impl<E: Evented> PollResource<E> {
    pub fn new(resource: E, io_handle: Arc<IoHandle>) -> PollResource<E> {
        PollResource {
            resource,
            io_handle,
        }
    }

    pub fn poll_readable(&self, cx: &mut Context) -> futures::Poll<Ready> {
        let state = Ready::from_usize(self.io_handle.state.load(Ordering::Acquire));

        if state.is_readable() {
            self.io_handle.clear_read();
            futures::Poll::Ready(state)
        } else {
            self.io_handle.register_read(cx.waker());
            futures::Poll::Pending
        }
    }

    pub fn poll_writable(&self, cx: &mut Context) -> futures::Poll<Ready> {
        let state = Ready::from_usize(self.io_handle.state.load(Ordering::Acquire));

        if state.is_writable() {
            self.io_handle.clear_write();
            futures::Poll::Ready(state)
        } else {
            self.io_handle.register_write(cx.waker());
            futures::Poll::Pending
        }
    }
}
