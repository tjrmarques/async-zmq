// TODO: remove this file once the reactor is migrated from `mio` to `polling` 3.x
// (see README). The Windows path here is a stepping-stone: `mio` 1.x has no
// public API for registering an arbitrary `RawSocket`, so we go through a
// `TcpStream` façade whose drop-close must be suppressed manually.

use std::io;

use mio::event::Source;
use mio::Token;
use zmq::Socket;

#[cfg(unix)]
use mio::unix::SourceFd;

#[cfg(windows)]
use mio::net::TcpStream;
#[cfg(windows)]
use std::os::windows::io::{FromRawSocket, IntoRawSocket};

pub(crate) struct ZmqSocket {
    // Field order matters: when `Drop::drop` below is skipped (should never
    // happen, but defence in depth), fields drop in declaration order. `stream`
    // must drop before `socket` so that mio's per-socket state is torn down
    // while the underlying SOCKET is still valid.
    #[cfg(windows)]
    stream: Option<TcpStream>,

    pub(crate) socket: Socket,
}

impl ZmqSocket {
    pub(crate) fn new(socket: Socket) -> Self {
        Self {
            #[cfg(windows)]
            stream: None,
            socket,
        }
    }
}

#[cfg(unix)]
impl Source for ZmqSocket {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        let fd = self.socket.get_fd()?;
        let mut source = SourceFd(&fd);
        registry.register(&mut source, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        let fd = self.socket.get_fd()?;
        let mut source = SourceFd(&fd);
        registry.reregister(&mut source, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        let fd = self.socket.get_fd()?;
        let mut source = SourceFd(&fd);
        registry.deregister(&mut source)
    }
}

#[cfg(windows)]
impl ZmqSocket {
    /// Build-or-reuse the `mio::net::TcpStream` façade used to register the
    /// libzmq signaler SOCKET with mio's reactor.
    ///
    /// The real owner of the SOCKET is libzmq's `signaler_t` (on Windows it's
    /// the read end of an emulated socketpair — see libzmq `src/signaler.cpp`).
    /// The `TcpStream` built here is a borrow-shaped façade whose only purpose
    /// is to reach mio's private `IoSource` registration machinery through the
    /// public `Source` trait; we MUST NOT let mio close the SOCKET on drop.
    /// Cleanup is handled by `impl Drop for ZmqSocket`, which consumes the
    /// stream via `IntoRawSocket::into_raw_socket` to release mio's state
    /// without calling `closesocket`.
    ///
    /// Double-AFD-polling note: libzmq's embedded wepoll already polls this
    /// SOCKET via its own AFD handle. Wepoll explicitly supports a socket being
    /// added to multiple epoll sets, so the second AFD poll from mio is safe
    /// for the loopback-TCP signaler's READABLE transitions.
    ///
    /// mio version risk: this relies on `mio::net::TcpStream` implementing
    /// `IntoRawSocket` such that `into_raw_socket` tears down the `IoSource`
    /// wrapper without calling `closesocket`. Verified against mio 1.0.x.
    #[allow(unsafe_code)]
    fn stream_mut(&mut self) -> io::Result<&mut TcpStream> {
        if self.stream.is_none() {
            // SAFETY: `get_fd()` returns the SOCKET owned by libzmq's signaler,
            // which libzmq configures non-blocking. We never let the returned
            // `TcpStream` reach its natural `Drop` — see `impl Drop for
            // ZmqSocket`.
            let stream = unsafe { TcpStream::from_raw_socket(self.socket.get_fd()?) };
            self.stream = Some(stream);
        }
        Ok(self.stream.as_mut().unwrap())
    }
}

#[cfg(windows)]
impl Source for ZmqSocket {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        registry.register(self.stream_mut()?, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        registry.reregister(self.stream_mut()?, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        registry.deregister(self.stream_mut()?)
    }
}

#[cfg(windows)]
impl Drop for ZmqSocket {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            // Release the SOCKET from mio's wrapper without closing it.
            // `into_raw_socket` consumes the `TcpStream`, tearing down mio's
            // per-socket bookkeeping, and returns the bare `RawSocket` which
            // we discard. libzmq's `signaler_t` — dropped next via
            // `self.socket` — is the real owner and will close it.
            let _raw = stream.into_raw_socket();
        }
    }
}
