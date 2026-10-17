//! The ordered byte stream shared by desk registrations and audio frames.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::link::HubAddress;

/// A local socket or a TCP connection speaking the same desk protocol.
#[derive(Debug)]
pub enum HubStream {
    /// A desk on this machine.
    Unix(UnixStream),
    /// A desk reached over the network.
    Tcp(TcpStream),
}

impl HubStream {
    pub(crate) fn connect(address: &HubAddress, timeout: Duration) -> io::Result<Self> {
        match address {
            HubAddress::Socket(path) => UnixStream::connect(path).map(Self::Unix),
            HubAddress::Tcp(target) => {
                let addresses = super::link::host_and_port(target)
                    .and_then(|host_port| host_port.to_socket_addrs())
                    .map_err(|error| {
                        io::Error::new(
                            error.kind(),
                            format!("cannot resolve desk {target}: {error}"),
                        )
                    })?;
                let mut failures = Vec::new();
                let mut kind = io::ErrorKind::AddrNotAvailable;
                for address in addresses {
                    match TcpStream::connect_timeout(&address, timeout) {
                        Ok(stream) => {
                            stream.set_nodelay(true).map_err(|error| {
                                io::Error::new(
                                    error.kind(),
                                    format!("desk {target} ({address}): TCP_NODELAY: {error}"),
                                )
                            })?;
                            return Ok(Self::Tcp(stream));
                        }
                        Err(error) => {
                            kind = error.kind();
                            failures.push(format!("{address}: {error}"));
                        }
                    }
                }
                let reason = if failures.is_empty() {
                    "host resolved to no addresses".to_owned()
                } else {
                    failures.join("; ")
                };
                Err(io::Error::new(
                    kind,
                    format!("cannot connect to desk {target}: {reason}"),
                ))
            }
            HubAddress::Discover => Self::connect(&address.resolve()?, timeout),
        }
    }

    /// Set whether reads and writes wait for the socket.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_nonblocking(nonblocking),
            Self::Tcp(stream) => stream.set_nonblocking(nonblocking),
        }
    }

    /// Bound blocking reads during registration.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_read_timeout(timeout),
            Self::Tcp(stream) => stream.set_read_timeout(timeout),
        }
    }

    /// Bound blocking writes during registration.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_write_timeout(timeout),
            Self::Tcp(stream) => stream.set_write_timeout(timeout),
        }
    }

    /// Close one or both directions of the connection.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.shutdown(how),
            Self::Tcp(stream) => stream.shutdown(how),
        }
    }

    /// Duplicate the handle to the same connection.
    pub fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Unix(stream) => stream.try_clone().map(Self::Unix),
            Self::Tcp(stream) => stream.try_clone().map(Self::Tcp),
        }
    }
}

impl Read for &HubStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            HubStream::Unix(stream) => (&*stream).read(bytes),
            HubStream::Tcp(stream) => (&*stream).read(bytes),
        }
    }
}

impl Write for &HubStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            HubStream::Unix(stream) => (&*stream).write(bytes),
            HubStream::Tcp(stream) => (&*stream).write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            HubStream::Unix(stream) => (&*stream).flush(),
            HubStream::Tcp(stream) => (&*stream).flush(),
        }
    }
}

impl Read for HubStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        (&*self).read(bytes)
    }
}

impl Write for HubStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (&*self).write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl AsFd for HubStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Unix(stream) => stream.as_fd(),
            Self::Tcp(stream) => stream.as_fd(),
        }
    }
}
