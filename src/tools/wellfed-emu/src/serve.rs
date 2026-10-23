//! Serve an emulated helper to a `wellfed-flash` process over a Unix socket.
//!
//! `wellfed-flash` reads `WELLFED_FLASH_EMULATOR` and speaks this protocol
//! (its `serial::Emulated` port) in place of a serial device. A request is an
//! op byte and a little-endian `u32`: `W` writes that many following bytes,
//! `R` reads up to that many (running the helper until it has output), `I`
//! reports buffered output and `F` flushes. Each reply is a little-endian
//! `u32` length and that many bytes.
//!
//! Unicorn is single-threaded, so the server runs on the caller's thread
//! until the client process exits. A helper error closes the connection: the
//! client sees a lost transport and stops, as it would on a real fault.

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::Duration;

use crate::Error;
use crate::helper::Helper;

/// Serve `listener` until `child` exits; returns its status and the first
/// helper error, if any.
pub fn until_exit(
    listener: &UnixListener,
    helper: &mut Helper<'_>,
    child: &mut Child,
) -> io::Result<(ExitStatus, Option<Error>)> {
    listener.set_nonblocking(true)?;
    let mut failure = None;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                if let Err(error) = connection(stream, helper) {
                    failure.get_or_insert(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Ok((status, failure));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}

/// One client connection, until it closes or the helper fails.
fn connection(mut stream: UnixStream, helper: &mut Helper<'_>) -> Result<(), Error> {
    let mut header = [0u8; 5];
    while stream.read_exact(&mut header).is_ok() {
        let [op, argument @ ..] = header;
        let argument = u32::from_le_bytes(argument) as usize;
        let reply = match op {
            b'W' => {
                let mut data = vec![0; argument];
                if stream.read_exact(&mut data).is_err() {
                    return Ok(());
                }
                count(helper.write(&data))
            }
            b'R' => helper.read(argument)?,
            b'I' => count(helper.in_waiting()),
            _ => count(0),
        };
        let mut framed = count(reply.len());
        framed.extend_from_slice(&reply);
        if stream.write_all(&framed).is_err() {
            return Ok(());
        }
    }
    Ok(())
}

fn count(value: usize) -> Vec<u8> {
    u32::try_from(value)
        .unwrap_or(u32::MAX)
        .to_le_bytes()
        .to_vec()
}
