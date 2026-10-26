//! The known PLAF103 OTA container and region profile.
//!
//! Port of `container` and `regions` in `src/tools/audit/audit-ota-layout.py`.
//!
//! Development updates and frozen reviews take a bank's payload only through
//! `container` and `regions`, so an image outside this one profile is refused.

use serde::Serialize;

use wellfed_formats::require;

use crate::error::Result;

/// A container or payload outside the one profile the factory loader is
/// known to accept.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("OTA header is truncated")]
    Truncated,
    #[error("Unsupported OTA container profile")]
    Profile,
    #[error("OTA payload size does not match EOF")]
    Size,
    #[error("OTA byte-sum checksum mismatch")]
    Checksum,
    #[error("Unexpected inter-core padding")]
    InterCorePadding,
    #[error("Missing {name} image header")]
    MissingHeader { name: &'static str },
    #[error("Unsupported {name} header/load address")]
    Header { name: &'static str },
    #[error("Unsupported {name} body size")]
    BodySize { name: &'static str },
    #[error("{name} body exceeds payload")]
    BodyOverrun { name: &'static str },
    #[error("Unexpected final padding/extra regions")]
    FinalPadding,
}

/// The original stock image, the only one a frozen review may accept
/// without a build manifest.
pub const STOCK_SHA256: &str = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498";
/// Each image header's first two words: ASCII `81958711`, the loader marker.
pub(crate) const IMAGE_MAGIC: [u32; 2] = [0x35393138, 0x31313738];

fn words(data: &[u8], at: usize) -> [u32; 8] {
    std::array::from_fn(|i| {
        let word = at + i * 4;
        u32::from_le_bytes([data[word], data[word + 1], data[word + 2], data[word + 3]])
    })
}

/// The application payload of a single-entry OTA container.
pub fn container(data: &[u8]) -> Result<&[u8]> {
    require(data.len() >= 32, || Error::Truncated)?;
    let [
        version,
        count,
        magic,
        entry,
        checksum,
        size,
        offset,
        address,
    ] = words(data, 0);

    require(
        (version, count, magic, entry, offset, address)
            == (
                0xffff_ffff,
                1,
                u32::from_le_bytes(*b"OTA1"),
                24,
                32,
                0xffff_ffff,
            ),
        || Error::Profile,
    )?;
    require(size > 0 && size as usize == data.len() - 32, || Error::Size)?;

    let payload = &data[32..];
    let sum = payload
        .iter()
        .fold(0u32, |sum, byte| sum.wrapping_add(*byte as u32));
    require(sum == checksum, || Error::Checksum)?;

    Ok(payload)
}

/// One core image in a bank payload, as `audit-ota-layout.py` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Image {
    pub name: &'static str,
    pub payload_header_offset: usize,
    pub body_bytes: usize,
    pub runtime_address: String,
}

/// Both cores' images at their exact addresses, with only erased padding.
pub fn images(payload: &[u8]) -> Result<Vec<Image>> {
    let profile = [
        ("km0_xip", 0x0C00_0020u32),
        ("km0_ram", 0x0008_3000),
        ("km4_xip", 0x0E00_0020),
        ("km4_ram", 0x1000_5000),
        ("km4_psram", 0x0200_0020),
    ];
    let mut result = Vec::new();
    let mut offset = 0usize;

    for (name, address) in profile {
        if name == "km4_xip" {
            let aligned = (offset + 4095) & !4095;
            require(
                aligned <= payload.len() && payload[offset..aligned].iter().all(|b| *b == 0xff),
                || Error::InterCorePadding,
            )?;
            offset = aligned;
        }

        require(offset + 32 <= payload.len(), || Error::MissingHeader {
            name,
        })?;
        let header = words(payload, offset);
        require(
            header[..2] == IMAGE_MAGIC && header[3] == address && header[4..] == [0xffff_ffff; 4],
            || Error::Header { name },
        )?;

        let body = header[2] as usize;
        require((name == "km4_psram") == (body == 0), || Error::BodySize {
            name,
        })?;
        require(offset + 32 + body <= payload.len(), || Error::BodyOverrun {
            name,
        })?;

        result.push(Image {
            name,
            payload_header_offset: offset,
            body_bytes: body,
            runtime_address: format!("0x{address:08x}"),
        });
        offset += 32 + body;
    }

    require(
        payload.len() == (offset + 4095) & !4095 && payload[offset..].iter().all(|b| *b == 0xff),
        || Error::FinalPadding,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{synthetic_payload, wrap};

    #[test]
    fn synthetic_images_pass_and_damage_is_refused() {
        let payload = synthetic_payload(5000, 9000, 1);
        let image = wrap(&payload);
        assert_eq!(container(&image).unwrap(), &payload[..]);
        assert_eq!(images(&payload).unwrap().len(), 5);
        assert_eq!(images(&payload).unwrap()[2].name, "km4_xip");

        let mut bad = image.clone();
        bad[40] ^= 1;
        assert!(matches!(
            container(&bad),
            Err(crate::Error::Ota(Error::Checksum))
        ));

        let mut padding = payload.clone();
        let last = padding.len() - 1;
        padding[last] = 0;
        assert!(matches!(
            images(&padding),
            Err(crate::Error::Ota(Error::FinalPadding))
        ));

        assert!(matches!(
            container(&image[..image.len() - 1]),
            Err(crate::Error::Ota(Error::Size))
        ));
    }
}
