// TODO: remove this file once an executor-native fast path lands.
#![allow(dead_code)]

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use polling::{Event, Events, Poller};
use slab::Slab;

#[cfg(unix)]
use std::os::fd::{BorrowedFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{BorrowedSocket, RawSocket};

#[cfg(unix)]
type RawHandle = RawFd;
#[cfg(windows)]
type RawHandle = RawSocket;

/// Borrow the platform-specific raw handle for use with `polling::Poller`.
///
/// SAFETY: caller must guarantee that `raw` refers to an open SOCKET that
/// outlives the borrow. In our reactor this holds because `ZmqSocket::Drop`
/// calls `Reactor::deregister` while still holding `zmq::Socket` (and hence
/// the SOCKET) — see field declaration order + explicit `Drop` on `ZmqSocket`.
#[cfg(unix)]
#[allow(unsafe_code)]
unsafe fn borrow(raw: RawHandle) -> BorrowedFd<'static> {
    // SAFETY: forwarded from caller's contract.
    unsafe { BorrowedFd::borrow_raw(raw) }
}
#[cfg(windows)]
#[allow(unsafe_code)]
unsafe fn borrow(raw: RawHandle) -> BorrowedSocket<'static> {
    // SAFETY: forwarded from caller's contract.
    unsafe { BorrowedSocket::borrow_raw(raw) }
}

/// Data associated with a registered I/O handle.
#[derive(Debug)]
struct Entry {
    /// Slab key (also the polling event key).
    key: usize,

    /// Raw underlying handle (FD on Unix, SOCKET on Windows).
    raw: RawHandle,

    /// Tasks that are blocked on reading from this I/O handle.
    readers: Mutex<Readers>,

    /// Tasks that are blocked on writing to this I/O handle.
    writers: Mutex<Writers>,
}

/// The set of `Waker`s interested in read readiness.
#[derive(Debug)]
struct Readers {
    /// Flag indicating read readiness.
    /// (cf. `ZmqSocket::poll_read_ready`)
    ready: bool,
    /// The `Waker`s blocked on reading.
    wakers: Vec<Waker>,
}

/// The set of `Waker`s interested in write readiness.
#[derive(Debug)]
struct Writers {
    /// Flag indicating write readiness.
    /// (cf. `ZmqSocket::poll_write_ready`)
    ready: bool,
    /// The `Waker`s blocked on writing.
    wakers: Vec<Waker>,
}

/// The state of the global networking driver.
struct Reactor {
    /// A `polling` instance that polls for new events.
    poller: Poller,

    /// A collection of registered I/O handles.
    entries: Mutex<Slab<Arc<Entry>>>,
}

impl Reactor {
    /// Returns the global reactor, lazily starting it on first use.
    fn get() -> &'static Self {
        static REACTOR: OnceLock<Reactor> = OnceLock::new();
        REACTOR.get_or_init(|| {
            let reactor = Reactor {
                poller: Poller::new().expect("cannot create poller"),
                entries: Mutex::new(Slab::new()),
            };
            std::thread::Builder::new()
                .name("async-zmq/reactor".to_string())
                .spawn(main_loop)
                .expect("cannot start reactor thread");
            reactor
        })
    }

    /// Registers an open SOCKET with the poller in default `Oneshot` mode
    /// (the only mode supported by all backends, including Windows IOCP/AFD).
    ///
    /// SAFETY: caller MUST call `deregister` (via `ZmqSocket::Drop`) before
    /// the SOCKET is closed.
    #[allow(unsafe_code)]
    unsafe fn register(&self, raw: RawHandle) -> io::Result<Arc<Entry>> {
        let mut entries = self.entries.lock().unwrap();

        // Reserve a vacant spot in the slab and use its key as the event key.
        let vacant = entries.vacant_entry();
        let key = vacant.key();

        let entry = Arc::new(Entry {
            key,
            raw,
            readers: Mutex::new(Readers {
                ready: false,
                wakers: Vec::new(),
            }),
            writers: Mutex::new(Writers {
                ready: false,
                wakers: Vec::new(),
            }),
        });
        vacant.insert(entry.clone());

        // SAFETY: per method-level safety contract — caller guarantees the
        // SOCKET stays open until `deregister`. `Poller::add` accepts the
        // raw handle directly via the `AsRawSource for RawFd/RawSocket` impl.
        unsafe {
            self.poller.add(raw, Event::all(key))?;
        }

        Ok(entry)
    }

    /// Deregisters an I/O event source associated with an entry.
    fn deregister(&self, entry: &Entry) -> io::Result<()> {
        // SAFETY: borrow lifetime ends in this call; `raw` is still valid
        // because `ZmqSocket::Drop` runs `deregister` BEFORE the
        // `zmq::Socket` field drops (which is what closes the SOCKET).
        #[allow(unsafe_code)]
        unsafe {
            self.poller.delete(borrow(entry.raw))?;
        }

        // Remove the entry associated with the I/O object.
        self.entries.lock().unwrap().remove(entry.key);

        Ok(())
    }
}

/// Waits on the poller for new events and wakes up tasks blocked on I/O handles.
fn main_loop() {
    let reactor = Reactor::get();
    let mut events = Events::new();

    loop {
        events.clear();
        // Block on the poller until at least one new event comes in.
        if reactor.poller.wait(&mut events, None).is_err() {
            continue;
        }

        // Lock the entire entry table while we're processing new events.
        // Holding this lock also blocks `ZmqSocket::Drop`, which guarantees
        // that any `raw` we re-arm below still refers to an open SOCKET.
        let entries = reactor.entries.lock().unwrap();

        for ev in events.iter() {
            if let Some(entry) = entries.get(ev.key) {
                // Wake up reader tasks blocked on this I/O handle.
                if ev.readable {
                    let mut readers = entry.readers.lock().unwrap();
                    readers.ready = true;
                    for w in readers.wakers.drain(..) {
                        w.wake();
                    }
                }

                // Wake up writer tasks blocked on this I/O handle.
                if ev.writable {
                    let mut writers = entry.writers.lock().unwrap();
                    writers.ready = true;
                    for w in writers.wakers.drain(..) {
                        w.wake();
                    }
                }

                // Re-arm: oneshot fires only once per registration.
                // SAFETY: the entry is still in the slab and the SOCKET is
                // still open because `ZmqSocket::Drop` is blocked on the
                // outer `entries` lock we hold here.
                #[allow(unsafe_code)]
                let _ = unsafe {
                    reactor
                        .poller
                        .modify(borrow(entry.raw), Event::all(entry.key))
                };
            }
        }
    }
}

/// An async-friendly handle wrapping a `zmq::Socket` registered with the reactor.
pub(crate) struct ZmqSocket {
    /// Reactor entry. Field order matters: declared before `socket` so that
    /// even if the explicit `Drop` is somehow skipped, deregistration logic
    /// has a chance to observe the still-open SOCKET. The contract is that
    /// `Drop::drop` runs `deregister` before `socket` is dropped.
    entry: Arc<Entry>,

    /// The underlying `zmq::Socket`. Dropped after `entry` per Rust's field
    /// drop order, which closes the SOCKET only after deregistration.
    socket: zmq::Socket,
}

impl ZmqSocket {
    /// Wrap a `zmq::Socket` and register it with the global reactor.
    pub(crate) fn new(socket: zmq::Socket) -> Self {
        // `zmq-0.10`'s `get_fd()` returns `RawFd` on every platform (the
        // macro yields `ZMQ_FD as RawFd`). On Windows we cast to `RawSocket`
        // for use with `BorrowedSocket`/`Poller` — mirrors what
        // `zmq::Socket`'s `AsRawSocket` impl does.
        let fd = socket.get_fd().expect("zmq socket has no fd");
        #[cfg(unix)]
        let raw: RawHandle = fd;
        #[cfg(windows)]
        let raw: RawHandle = fd as RawSocket;

        // SAFETY: `ZmqSocket::Drop` calls `Reactor::deregister` BEFORE
        // `zmq::Socket` drops (which closes the SOCKET), upholding the
        // contract on `Poller::add`.
        #[allow(unsafe_code)]
        let entry = unsafe { Reactor::get().register(raw) }
            .expect("cannot register zmq socket with reactor");

        Self { entry, socket }
    }

    /// Returns a reference to the inner `zmq::Socket`.
    pub(crate) fn get_ref(&self) -> &zmq::Socket {
        &self.socket
    }

    /// Polls the inner I/O source for a non-blocking read operation.
    ///
    /// If the operation returns an error of the `io::ErrorKind::WouldBlock` kind, the current task
    /// will be registered for wake-up when the I/O source becomes readable.
    pub(crate) fn poll_read_with<'a, F, R>(
        &'a self,
        cx: &mut Context<'_>,
        mut f: F,
    ) -> Poll<io::Result<R>>
    where
        F: FnMut(&'a zmq::Socket) -> io::Result<R>,
    {
        // If the operation isn't blocked, return its result.
        match f(&self.socket) {
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            res => return Poll::Ready(res),
        }

        // Lock the waker list.
        let mut readers = self.entry.readers.lock().unwrap();

        // Try running the operation again.
        match f(&self.socket) {
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            res => return Poll::Ready(res),
        }

        // Register the task if it isn't registered already.
        if readers.wakers.iter().all(|w| !w.will_wake(cx.waker())) {
            readers.wakers.push(cx.waker().clone());
        }

        readers.ready = false;

        Poll::Pending
    }

    /// Polls the inner I/O source for a non-blocking write operation.
    ///
    /// If the operation returns an error of the `io::ErrorKind::WouldBlock` kind, the current task
    /// will be registered for wake-up when the I/O source becomes writable.
    pub(crate) fn poll_write_with<'a, F, R>(
        &'a self,
        cx: &mut Context<'_>,
        mut f: F,
    ) -> Poll<io::Result<R>>
    where
        F: FnMut(&'a zmq::Socket) -> io::Result<R>,
    {
        // If the operation isn't blocked, return its result.
        match f(&self.socket) {
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            res => return Poll::Ready(res),
        }

        // Lock the waker list.
        let mut writers = self.entry.writers.lock().unwrap();

        // Try running the operation again.
        match f(&self.socket) {
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            res => return Poll::Ready(res),
        }

        // Register the task if it isn't registered already.
        if writers.wakers.iter().all(|w| !w.will_wake(cx.waker())) {
            writers.wakers.push(cx.waker().clone());
        }

        writers.ready = false;

        Poll::Pending
    }

    /// Polls the inner I/O source until a non-blocking read can be performed.
    ///
    /// If non-blocking reads are currently not possible, the `Waker`
    /// will be saved and notified when it can read non-blocking
    /// again.
    #[allow(dead_code)]
    pub(crate) fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<()> {
        // Lock the waker list.
        let mut readers = self.entry.readers.lock().unwrap();
        if readers.ready {
            return Poll::Ready(());
        }
        // Register the task if it isn't registered already.
        if readers.wakers.iter().all(|w| !w.will_wake(cx.waker())) {
            readers.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }

    /// Polls the inner I/O source until a non-blocking write can be performed.
    ///
    /// If non-blocking writes are currently not possible, the `Waker`
    /// will be saved and notified when it can write non-blocking
    /// again.
    pub(crate) fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<()> {
        // Lock the waker list.
        let mut writers = self.entry.writers.lock().unwrap();
        if writers.ready {
            return Poll::Ready(());
        }
        // Register the task if it isn't registered already.
        if writers.wakers.iter().all(|w| !w.will_wake(cx.waker())) {
            writers.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for ZmqSocket {
    fn drop(&mut self) {
        // Deregister BEFORE `zmq::Socket` drops (which closes the SOCKET).
        // `main_loop` and `Drop` both lock `entries`, so they serialize.
        let _ = Reactor::get().deregister(&self.entry);
    }
}

impl fmt::Debug for ZmqSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZmqSocket").field("entry", &self.entry).finish()
    }
}
