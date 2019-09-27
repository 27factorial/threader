use crate::reactor::{self, PollResource};
use {
    futures::future,
    mio::{net::TcpStream as MioTcpStream, Ready},
    std::{
        io,
        net::{SocketAddr, TcpStream as StdTcpStream, ToSocketAddrs},
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
        let io_waker = reactor::register(&stream, Ready::writable())?;
        let poll_resource = PollResource::new(stream, io_waker);

        poll_resource.await_writable().await;

        match poll_resource.get_ref().take_error()? {
            Some(err) => Err(err),
            None => Ok(Self { io: poll_resource }),
        }
    }
}
