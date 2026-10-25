//! Stock 1.1.15's setup frame, as the vendor app sends it to a feeder in
//! setup mode (static analysis; [consumer installation]). One plaintext UDP
//! datagram to the feeder's setup access point carries the Wi-Fi network
//! and the MQTT broker, so a stock feeder can be pointed at this server
//! with no traffic redirect. Type 3 sets both; PLAF103 frames carry no
//! checksum and no encryption.
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use anyhow::{Context, Result, ensure};

/// Where the feeder's setup access point listens (`Ap_Config_Udp_Thread`).
pub const SETUP_ADDRESS: &str = "192.168.1.1:17484";
/// Wi-Fi and server together, what the app sends for PLAF103.
const TYPE_BOTH: u8 = 3;
const END: u8 = 0xff;

/// The limits stock's parser and `Set_Dev_Info` enforce.
const SSID_MAX: usize = 32;
const PASSWORD_MAX: usize = 64;
const SERVER_MAX: usize = 100;
const ID_MAX: usize = 30;
/// Stock copies at most four digits after the `:`.
const PORT_MAX: u16 = 9999;

/// What one setup frame tells stock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub ssid: String,
    pub password: String,
    /// The broker's IPv4 address or name, without a port.
    pub host: String,
    pub port: u16,
    /// The account's numeric ID (ASCII digits, nonzero), which stock keeps
    /// beside the broker and requires.
    pub id: String,
}

impl Setup {
    /// The type-3 frame: `DL`, type, checksum flag 0, 0, payload length,
    /// then length-prefixed SSID, password, `host:port` and ID, then `0xFF`.
    pub fn frame(&self) -> Result<Vec<u8>> {
        let server = format!("{}:{}", self.host, self.port);
        ensure!(
            !self.ssid.is_empty() && self.ssid.len() <= SSID_MAX,
            "SSID must be 1–32 bytes"
        );
        ensure!(
            self.password.len() <= PASSWORD_MAX,
            "Password must be at most 64 bytes"
        );
        ensure!(
            !self.host.is_empty() && !self.host.contains(':') && server.len() <= SERVER_MAX,
            "Host must be set, without a port, and host:port at most 100 bytes"
        );
        ensure!(
            (1..=PORT_MAX).contains(&self.port),
            "Port must be 1–9999: stock reads four digits"
        );
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= ID_MAX
                && self.id.bytes().all(|b| b.is_ascii_digit())
                && self.id.bytes().any(|b| b != b'0'),
            "ID must be 1–30 ASCII digits, not zero"
        );

        let fields = [
            self.ssid.as_bytes(),
            self.password.as_bytes(),
            server.as_bytes(),
            self.id.as_bytes(),
        ];
        let length: usize = fields.iter().map(|field| 1 + field.len()).sum();
        let mut frame = vec![b'D', b'L', TYPE_BOTH, 0, 0];
        frame.push(u8::try_from(length).context("frame too long")?);
        for field in fields {
            frame.push(u8::try_from(field.len()).context("field too long")?);
            frame.extend_from_slice(field);
        }
        frame.push(END);
        Ok(frame)
    }
}

/// Send one frame to `to` and wait up to `wait` for the feeder's answer,
/// which is returned as it came. Never resent: stock applies a frame once.
pub fn send(setup: &Setup, to: SocketAddr, wait: Duration) -> Result<Option<Vec<u8>>> {
    let frame = setup.frame()?;
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

    fn setup() -> Setup {
        Setup {
            ssid: "Lab".into(),
            password: "secret12".into(),
            host: "10.42.103.1".into(),
            port: 1883,
            id: "1234".into(),
        }
    }

    #[test]
    fn the_frame_is_what_stock_s_type_3_parser_reads() {
        let frame = setup().frame().unwrap();
        let mut want = vec![b'D', b'L', 3, 0, 0, 4 + 3 + 8 + 16 + 4];
        for field in ["Lab", "secret12", "10.42.103.1:1883", "1234"] {
            want.push(field.len() as u8);
            want.extend_from_slice(field.as_bytes());
        }
        want.push(0xff);
        assert_eq!(frame, want);

        // The parser's own length check: byte 5 is every prefix plus string.
        let lengths: usize = [3, 8, 16, 4].iter().sum::<usize>() + 4;
        assert_eq!(usize::from(frame[5]), lengths);
        assert_eq!(*frame.last().unwrap(), 0xff);
    }

    #[test]
    fn values_stock_would_misread_are_refused() {
        let refused = |change: fn(&mut Setup)| {
            let mut s = setup();
            change(&mut s);
            s.frame().is_err()
        };
        assert!(refused(|s| s.ssid = String::new()));
        assert!(refused(|s| s.ssid = "x".repeat(33)));
        assert!(refused(|s| s.password = "x".repeat(65)));
        assert!(refused(|s| s.port = 18883));
        assert!(refused(|s| s.port = 0));
        assert!(refused(|s| s.host = "10.0.0.1:1883".into()));
        assert!(refused(|s| s.host = "h".repeat(96)));
        assert!(refused(|s| s.id = "12a4".into()));
        assert!(refused(|s| s.id = "000".into()));
        assert!(refused(|s| s.id = "1".repeat(31)));

        // At the limits it still fits one length byte.
        let mut s = setup();
        s.ssid = "s".repeat(32);
        s.password = "p".repeat(64);
        s.host = "h".repeat(95);
        s.port = 9999;
        s.id = "9".repeat(30);
        assert_eq!(s.frame().unwrap().len(), 6 + 4 + 32 + 64 + 100 + 30 + 1);
    }

    #[test]
    fn one_frame_goes_out_and_the_answer_comes_back() {
        let feeder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = feeder.local_addr().unwrap();
        let echo = std::thread::spawn(move || {
            let mut buf = [0; 256];
            let (n, from) = feeder.recv_from(&mut buf).unwrap();
            feeder.send_to(&[0xaa, 0x01], from).unwrap();
            buf[..n].to_vec()
        });

        let answer = send(&setup(), to, Duration::from_secs(2)).unwrap();
        assert_eq!(answer, Some(vec![0xaa, 0x01]));
        assert_eq!(echo.join().unwrap(), setup().frame().unwrap());
    }
}
