use crate::reactor::{self, PollResource};
use {
    futures::future,
    mio::{net::TcpStream as MioTcpStream, PollOpt, Ready},
    std::{
        io,
        net::{Shutdown, SocketAddr, TcpStream as StdTcpStream, ToSocketAddrs},
        time::Duration,
    },
};

pub struct TcpStream {
    io: PollResource<MioTcpStream>,
}

impl TcpStream {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let mut last_err = None;
        let addrs = addr.to_socket_addrs()?;

        for addr in addrs {
            match Self::connect_addr(&addr).await {
                Ok(stream) => return Ok(stream),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any addresses",
            )
        }))
    }

    pub async fn connect_addr(addr: &SocketAddr) -> io::Result<Self> {
        let stream = MioTcpStream::connect(&addr)?;

        // The stream will be writable when it's connected. We're assuming
        // the reactor is being polled here.
        let io_waker = reactor::register(&stream, Ready::writable(), PollOpt::edge())?;
        let io = PollResource::new(stream, Ready::readable() | Ready::writable(), PollOpt::edge())?;

        io.await_writable().await;

        match io.get_ref().take_error()? {
            Some(err) => Err(err),
            None => Ok(Self { io }),
        }
    }

    pub fn from_std(stream: StdTcpStream) -> io::Result<Self> {
        let stream = MioTcpStream::from_stream(stream)?;
        let io = PollResource::new(stream, Ready::readable() | Ready::writable(), PollOpt::edge())?;
        Ok(Self {
            io,
        })
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


