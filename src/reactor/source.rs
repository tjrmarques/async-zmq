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
