use {
    futures::task::Waker,
    mio::{Event, Evented, Events, Poll, PollOpt, Ready, Token},
    parking_lot::Mutex,
    std::{
        collections::HashMap,
        io,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        usize,
    },
};

const MAX_EVENTS: usize = 8192;

/// The event queue. This is the part of the thread pool
/// that drives IO resources using the system selector.
pub struct EventQueue {
    poll: Poll,
    events: Events,
    token: AtomicUsize,
    scheduled: HashMap<Token, Arc<Handler>>,
}

impl EventQueue {
    /// Creates a new `EventQueue` with the default event
    /// capacity.
    pub fn new() -> io::Result<EventQueue> {
        EventQueue::new_priv(None)
    }

    /// Creates a new `EventQueue` with the given event
    /// capacity.
    pub fn with_capacity(capacity: usize) -> io::Result<EventQueue> {
        EventQueue::new_priv(Some(capacity))
    }

    /// Registers a new IO handle with this event queue, tracking the events
    /// it produces.
    pub fn register<E: Evented>(&mut self, io: &E) -> io::Result<Arc<Handler>> {
        let id = self.token.fetch_add(1, Ordering::AcqRel);

        if id == usize::MAX {
            panic!(
                "Registered more than {} Evented types! How did you manage that?",
                usize::MAX - 1
            );
        }

        let token = Token(id);
        let ready = Ready::readable() | Ready::writable();
        let opts = PollOpt::level();
        let handler = Arc::new(Handler {
            state: AtomicUsize::new(0),
            waker: Mutex::new(None),
        });

        self.poll.register(io, token, ready, opts)?;
        self.scheduled.insert(token, Arc::clone(&handler));

        Ok(handler)
    }

    pub fn advance(&mut self) -> io::Result<()> {
        self.poll.poll(&mut self.events, None)?;

        for event in self.events.iter() {
            let token = event.token();

            if let Some(handler) = self.scheduled.get(&token) {
                let state = event.readiness().as_usize();
                handler.state.fetch_or(state, Ordering::AcqRel);
                handler.wake();
            }
        }

        Ok(())
    }

    fn new_priv(capacity: Option<usize>) -> io::Result<EventQueue> {
        let capacity = capacity.unwrap_or(MAX_EVENTS);
        let poll = Poll::new()?;

        Ok(EventQueue {
            poll,
            events: Events::with_capacity(capacity),
            token: AtomicUsize::new(0),
            scheduled: HashMap::new(),
        })
    }
}

pub struct Handler {
    state: AtomicUsize,
    waker: Mutex<Option<Waker>>,
}

impl Handler {
    fn wake(&self) {
        // We don't want to wake if there is no waker,
        // because that means it was a spurious wakeup.
        if let Some(waker) = self.waker.lock().take() {
            waker.wake();
        }
    }
}
