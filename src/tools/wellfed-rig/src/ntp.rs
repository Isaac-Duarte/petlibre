//! A bounded NTP responder for the bench: answers the feeder's SNTP
//! requests with this host's clock, on an address and port the operator
//! names, for at most [`LIMIT`], in the foreground. It installs nothing and
//! changes no firewall or service: a port the host firewall does not admit
//! from the hotspot simply never sees a request.
//!
//! The answers are `wellfed_core::clock::ntp::serve`'s, the same the
//! simulator's tests use, as stratum 2 with a dispersion the operator
//! states for this host's own clock.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use wellfed_core::clock::ntp;

/// The longest a run may last.
pub const LIMIT: Duration = Duration::from_secs(3600);
/// How often a run looks at the clock and the operator's interrupt.
const TICK: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum NtpError {
    #[error("bind {listen}: {source}")]
    Bind {
        listen: SocketAddr,
        source: std::io::Error,
    },
    #[error("a run lasts at most {} s", LIMIT.as_secs())]
    TooLong,
    #[error("this host's clock is before 1970")]
    Clock,
    #[error("receive: {0}")]
    Receive(std::io::Error),
}

/// One answered request, for the operator's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answered {
    pub from: SocketAddr,
    pub unix_ms: u64,
}

fn now_ms() -> Result<u64, NtpError> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| NtpError::Clock)?;
    u64::try_from(since.as_millis()).map_err(|_| NtpError::Clock)
}

/// Answer requests on `listen` for `run`, or until `stop` says so; each
/// answer is handed to `answered`. The number answered.
pub fn serve(
    listen: SocketAddr,
    run: Duration,
    dispersion_ms: u32,
    mut stop: impl FnMut() -> bool,
    mut answered: impl FnMut(Answered),
) -> Result<u32, NtpError> {
    if run > LIMIT {
        return Err(NtpError::TooLong);
    }
    let socket = UdpSocket::bind(listen).map_err(|source| NtpError::Bind { listen, source })?;
    socket
        .set_read_timeout(Some(TICK))
        .map_err(NtpError::Receive)?;

    let end = Instant::now() + run;
    let mut count = 0;
    let mut request = [0; 128];
    while Instant::now() < end && !stop() {
        let (len, from) = match socket.recv_from(&mut request) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(NtpError::Receive(error)),
        };
        let received = now_ms()?;
        let transmit = now_ms()?;
        let Some(answer) = ntp::serve(&request[..len], received, transmit, 2, dispersion_ms) else {
            continue;
        };
        // A lost answer is the feeder's to retry.
        if socket.send_to(&answer, from).is_ok() {
            count += 1;
            answered(Answered {
                from,
                unix_ms: transmit,
            });
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_on_loopback_is_answered_with_this_hosts_clock() {
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let listen = probe.local_addr().unwrap();
        drop(probe);

        let server = std::thread::spawn(move || {
            let seen = std::cell::Cell::new(0);
            serve(
                listen,
                Duration::from_secs(5),
                10,
                || seen.get() > 0,
                |_| seen.set(seen.get() + 1),
            )
            .unwrap()
        });

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let nonce = 0x5eed;
        let mut answer = [0; 64];
        let fix = (0..20).find_map(|_| {
            client.send_to(&ntp::request(nonce), listen).unwrap();
            let (len, _) = client.recv_from(&mut answer).ok()?;
            ntp::answer(&answer[..len], nonce, 0, 1).ok()
        });
        let fix = fix.expect("an answer");
        assert!(fix.unix_ms.abs_diff(now_ms().unwrap()) < 1_000);
        assert_eq!(server.join().unwrap(), 1);
    }

    #[test]
    fn a_run_is_bounded() {
        assert!(matches!(
            serve(
                "127.0.0.1:0".parse().unwrap(),
                LIMIT + Duration::from_secs(1),
                0,
                || true,
                |_| {}
            ),
            Err(NtpError::TooLong)
        ));
    }
}
