use std::io;

use mio::event::Source;
use mio::Token;
use zmq::Socket;

pub(crate) struct ZmqSocket(pub(crate) Socket);

#[cfg(unix)]
use mio::unix::SourceFd;

#[cfg(unix)]
impl Source for ZmqSocket {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        let fd = self.0.get_fd()?;
        let mut source = SourceFd(&fd);
        registry.register(&mut source, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        let fd = self.0.get_fd()?;
        let mut source = SourceFd(&fd);
        registry.reregister(&mut source, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        let fd = self.0.get_fd()?;
        let mut source = SourceFd(&fd);
        registry.deregister(&mut source)
    }
}

#[cfg(not(unix))]
use mio::net::TcpStream;

#[cfg(windows)]
use std::os::windows::io::FromRawSocket;
#[cfg(windows)]
impl Source for ZmqSocket {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        // FIXME: This hack is required because `mio::::io_source::IoSource` is private
        // Using any of the net Io types to wrap the RawSocket works

        // SAFETY: This is safe, as we never use the wrapping type: the register method just uses the underlying RawFd/RawSocket, without looking at
        // any other field
        #![allow(unsafe_code)]
        let mut stream = unsafe { TcpStream::from_raw_socket(self.0.get_fd()?) };
        registry.register(&mut stream, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        // FIXME: This hack is required because `mio::::io_source::IoSource` is private
        // Using any of the net Io types to wrap the RawSocket

        // SAFETY: This is safe, as we never use the wrapping type: the register method just uses the underlying RawFd/RawSocket, without looking at
        // any other field
        #![allow(unsafe_code)]
        let mut stream = unsafe { TcpStream::from_raw_socket(self.0.get_fd()?) };
        registry.reregister(&mut stream, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        // FIXME: This hack is required because `mio::::io_source::IoSource` is private
        // Using any of the net Io types to wrap the RawSocket

        // SAFETY: This is safe, as we never use the wrapping type: the register method just uses the inner RawFd/RawSocket
        #![allow(unsafe_code)]
        let mut stream = unsafe { TcpStream::from_raw_socket(self.0.get_fd()?) };
        registry.deregister(&mut stream)
    }
}

#[cfg(all(not(windows), not(unix)))]
impl Source for ZmqSocket {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        // FIXME: This hack is required because `mio::::io_source::IoSource` is private
        // Using any of the net Io types to wrap the RawFd works

        // SAFETY: This is safe, as we never use the wrapping type: the register method just uses the underlying RawFd/RawSocket, without looking at
        // any other field
        #![allow(unsafe_code)]
        let mut stream = unsafe { TcpStream::from_raw_fd(self.0.get_fd()?) };
        registry.register(&mut stream, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        // FIXME: This hack is required because `mio::::io_source::IoSource` is private
        // Using any of the net Io types to wrap the RawFd

        // SAFETY: This is safe, as we never use the wrapping type: the register method just uses the underlying RawFd/RawSocket, without looking at
        // any other field
        #![allow(unsafe_code)]
        let mut stream = unsafe { TcpStream::from_raw_fd(self.0.get_fd()?) };
        registry.reregister(&mut stream, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        // FIXME: This hack is required because `mio::::io_source::IoSource` is private
        // Using any of the net Io types to wrap the RawFd

        // SAFETY: This is safe, as we never use the wrapping type: the register method just uses the inner RawFd/RawSocket
        #![allow(unsafe_code)]
        let mut stream = unsafe { TcpStream::from_raw_fd(self.0.get_fd()?) };
        registry.deregister(&mut stream)
    }
}
