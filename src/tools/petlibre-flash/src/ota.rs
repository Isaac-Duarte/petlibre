//! The known PLAF103 OTA container and region profile.
//!
//! Port of `container` and `regions` in `src/tools/audit/audit-ota-layout.py`.
//!
//! Development updates and frozen reviews take a bank's payload only through
//! `container` and `regions`, so an image outside this one profile is refused.

use serde::Serialize;

use crate::error::{Result, require};

/// The original stock image, the only one a frozen review may accept
/// without a build manifest.
pub const STOCK_SHA256: &str = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498";
/// Each image header's first two words: ASCII `81958711`, the loader marker.
const IMAGE_MAGIC: [u32; 2] = [0x35393138, 0x31313738];

fn words(data: &[u8], at: usize) -> [u32; 8] {
    std::array::from_fn(|i| {
        let word = at + i * 4;
        u32::from_le_bytes([data[word], data[word + 1], data[word + 2], data[word + 3]])
    })
}

/// The application payload of a single-entry OTA container.
pub fn container(data: &[u8]) -> Result<&[u8]> {
    require(data.len() >= 32, "OTA header is truncated")?;
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
        "Unsupported OTA container profile",
    )?;
    require(
        size > 0 && size as usize == data.len() - 32,
        "OTA payload size does not match EOF",
    )?;

    let payload = &data[32..];
    let sum = payload
        .iter()
        .fold(0u32, |sum, byte| sum.wrapping_add(*byte as u32));
    require(sum == checksum, "OTA byte-sum checksum mismatch")?;

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
                "Unexpected inter-core padding",
            )?;
            offset = aligned;
        }

        require(
            offset + 32 <= payload.len(),
            format!("Missing {name} image header"),
        )?;
        let header = words(payload, offset);
        require(
            header[..2] == IMAGE_MAGIC && header[3] == address && header[4..] == [0xffff_ffff; 4],
            format!("Unsupported {name} header/load address"),
        )?;

        let body = header[2] as usize;
        require(
            (name == "km4_psram") == (body == 0),
            format!("Unsupported {name} body size"),
        )?;
        require(
            offset + 32 + body <= payload.len(),
            format!("{name} body exceeds payload"),
        )?;

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
        "Unexpected final padding/extra regions",
    )?;
    Ok(result)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A structurally valid synthetic payload: never a boot candidate.
    pub fn synthetic_payload(km0: usize, km4: usize, seed: u8) -> Vec<u8> {
        fn image(out: &mut Vec<u8>, address: u32, body: usize, seed: u8) {
            for word in [IMAGE_MAGIC[0], IMAGE_MAGIC[1], body as u32, address] {
                out.extend_from_slice(&word.to_le_bytes());
            }
            out.extend_from_slice(&[0xff; 16]);
            out.extend((0..body).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
        }

        let mut out = Vec::new();
        image(&mut out, 0x0C00_0020, km0, seed);
        image(&mut out, 0x0008_3000, 64, seed);
        out.resize((out.len() + 4095) & !4095, 0xff);
        image(&mut out, 0x0E00_0020, km4, seed ^ 0x55);
        image(&mut out, 0x1000_5000, 64, seed);
        image(&mut out, 0x0200_0020, 0, seed);
        out.resize((out.len() + 4095) & !4095, 0xff);
        out
    }

    pub fn wrap(payload: &[u8]) -> Vec<u8> {
        let sum = payload
            .iter()
            .fold(0u32, |sum, byte| sum.wrapping_add(*byte as u32));
        let mut out = Vec::new();
        for word in [
            0xffff_ffff,
            1,
            u32::from_le_bytes(*b"OTA1"),
            24,
            sum,
            payload.len() as u32,
            32,
            0xffff_ffff,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn synthetic_images_pass_and_damage_is_refused() {
        let payload = synthetic_payload(5000, 9000, 1);
        let image = wrap(&payload);
        assert_eq!(container(&image).unwrap(), &payload[..]);
        assert_eq!(images(&payload).unwrap().len(), 5);
        assert_eq!(images(&payload).unwrap()[2].name, "km4_xip");

        let mut bad = image.clone();
        bad[40] ^= 1;
        assert!(container(&bad).is_err());

        let mut padding = payload.clone();
        let last = padding.len() - 1;
        padding[last] = 0;
        assert!(images(&padding).is_err());

        assert!(container(&image[..image.len() - 1]).is_err());
    }
}
