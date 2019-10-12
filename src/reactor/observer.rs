use super::{Handle, IoWaker};
use futures::future;
use mio::{Evented, PollOpt, Ready};
use std::{
    io::{self, Read, Write},
    sync::{atomic::Ordering, Arc},
    task::Context,
};

/// A type that "observes" changes in a resource's state
/// from the reactor, waking from the `IoWaker` if a change
/// is detected.
pub struct Observer<R: Evented> {
    resource: R,
    io_waker: Arc<IoWaker>,
    handle: Handle,
}

impl<R: Evented> Observer<R> {
    /// Creates a new instance of `Observer`
    pub fn new(resource: R, interest: Ready, opts: PollOpt) -> io::Result<Self> {
        Self::new_priv(resource, interest, opts, None)
    }

    /// Creates a new instance of `Observer` with the given reactor handle.
    pub fn with_handle(
        resource: R,
        interest: Ready,
        opts: PollOpt,
        handle: Handle,
    ) -> io::Result<Self> {
        Self::new_priv(resource, interest, opts, Some(handle))
    }

    /// Creates an `Observer` from another using a different resource but the same
    /// `IoWaker` and `Handle`.
    pub fn from_other(resource: R, other: &Self) -> Self {
        Self {
            resource,
            io_waker: other.io_waker(),
            handle: other.handle(),
        }
    }

    /// Returns a reference to the underlying resource.
    pub fn get_ref(&self) -> &R {
        &self.resource
    }

    /// Returns a mutable reference to the underlying resource.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.resource
    }

    /// Clones the internal `IoWaker` and returns it.
    pub fn io_waker(&self) -> Arc<IoWaker> {
        Arc::clone(&self.io_waker)
    }

    /// Clones the internal `Handle` and returns it.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Reregisters the underlying resource with the internal `Handle`.
    pub fn reregister(&self, interest: Ready, opts: PollOpt) -> io::Result<()> {
        self.handle
            .reregister(&self.resource, &self.io_waker, interest, opts)
    }

    /// Deregisters a resource from the reactor that drives it.
    pub fn deregister(&self) -> io::Result<()> {
        self.handle.deregister(&self.resource, &self.io_waker)
    }

    /// Polls for read readiness.
    pub fn poll_readable(&self, cx: &mut Context<'_>) -> futures::Poll<Ready> {
        let readiness = Ready::from_usize(self.io_waker.readiness.load(Ordering::SeqCst));

        if readiness.is_readable() {
            self.io_waker.clear_read();
            futures::Poll::Ready(readiness)
        } else {
            self.io_waker.register_read(cx.waker());
            futures::Poll::Pending
        }
    }

    /// Polls for write readiness.
    pub fn poll_writable(&self, cx: &mut Context<'_>) -> futures::Poll<Ready> {
        let readiness = Ready::from_usize(self.io_waker.readiness.load(Ordering::SeqCst));

        if readiness.is_writable() {
            self.io_waker.clear_write();
            futures::Poll::Ready(readiness)
        } else {
            self.io_waker.register_write(cx.waker());
            futures::Poll::Pending
        }
    }

    /// A convenience method for wrapping poll_readable in a future.
    pub async fn await_readable(&self) -> Ready {
        future::poll_fn(|cx| self.poll_readable(cx)).await
    }

    /// A convenience method for wrapping poll_writable in a future.
    pub async fn await_writable(&self) -> Ready {
        future::poll_fn(|cx| self.poll_writable(cx)).await
    }

    fn new_priv(
        resource: R,
        interest: Ready,
        opts: PollOpt,
        handle: Option<Handle>,
    ) -> io::Result<Self> {
        let handle = handle.unwrap_or_else(super::handle);
        let io_waker = handle.register(&resource, interest, opts)?;

        Ok(Self {
            resource,
            io_waker,
            handle,
        })
    }
}

impl<E: Evented> Drop for Observer<E> {
    fn drop(&mut self) {
        // it doesn't really matter if an error happens here, since
        // the resource won't be used later anyway.
        let _ = self.deregister();
    }
}

impl<E: Evented + Read> Read for Observer<E> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.resource.read(buf)
    }
}

impl<E: Evented + Write> Write for Observer<E> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.resource.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.resource.flush()
    }
}
