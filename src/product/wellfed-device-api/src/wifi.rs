//! Wi-Fi credentials, as a request sets them and the feeder's Wi-Fi store
//! keeps them.

pub const MAX_SSID: usize = 32;
/// WPA2 passphrases are 8-63 printable ASCII characters.
pub const MIN_PSK: usize = 8;
pub const MAX_PSK: usize = 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Credentials {
    ssid: [u8; MAX_SSID],
    ssid_len: u8,
    psk: [u8; 64],
    psk_len: u8,
}

impl Credentials {
    /// A 1-32 byte SSID and an 8-63 character printable-ASCII passphrase.
    pub fn new(ssid: &[u8], psk: &[u8]) -> Option<Self> {
        if ssid.is_empty()
            || ssid.len() > MAX_SSID
            || !(MIN_PSK..=MAX_PSK).contains(&psk.len())
            || !psk.iter().all(|b| (0x20..0x7f).contains(b))
        {
            return None;
        }
        let mut out = Self {
            ssid: [0; MAX_SSID],
            ssid_len: u8::try_from(ssid.len()).ok()?,
            psk: [0; 64],
            psk_len: u8::try_from(psk.len()).ok()?,
        };
        out.ssid[..ssid.len()].copy_from_slice(ssid);
        out.psk[..psk.len()].copy_from_slice(psk);
        Some(out)
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }

    pub fn psk(&self) -> &[u8] {
        &self.psk[..usize::from(self.psk_len)]
    }
}
