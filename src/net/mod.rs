use std::net::{SocketAddr, ToSocketAddrs};

pub mod tcp;

// Just a convenience method for TcpListener::listen() and TcpStream::connect()
pub(crate) fn to_socket_addr<A: ToSocketAddrs>(addr: A) -> SocketAddr {}
