use crate::reactor::observer::Observer;
use futures::{
    io::{AsyncRead, AsyncWrite},
    task::{Context, Poll},
};
use mio::{
    net::{TcpListener as MioTcpListener, TcpStream as MioTcpStream},
    PollOpt, Ready,
};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream},
    pin::Pin,
    time::Duration,
};

// =============== HELPERS =============== //
macro_rules! poll_rw {
    ($receiver:ident, $delegate:expr, $on_ok:pat, $ret:expr, $poll:expr) => {
        loop {
            match $delegate {
                $on_ok => return ::std::task::Poll::Ready($ret),
                ::std::result::Result::Err(e) if !is_retry(&e) => {
                    return ::std::task::Poll::Ready(Err(e))
                }
                ::std::result::Result::Err(e) if e.kind() == ::std::io::ErrorKind::WouldBlock => {
                    if let Err(e) = $receiver
                        .as_ref()
                        .observer
                        .reregister(rw(), mio::PollOpt::edge())
                    {
                        return ::std::task::Poll::Ready(::std::result::Result::Err(e));
                    } else if $poll == ::std::task::Poll::Pending {
                        return ::std::task::Poll::Pending;
                    }
                }
                _ => (), // interrupted.
            }
        }
    };
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

// Creates a new Observer<MioTcpStream> with default values for registration.
fn observer_stream(stream: MioTcpStream) -> io::Result<Observer<MioTcpStream>> {
    Observer::new(stream, rw(), PollOpt::edge())
}

// Creates a new Observer<MioTcpListener> with default values for registration.
fn observer_listener(listener: MioTcpListener) -> io::Result<Observer<MioTcpListener>> {
    Observer::new(listener, rw(), PollOpt::edge())
}

// =============== TcpStream =============== //

#[derive(Debug)]
pub struct TcpStream {
    observer: Observer<MioTcpStream>,
}

impl TcpStream {
    pub async fn connect(addr: &SocketAddr) -> io::Result<Self> {
        let stream = MioTcpStream::connect(&addr)?;
        let io = observer_stream(stream)?;

        dbg!(io.await_writable().await);

        match io.get_ref().take_error()? {
            Some(err) => Err(err),
            None => Ok(Self { observer: io }),
        }
    }

    pub fn from_std(stream: StdTcpStream) -> io::Result<Self> {
        let stream = MioTcpStream::from_stream(stream)?;
        let io = observer_stream(stream)?;
        Ok(Self { observer: io })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.observer.get_ref().peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.observer.get_ref().local_addr()
    }

    // Private because concurrently reading from or writing to
    // the same socket can lead to unexpected results, and there
    // should therefore be at most one reader _and_ one writer. One
    // should prefer to use the split() method.
    fn try_clone(&self) -> io::Result<TcpStream> {
        let stream = self.observer.get_ref().try_clone()?;
        Ok(Self {
            observer: Observer::from_other(stream, &self.observer),
        })
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.observer.get_ref().shutdown(how)
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.observer.get_ref().set_nodelay(nodelay)
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        self.observer.get_ref().nodelay()
    }

    pub fn set_recv_buffer_size(&self, size: usize) -> io::Result<()> {
        self.observer.get_ref().set_recv_buffer_size(size)
    }

    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        self.observer.get_ref().recv_buffer_size()
    }

    pub fn set_send_buffer_size(&self, size: usize) -> io::Result<()> {
        self.observer.get_ref().set_send_buffer_size(size)
    }

    pub fn send_buffer_size(&self) -> io::Result<usize> {
        self.observer.get_ref().send_buffer_size()
    }

    pub fn set_keepalive(&self, keepalive: Option<Duration>) -> io::Result<()> {
        self.observer.get_ref().set_keepalive(keepalive)
    }

    pub fn keepalive(&self) -> io::Result<Option<Duration>> {
        self.observer.get_ref().keepalive()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.observer.get_ref().set_ttl(ttl)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.observer.get_ref().ttl()
    }

    pub fn set_only_v6(&self, only_v6: bool) -> io::Result<()> {
        self.observer.get_ref().set_only_v6(only_v6)
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        self.observer.get_ref().only_v6()
    }

    pub fn set_linger(&self, dur: Option<Duration>) -> io::Result<()> {
        self.observer.get_ref().set_linger(dur)
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        self.observer.get_ref().linger()
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        use io::ErrorKind::WouldBlock;

        loop {
            match self.observer.get_ref().peek(buf) {
                Ok(n) => return Ok(n),
                Err(e) if !is_retry(&e) => return Err(e),
                Err(e) if e.kind() == WouldBlock => {
                    self.observer.reregister(rw(), PollOpt::edge())?;
                    self.observer.await_readable().await;
                }
                _ => (), // interrupted.
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
        poll_rw!(
            self,
            self.as_mut().observer.read(buf),
            Ok(n),
            Ok(n),
            self.as_ref().observer.poll_readable(cx)
        )
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        poll_rw!(
            self,
            self.as_mut().observer.write(buf),
            Ok(n),
            Ok(n),
            self.as_ref().observer.poll_writable(cx)
        )
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        poll_rw!(
            self,
            self.as_mut().observer.flush(),
            Ok(_),
            Ok(()),
            self.as_ref().observer.poll_writable(cx)
        )
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // This immediately closes the write side of the socket.
        Poll::Ready(self.shutdown(Shutdown::Write))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use crate::thread_pool::ThreadPool;
    use crossbeam::channel;
    use once_cell::sync::Lazy;

    static EX: Lazy<ThreadPool> = Lazy::new(|| ThreadPool::new().unwrap());

    #[test]
    fn connect_test() {
        let (tx, rx) = channel::unbounded();

        EX.spawn(async move {
            let addr = "173.245.52.164:80".parse().unwrap();
            let stream = TcpStream::connect(&addr).await;
            let _ = dbg!(stream);
            tx.send(0).unwrap();
        });

        assert_eq!(rx.recv(), Ok(0));
    }
}
