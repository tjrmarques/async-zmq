//! Smoke test: construct and drop Pub and Sub sockets.
//!
//! This is the minimal reproducer for a whole class of reactor/ownership bugs
//! that fire on the first ZMQ internal command dispatch (which happens during
//! `bind()`/`connect()`, not during message I/O). No network handshake or
//! async runtime is needed to exercise it.

use async_zmq::{publish, subscribe, Message, Result};

#[test]
fn construct_and_drop_pub_sub() -> Result<()> {
    let pub_socket =
        publish::<std::vec::IntoIter<Message>, Message>("tcp://127.0.0.1:0")?.bind()?;
    let sub_socket = subscribe("tcp://127.0.0.1:1")?.connect()?;
    drop(pub_socket);
    drop(sub_socket);
    Ok(())
}
