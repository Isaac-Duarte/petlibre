//! Stock's broker as its settings sector at 0x3e8000 stores it, read from
//! a whole flash read or the sector alone: the name the DNS-first install
//! has the owner's resolver override (`wellfed install --broker-name`).
//!
//! The name is the vendor's and is kept out of every tracked file, so the
//! bench takes it from its own flash reads instead. The record is the one
//! `Set_Dev_Info` writes and stock loads at every boot (static analysis of
//! 1.1.15): the port as a little-endian word at its start and the host,
//! NUL-terminated, at 0x0c, up to the 100 bytes the setup frame allows. The
//! offsets come from the research unit's flash reads (physical).

use core::ops::Range;

/// Bytes in a whole flash read of the feeder.
const FLASH_BYTES: usize = 0x80_0000;

/// The settings sector in flash.
const SECTOR: Range<usize> = 0x3e_8000..0x3e_9000;
/// The port word and the host field within the record.
const PORT: Range<usize> = 0x00..0x04;
const HOST: Range<usize> = 0x0c..0x0c + 100;

/// Stock's broker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broker {
    pub host: String,
    pub port: u32,
}

/// Why a file names no broker.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum BrokerError {
    #[error(
        "{len} bytes are neither stock's settings sector ({sector}) nor a whole flash read ({FLASH_BYTES})"
    )]
    Size { len: usize, sector: usize },
    #[error("stock's settings sector is erased: this stock was never set up")]
    Erased,
    #[error("stock's settings sector holds no broker host")]
    Host,
}

/// Stock's broker from `bytes`: the settings sector or a whole flash read.
pub fn read(bytes: &[u8]) -> Result<Broker, BrokerError> {
    let sector = match bytes.len() {
        FLASH_BYTES => bytes.get(SECTOR).unwrap_or_default(),
        len if len == SECTOR.len() => bytes,
        len => {
            return Err(BrokerError::Size {
                len,
                sector: SECTOR.len(),
            });
        }
    };
    let port = sector
        .get(PORT)
        .and_then(|word| <[u8; 4]>::try_from(word).ok());
    let (Some(port), Some(host)) = (port, sector.get(HOST)) else {
        return Err(BrokerError::Host);
    };
    if sector
        .get(..HOST.end)
        .is_some_and(|record| record.iter().all(|&b| b == 0xff))
    {
        return Err(BrokerError::Erased);
    }

    let host = host.split(|&b| b == 0).next().unwrap_or_default();
    let host = std::str::from_utf8(host)
        .ok()
        .filter(|host| !host.is_empty() && host.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or(BrokerError::Host)?;
    Ok(Broker {
        host: host.to_owned(),
        port: u32::from_le_bytes(port),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(host: &[u8], port: u32) -> Vec<u8> {
        let mut sector = vec![0xff; SECTOR.len()];
        sector[..0x484].fill(0);
        sector[PORT].copy_from_slice(&port.to_le_bytes());
        sector[HOST.start..HOST.start + host.len()].copy_from_slice(host);
        sector
    }

    #[test]
    fn the_sector_or_a_flash_read_names_the_same_broker() {
        let sector = sector(b"broker.example.net", 1883);
        let expected = Broker {
            host: "broker.example.net".into(),
            port: 1883,
        };
        assert_eq!(read(&sector), Ok(expected.clone()));

        let mut flash = vec![0xff; FLASH_BYTES];
        flash[SECTOR].copy_from_slice(&sector);
        assert_eq!(read(&flash), Ok(expected));
    }

    #[test]
    fn an_erased_or_odd_record_names_no_broker() {
        assert_eq!(read(&vec![0xff; SECTOR.len()]), Err(BrokerError::Erased));
        assert_eq!(read(&sector(b"", 1883)), Err(BrokerError::Host));
        assert_eq!(read(&sector(b"bad host", 1883)), Err(BrokerError::Host));
        assert!(matches!(
            read(&[0; 16]),
            Err(BrokerError::Size { len: 16, .. })
        ));
    }
}
