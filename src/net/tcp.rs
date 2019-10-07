use crate::reactor::PollResource;
use {
    futures::{
        future,
        io::{AsyncRead, AsyncWrite},
        task::{Context, Poll},
    },
    mio::{net::TcpStream as MioTcpStream, PollOpt, Ready},
    std::{
        io::{self, Read, Write},
        net::{Shutdown, SocketAddr, TcpStream as StdTcpStream, ToSocketAddrs},
        pin::Pin,
        time::Duration,
    },
};

// Helpers

macro_rules! poll_rw {
    ($receiver:ident, $delegate:expr, $on_ok:pat, $ret:expr, $poll:expr) => {
        loop {
            match $delegate {
                $on_ok => return ::std::task::Poll::Ready($ret),
                Err(e) if !is_retry(&e) => return ::std::task::Poll::Ready(Err(e)),
                Err(e) if e.kind() == ::std::io::ErrorKind::WouldBlock => {
                    if let Err(e) = $receiver.as_ref().io.reregister(rw(), mio::PollOpt::edge()) {
                        return ::std::task::Poll::Ready(::std::result::Result::Err(e));
                    } else if $poll == ::std::task::Poll::Pending {
                        return ::std::task::Poll::Pending;
                    }
                }
                _ => (), // interrupted.
            }
        }
    }
}

// Returns a Ready value which is readable and writable.
fn rw() -> Ready {
    Ready::readable() | Ready::writable()
}

// Checks if the given error is WouldBlock or Interrupted.
fn is_retry(e: &io::Error) -> bool {
    use io::ErrorKind::{Interrupted, WouldBlock};
    e.kind() == WouldBlock || e.kind() == Interrupted
}

// Creates a new PollResource<TcpStream> with default values for registration.
fn resource_default(stream: MioTcpStream) -> io::Result<PollResource<MioTcpStream>> {
    PollResource::new(stream, rw(), PollOpt::edge())
}

pub struct TcpStream {
    io: PollResource<MioTcpStream>,
}

impl TcpStream {
    pub async fn connect(addr: &SocketAddr) -> io::Result<Self> {
        let stream = MioTcpStream::connect(&addr)?;

        // The stream will be writable when it's connected. We're assuming
        // the reactor is being polled here.
        let io = resource_default(stream)?;

        io.await_writable().await;

        match io.get_ref().take_error()? {
            Some(err) => Err(err),
            None => Ok(Self { io }),
        }
    }

    pub fn from_std(stream: StdTcpStream) -> io::Result<Self> {
        let stream = MioTcpStream::from_stream(stream)?;
        let io = resource_default(stream)?;
        Ok(Self { io })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    // Private because concurrently reading from or writing to
    // the same socket can lead to unexpected results, and there
    // should therefore be at most one reader _and_ one writer. One
    // should prefer to use the split() method.
    fn try_clone(&self) -> io::Result<TcpStream> {
        let stream = self.io.get_ref().try_clone()?;
        Ok(Self {
            io: PollResource::from_other(stream, &self.io),
        })
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.io.get_ref().shutdown(how)
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.io.get_ref().set_nodelay(nodelay)
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        self.io.get_ref().nodelay()
    }

    pub fn set_recv_buffer_size(&self, size: usize) -> io::Result<()> {
        self.io.get_ref().set_recv_buffer_size(size)
    }

    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        self.io.get_ref().recv_buffer_size()
    }

    pub fn set_send_buffer_size(&self, size: usize) -> io::Result<()> {
        self.io.get_ref().set_send_buffer_size(size)
    }

    pub fn send_buffer_size(&self) -> io::Result<usize> {
        self.io.get_ref().send_buffer_size()
    }

    pub fn set_keepalive(&self, keepalive: Option<Duration>) -> io::Result<()> {
        self.io.get_ref().set_keepalive(keepalive)
    }

    pub fn keepalive(&self) -> io::Result<Option<Duration>> {
        self.io.get_ref().keepalive()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.io.get_ref().set_ttl(ttl)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.io.get_ref().ttl()
    }

    pub fn set_only_v6(&self, only_v6: bool) -> io::Result<()> {
        self.io.get_ref().set_only_v6(only_v6)
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        self.io.get_ref().only_v6()
    }

    pub fn set_linger(&self, dur: Option<Duration>) -> io::Result<()> {
        self.io.get_ref().set_linger(dur)
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        self.io.get_ref().linger()
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        use io::ErrorKind::WouldBlock;

        loop {
            // Wait for data to come in on the socket if it wasn't there already.
            // If this was a spurious wakeup, we'll just loop back to here.
            self.io.await_readable().await;

            match self.io.get_ref().peek(buf) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() != WouldBlock => return Err(e),
                _ => (),
            }
        }
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        poll_rw!(self, self.as_mut().io.read(buf), Ok(n), Ok(n), self.as_ref().io.poll_readable(cx))
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        poll_rw!(self, self.as_mut().io.write(buf), Ok(n), Ok(n), self.as_ref().io.poll_writable(cx))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        poll_rw!(self, self.as_mut().io.flush(), Ok(_), Ok(()), self.as_ref().io.poll_writable(cx))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // This immediately closes the write side of the socket.
        Poll::Ready(self.shutdown(Shutdown::Write))
    }
}
