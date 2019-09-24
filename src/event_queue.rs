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
    inner: Arc<Inner>,
    token: AtomicUsize,
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
    pub fn register<E: Evented>(&self, io: &E) -> io::Result<Arc<EventHandler>> {
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
        let handler = Arc::new(EventHandler {
            state: AtomicUsize::new(0),
            waker: Mutex::new(None),
        });

        self.inner.poll.register(io, token, ready, opts)?;
        self.inner
            .scheduled
            .lock()
            .insert(token, Arc::clone(&handler));

        Ok(handler)
    }

    pub fn advance(&mut self) -> io::Result<()> {
        let scheduled = self.inner.scheduled.lock();
        let mut events = self.inner.events.lock();

        self.inner.poll.poll(&mut events, None)?;

        for event in events.iter() {
            let token = event.token();

            if let Some(handler) = scheduled.get(&token) {
                let state = event.readiness().as_usize();
                handler.state.fetch_or(state, Ordering::AcqRel);
                handler.wake();
            }
        }

        events.clear();

        Ok(())
    }

    fn new_priv(capacity: Option<usize>) -> io::Result<EventQueue> {
        let capacity = capacity.unwrap_or(MAX_EVENTS);
        let poll = Poll::new()?;

        let inner = Arc::new(Inner {
            poll,
            events: Mutex::new(Events::with_capacity(capacity)),
            scheduled: Mutex::new(HashMap::new()),
        });

        Ok(EventQueue {
            inner,
            token: AtomicUsize::new(0),
        })
    }
}

pub struct Inner {
    poll: Poll,
    events: Mutex<Events>,
    scheduled: Mutex<HashMap<Token, Arc<EventHandler>>>,
}

pub struct EventHandler {
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
