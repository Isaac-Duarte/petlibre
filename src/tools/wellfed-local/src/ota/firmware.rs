//! Replacement-firmware update delivery over MQTT.
//!
//! `Offer` is the server side of delivery: one owner-loaded update held only
//! in memory and answered over `ota/get`. Each offer is served to completion
//! once; a manifest request after its last chunk means the candidate fell
//! back, so the offer is dropped until the owner offers it again. Signing is
//! `wellfed-build sign`, which signs only a frozen review.
use anyhow::{Result, ensure};
use wellfed_core::ota::{self, MANIFEST_LEN};
use sha2::{Digest, Sha256};

/// The container stock installs is the one offered here.
pub use wellfed_stock::ota::image::{CONTAINER_HEADER, payload};

/// One owner-loaded update the server answers `ota/get` from. Checked
/// against its own manifest; authenticity is the firmware's signature check.
#[derive(Clone, Debug)]
pub struct Offer {
    pub manifest: [u8; MANIFEST_LEN],
    pub payload: Vec<u8>,
    pub version: u32,
    /// The last chunk has been served once: a later manifest request means
    /// the candidate fell back and the firmware is starting over.
    pub delivered: bool,
}

impl Offer {
    pub fn new(manifest: &[u8], container: &[u8]) -> Result<Self> {
        let manifest: [u8; MANIFEST_LEN] = manifest
            .try_into()
            .map_err(|_| anyhow::anyhow!("manifest must be {MANIFEST_LEN} bytes"))?;
        ensure!(manifest[..8] == ota::MAGIC, "not an update manifest");

        let payload = self::payload(container)?.to_vec();
        // Offsets follow `ota::manifest_body`: version at 8, image length at 12 and
        // the SHA-256 at 20..52.
        let field = |at: usize| {
            u32::from_le_bytes(
                *manifest[at..]
                    .first_chunk()
                    .expect("inside the checked manifest"),
            )
        };
        ensure!(
            field(12) as usize == payload.len(),
            "manifest length differs from the image"
        );
        ensure!(
            manifest[20..52] == Sha256::digest(&payload)[..],
            "manifest hash differs from the image"
        );

        Ok(Self {
            manifest,
            payload,
            version: field(8),
            delivered: false,
        })
    }

    /// The `ota/data` answer to one `ota/get`, or `None` for a malformed request.
    /// As `answer`, serving each offer to completion once. A manifest request
    /// after the last chunk went out drops the offer: the candidate was
    /// delivered and the firmware fell back (observed on hardware: a failing
    /// build was re-served after every fallback). Re-asks of chunks, such as
    /// a lost last answer, are still served. The owner re-offers to retry.
    /// Returns the answer and whether the offer was dropped.
    pub fn serve(offer: &mut Option<Self>, request: &[u8]) -> (Option<Vec<u8>>, bool) {
        let (part, offset, len) = match ota::decode_get(request) {
            Some(get) => get,
            None => return (None, false),
        };

        let dropped = part == ota::Part::Manifest && offer.as_ref().is_some_and(|o| o.delivered);
        if dropped {
            *offer = None;
        }

        let answer = Self::answer(offer.as_ref(), request);
        // Byte 1 of an answer is its status: only an `Ok` answer for the final
        // chunk counts as delivery.
        if let Some(o) = offer.as_mut()
            && part == ota::Part::Chunk
            && offset as usize + len as usize == o.payload.len()
            && answer.as_ref().is_some_and(|a| a.get(1) == Some(&0))
        {
            o.delivered = true;
        }

        (answer, dropped)
    }

    /// The answer to the normal role's `ota/check`: the manifest, or no
    /// offer. Never counted as a delivery.
    pub fn check(offer: Option<&Self>) -> Vec<u8> {
        Self::answer(offer, &ota::encode_get(ota::Part::Manifest, 0, 0)).unwrap_or_default()
    }

    pub fn answer(offer: Option<&Self>, request: &[u8]) -> Option<Vec<u8>> {
        let (part, offset, len) = ota::decode_get(request)?;
        let refuse = |status| ota::encode_data_header(part, status, offset, 0).to_vec();
        let Some(offer) = offer else {
            return Some(refuse(ota::Status::NoOffer));
        };

        let body: &[u8] = match part {
            ota::Part::Manifest => &offer.manifest,
            ota::Part::Chunk => {
                let end = offset as usize + len as usize;
                if len == 0 || len > ota::MAX_CHUNK || end > offer.payload.len() {
                    return Some(refuse(ota::Status::Range));
                }
                &offer.payload[offset as usize..end]
            }
        };

        let mut out = ota::encode_data_header(
            part,
            ota::Status::Ok,
            offset,
            u32::try_from(body.len()).ok()?,
        )
        .to_vec();
        out.extend_from_slice(body);
        Some(out)
    }
}
