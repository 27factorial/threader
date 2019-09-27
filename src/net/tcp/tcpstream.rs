use crate::reactor::{self, PollResource};
use {
    futures::{
        future,
    },
    mio::{
        Ready,
        net::TcpStream as MioTcpStream
    },
    std::{
        io,
        net::{TcpStream as StdTcpStream, SocketAddr}
    },
};

pub struct TcpStream {
    io: PollResource<MioTcpStream>,
}

impl TcpStream {
    pub async fn connect(addr: &SocketAddr) -> io::Result<TcpStream> {
        let stream = MioTcpStream::connect(&addr)?;

        // The stream will be writable when it's connected. We're assuming
        // the reactor is being turned here.
        let io_waker = reactor::register(&stream, Ready::writable())?;
        let poll_resource = PollResource::new(stream, io_waker);

        poll_resource.await_writable().await;

        match poll_resource.get_ref().take_error()? {
            Some(err) => Err(err),
            None => Ok(TcpStream {
                io: poll_resource,
            }),
        }
    }
}
