//! Sending stock 1.1.15's setup frame to a feeder in setup mode. The frame
//! itself is `wellfed_client::install`'s, the one the installer sends; this
//! is the bench's way to send it alone (`setup-send`).

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use anyhow::{Context, Result};
use wellfed_client::install::SetupFrame;

/// Send one frame to `to` and wait up to `wait` for the feeder's answer,
/// which is returned as it came. Never resent: stock applies a frame once.
pub fn send(setup: &SetupFrame, to: SocketAddr, wait: Duration) -> Result<Option<Vec<u8>>> {
    let frame = setup.encode()?;
    let socket = UdpSocket::bind("0.0.0.0:0").context("open a UDP socket")?;
    socket.set_read_timeout(Some(wait))?;
    socket
        .send_to(&frame, to)
        .with_context(|| format!("send the setup frame to {to}"))?;

    let mut answer = [0; 256];
    match socket.recv_from(&mut answer) {
        Ok((n, _)) => Ok(Some(answer[..n].to_vec())),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error).context("read the feeder's answer"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_frame_goes_out_and_the_answer_comes_back() {
        let setup = SetupFrame {
            ssid: "Lab".into(),
            password: "secret12".into(),
            host: "10.42.103.1".into(),
            port: 1883,
            id: "1234".into(),
        };
        let feeder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = feeder.local_addr().unwrap();
        let echo = std::thread::spawn(move || {
            let mut buf = [0; 256];
            let (n, from) = feeder.recv_from(&mut buf).unwrap();
            feeder.send_to(&[0xaa, 0x01], from).unwrap();
            buf[..n].to_vec()
        });

        let answer = send(&setup, to, Duration::from_secs(2)).unwrap();
        assert_eq!(answer, Some(vec![0xaa, 0x01]));
        assert_eq!(echo.join().unwrap(), setup.encode().unwrap());
    }
}
