//! Stock 1.1.15's setup frame, as the vendor app sends it to a feeder in
//! setup mode (static analysis; [consumer installation]). One plaintext UDP
//! datagram to the feeder's setup access point carries the Wi-Fi network
//! and the MQTT broker, so a stock feeder can be pointed at the installer's
//! broker with no traffic redirect. Type 3 sets both; PLAF103 frames carry
//! no checksum and no encryption, so the frame is all there is to it.
//!
//! The broker it names keeps nothing of the vendor's: stock presents its
//! own MQTT credentials to whatever broker the frame names.
//!
//! [consumer installation]: ../../../../../docs/firmware/consumer-installation.md

use core::net::{Ipv4Addr, SocketAddrV4};

/// Where the feeder's setup access point listens (`Ap_Config_Udp_Thread`,
/// port `0x444C`, "DL").
pub const SETUP_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 17484);

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

/// A value stock would misread or refuse, so no frame is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("the SSID must be 1-{SSID_MAX} bytes")]
    Ssid,
    #[error("the passphrase must be at most {PASSWORD_MAX} bytes")]
    Password,
    #[error(
        "the broker host must be set, without a port, and host:port at most {SERVER_MAX} bytes"
    )]
    Host,
    #[error("the broker port must be 1-{PORT_MAX}: stock reads four digits")]
    Port,
    #[error("the account ID must be 1-{ID_MAX} ASCII digits, not zero")]
    Id,
}

/// What one setup frame tells stock.
#[derive(Clone, PartialEq, Eq)]
pub struct SetupFrame {
    pub ssid: String,
    pub password: String,
    /// The broker's IPv4 address or name, without a port.
    pub host: String,
    pub port: u16,
    /// The account's numeric ID (ASCII digits, nonzero), which stock keeps
    /// beside the broker and requires. What the vendor means by it is not
    /// established; stock checks only its form.
    pub id: String,
}

// The passphrase stays out of logs.
impl core::fmt::Debug for SetupFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SetupFrame")
            .field("ssid", &self.ssid)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl SetupFrame {
    /// The type-3 frame: `DL`, type, checksum flag 0, 0, payload length,
    /// then length-prefixed SSID, password, `host:port` and ID, then `0xFF`.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let server = format!("{}:{}", self.host, self.port);
        if self.ssid.is_empty() || self.ssid.len() > SSID_MAX {
            return Err(FrameError::Ssid);
        }
        if self.password.len() > PASSWORD_MAX {
            return Err(FrameError::Password);
        }
        if self.host.is_empty() || self.host.contains(':') || server.len() > SERVER_MAX {
            return Err(FrameError::Host);
        }
        if !(1..=PORT_MAX).contains(&self.port) {
            return Err(FrameError::Port);
        }
        let digits = self.id.bytes().all(|b| b.is_ascii_digit());
        let nonzero = self.id.bytes().any(|b| b != b'0');
        if self.id.is_empty() || self.id.len() > ID_MAX || !digits || !nonzero {
            return Err(FrameError::Id);
        }

        let fields = [
            self.ssid.as_bytes(),
            self.password.as_bytes(),
            server.as_bytes(),
            self.id.as_bytes(),
        ];
        // Every field is within its limit, so each length and their sum
        // (at most 4 + 32 + 64 + 100 + 30 = 230) fits one byte.
        let length: usize = fields.iter().map(|field| 1 + field.len()).sum();
        let mut frame = vec![b'D', b'L', TYPE_BOTH, 0, 0];
        frame.push(u8::try_from(length).map_err(|_| FrameError::Host)?);
        for field in fields {
            frame.push(u8::try_from(field.len()).map_err(|_| FrameError::Host)?);
            frame.extend_from_slice(field);
        }
        frame.push(END);
        Ok(frame)
    }
}
