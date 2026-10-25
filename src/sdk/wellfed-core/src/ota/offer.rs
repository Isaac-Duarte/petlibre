//! Signed offers over the device API: the version 2 manifest, the keys
//! that may sign one, and the statements that change those keys or allow
//! a downgrade.
//!
//! A manifest names the key that signed it by its [`KeyId`], so the
//! feeder checks one signature, never every key it knows. Three kinds of
//! key are trusted ([`KeyRing`]):
//! - **Built in:** compiled into the firmware (today the bench key). These
//!   and the release slot's key sign key statements, so a key the owner
//!   enrolled cannot add or retire keys behind the build's back.
//! - **Added:** the release slot, empty in the build and filled by a
//!   [`KEY_ADD_MAGIC`] statement a built-in key signed. A release key can
//!   then arrive without reinstalling bank 0, which keeps the build's list,
//!   and goes on signing key statements once the bench key is retired.
//! - **Enrolled:** an owner's Ed25519 key, enrolled over the device API with
//!   a Dispense press and removed like any client.
//!
//! A [`REVOKE_MAGIC`] statement retires any key by id, for good: the update
//! store keeps the ids and never forgets one.
//!
//! Anti-rollback: an offer whose version is not above the confirmed floor
//! (`min_version`, kept in the update store), and every stock image, needs a
//! [`DOWNGRADE_MAGIC`] statement bound to this feeder's identity, the exact
//! manifest and the current floor. A replayed old manifest, or a statement
//! made for another feeder or before the floor last rose, is refused.
//!
//! Each statement starts with its own magic, so no signed message can be
//! read as another kind. Verification is sans-IO and allocation-free; it
//! needs about 8 KiB of stack per signature, so the firmware runs it outside
//! its loop task.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use super::{BANK1_BYTES, MAGIC as V1_MAGIC, Manifest, SECTOR_BYTES};

pub const MAGIC: [u8; 8] = *b"PLOTA2\0\0";
pub const KEY_ADD_MAGIC: [u8; 8] = *b"PLKEY1\0\0";
pub const REVOKE_MAGIC: [u8; 8] = *b"PLREV1\0\0";
pub const DOWNGRADE_MAGIC: [u8; 8] = *b"PLDWN1\0\0";

pub const SIGNATURE_LEN: usize = 64;
pub const KEY_ID_LEN: usize = 8;

/// magic, version, image length, target bank, kind and three zero bytes,
/// key id, image SHA-256, build ID.
pub const MANIFEST_BODY: usize = 8 + 4 + 4 + 4 + 4 + KEY_ID_LEN + 32 + 32;
pub const MANIFEST_LEN: usize = MANIFEST_BODY + SIGNATURE_LEN;

/// Every statement: its magic and the signer's key id, then its fields.
const STATEMENT_HEADER: usize = 8 + KEY_ID_LEN;
/// The added key.
pub const KEY_ADD_BODY: usize = STATEMENT_HEADER + 32;
/// The retired key's id.
pub const REVOKE_BODY: usize = STATEMENT_HEADER + KEY_ID_LEN;
/// The feeder's identity key, the manifest's digest and the floor.
pub const DOWNGRADE_BODY: usize = STATEMENT_HEADER + 32 + 32 + 4;
/// The longest statement, signature included.
pub const STATEMENT_MAX: usize = DOWNGRADE_BODY + SIGNATURE_LEN;

/// The first 8 bytes of SHA-256 over an Ed25519 public key.
pub type KeyId = [u8; KEY_ID_LEN];

pub fn key_id(public: &[u8; 32]) -> KeyId {
    let digest = Sha256::digest(public);
    let mut id = [0; KEY_ID_LEN];
    id.copy_from_slice(&digest[..KEY_ID_LEN]);
    id
}

/// SHA-256 over a whole manifest, signature included: what the give-way
/// records and a downgrade statement names.
pub fn manifest_digest(manifest: &[u8]) -> [u8; 32] {
    Sha256::digest(manifest).into()
}

/// What an image is. A stock image always needs a downgrade statement:
/// it has no trial, so the replacement cannot come back without the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Replacement,
    Stock,
}

impl ImageKind {
    pub const fn code(self) -> u8 {
        match self {
            Self::Replacement => 1,
            Self::Stock => 2,
        }
    }

    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Replacement),
            2 => Some(Self::Stock),
            _ => None,
        }
    }
}

/// A version 2 manifest's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signed {
    pub manifest: Manifest,
    pub kind: ImageKind,
    /// The key that signed it.
    pub key_id: KeyId,
}

/// The signed part of a version 2 manifest; the signer appends the
/// 64-byte Ed25519 signature.
pub fn manifest_body(signed: &Signed) -> [u8; MANIFEST_BODY] {
    let m = &signed.manifest;
    let mut out = [0u8; MANIFEST_BODY];
    out[..8].copy_from_slice(&MAGIC);
    out[8..12].copy_from_slice(&m.version.to_le_bytes());
    out[12..16].copy_from_slice(&m.image_len.to_le_bytes());
    out[16..20].copy_from_slice(&m.target_bank.to_le_bytes());
    out[20] = signed.kind.code();
    out[24..32].copy_from_slice(&signed.key_id);
    out[32..64].copy_from_slice(&m.image_sha256);
    out[64..96].copy_from_slice(&m.build_id);
    out
}

/// A key statement's signed part: retire `revoked`.
pub fn revoke_body(signer: &KeyId, revoked: &KeyId) -> [u8; REVOKE_BODY] {
    let mut out = [0u8; REVOKE_BODY];
    out[..8].copy_from_slice(&REVOKE_MAGIC);
    out[8..16].copy_from_slice(signer);
    out[16..].copy_from_slice(revoked);
    out
}

/// A key statement's signed part: fill the release slot with `key`.
pub fn key_add_body(signer: &KeyId, key: &[u8; 32]) -> [u8; KEY_ADD_BODY] {
    let mut out = [0u8; KEY_ADD_BODY];
    out[..8].copy_from_slice(&KEY_ADD_MAGIC);
    out[8..16].copy_from_slice(signer);
    out[16..].copy_from_slice(key);
    out
}

/// A downgrade statement's signed part: install the manifest with `digest`
/// on the feeder whose identity key is `device`, while its floor is `floor`.
pub fn downgrade_body(
    signer: &KeyId,
    device: &[u8; 32],
    digest: &[u8; 32],
    floor: u32,
) -> [u8; DOWNGRADE_BODY] {
    let mut out = [0u8; DOWNGRADE_BODY];
    out[..8].copy_from_slice(&DOWNGRADE_MAGIC);
    out[8..16].copy_from_slice(signer);
    out[16..48].copy_from_slice(device);
    out[48..80].copy_from_slice(digest);
    out[80..84].copy_from_slice(&floor.to_le_bytes());
    out
}

/// How a key came to be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    BuiltIn,
    Added,
    Enrolled,
}

/// The keys one check may use. Revocation wins over every list.
#[derive(Clone, Copy, Debug)]
pub struct KeyRing<'a> {
    pub built_in: &'a [[u8; 32]],
    pub added: Option<[u8; 32]>,
    pub enrolled: &'a [[u8; 32]],
    pub revoked: &'a [KeyId],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// No trusted key has that id.
    Unknown,
    Revoked,
}

impl KeyRing<'_> {
    fn revoked(&self, id: &KeyId) -> bool {
        self.revoked.contains(id)
    }

    /// The trusted key with `id`, for a manifest or a downgrade statement.
    pub fn find(&self, id: &KeyId) -> Result<([u8; 32], Trust), KeyError> {
        if self.revoked(id) {
            return Err(KeyError::Revoked);
        }
        let built_in = self.built_in.iter().map(|key| (*key, Trust::BuiltIn));
        let added = self.added.map(|key| (key, Trust::Added));
        let enrolled = self.enrolled.iter().map(|key| (*key, Trust::Enrolled));
        built_in
            .chain(added)
            .chain(enrolled)
            .find(|(key, _)| key_id(key) == *id)
            .ok_or(KeyError::Unknown)
    }

    /// The key with `id` that may sign key statements: a built-in key, or
    /// the release slot's (which only a built-in key's statement fills), so
    /// retiring the bench key never freezes rotation. An owner's enrolled
    /// key signs offers only, and a retired key signs nothing.
    fn statement_signer(&self, id: &KeyId) -> Result<[u8; 32], KeyError> {
        if self.revoked(id) {
            return Err(KeyError::Revoked);
        }
        self.built_in
            .iter()
            .copied()
            .chain(self.added)
            .find(|key| key_id(key) == *id)
            .ok_or(KeyError::Unknown)
    }
}

/// Why a statement was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementError {
    Length,
    /// Not the kind of statement this place takes.
    Magic,
    Key(KeyError),
    Signature,
    /// A downgrade statement made for another feeder.
    Device,
    /// A downgrade statement for another manifest.
    Digest,
    /// A downgrade statement made while the floor stood elsewhere.
    Floor,
}

/// Why an offer was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferError {
    Length,
    /// A version 1 manifest: this firmware takes offers over the device
    /// API only, and those are version 2.
    Version,
    Magic,
    Key(KeyError),
    Signature,
    Bank,
    Size,
    /// An image kind this firmware does not know, or nonzero padding.
    Kind,
    /// The running build: nothing to install.
    Same,
    /// The version is not above the floor (or the image is stock) and no
    /// downgrade statement came with it.
    Downgrade,
    Statement(StatementError),
}

/// An offer that passed every check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub signed: Signed,
    pub trust: Trust,
    /// [`manifest_digest`] of the offered bytes.
    pub digest: [u8; 32],
    /// The signer of the downgrade statement that allowed it, if one did.
    pub downgrade: Option<Signer>,
}

/// Who signed a statement: its key id and how that key is trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signer {
    pub key_id: KeyId,
    pub trust: Trust,
}

/// What an offer is checked against.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    pub ring: KeyRing<'a>,
    /// The highest confirmed version: an offer must be above it.
    pub floor: u32,
    /// This feeder's identity key (X25519 public), which a downgrade
    /// statement must name.
    pub device: [u8; 32],
    /// The running build's identity.
    pub running: [u8; 32],
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [0; N];
    out.copy_from_slice(&bytes[..N]);
    out
}

/// Check `bytes` (body then signature) against `key`. Out of line: the
/// Ed25519 verification is the deepest frame in the update path, and two
/// sequential checks must not each inline their own copy.
#[inline(never)]
fn verify(key: &[u8; 32], body: &[u8], signature: &[u8]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(key) else {
        return false;
    };
    let Ok(signature) = <[u8; SIGNATURE_LEN]>::try_from(signature) else {
        return false;
    };
    key.verify_strict(body, &Signature::from_bytes(&signature))
        .is_ok()
}

/// The manifest's fields, without checking its signature: for a client
/// showing what it holds, and the first step of [`admit_offer`].
pub fn parse_manifest(bytes: &[u8]) -> Result<Signed, OfferError> {
    if bytes.len() >= 8 && bytes[..8] == V1_MAGIC {
        return Err(OfferError::Version);
    }
    if bytes.len() != MANIFEST_LEN {
        return Err(OfferError::Length);
    }
    if bytes[..8] != MAGIC {
        return Err(OfferError::Magic);
    }
    let kind = ImageKind::from_byte(bytes[20]).ok_or(OfferError::Kind)?;
    if bytes[21..24] != [0; 3] {
        return Err(OfferError::Kind);
    }

    Ok(Signed {
        manifest: Manifest {
            version: u32_at(bytes, 8),
            image_len: u32_at(bytes, 12),
            target_bank: u32_at(bytes, 16),
            image_sha256: array(&bytes[32..64]),
            build_id: array(&bytes[64..96]),
        },
        kind,
        key_id: array(&bytes[24..32]),
    })
}

/// Accept an offer only if a trusted key signed it, it targets bank 1 and
/// fits the reviewed bound, it is not the running build, and it is either
/// above the floor or carries a downgrade statement for exactly this
/// feeder, manifest and floor (always, for stock).
pub fn admit_offer(
    manifest: &[u8],
    downgrade: Option<&[u8]>,
    context: &Context<'_>,
) -> Result<Admitted, OfferError> {
    let signed = parse_manifest(manifest)?;
    let (key, trust) = context.ring.find(&signed.key_id).map_err(OfferError::Key)?;
    if !verify(&key, &manifest[..MANIFEST_BODY], &manifest[MANIFEST_BODY..]) {
        return Err(OfferError::Signature);
    }

    let m = &signed.manifest;
    if m.target_bank != 1 {
        return Err(OfferError::Bank);
    }
    if m.image_len < SECTOR_BYTES || m.image_len > BANK1_BYTES {
        return Err(OfferError::Size);
    }
    if m.build_id == context.running {
        return Err(OfferError::Same);
    }

    let digest = manifest_digest(manifest);
    let needs_statement = signed.kind == ImageKind::Stock || m.version <= context.floor;
    let downgrade = match downgrade {
        Some(statement) if needs_statement => {
            Some(check_downgrade(statement, &digest, context).map_err(OfferError::Statement)?)
        }
        None if needs_statement => return Err(OfferError::Downgrade),
        _ => None,
    };

    Ok(Admitted {
        signed,
        trust,
        digest,
        downgrade,
    })
}

fn check_downgrade(
    statement: &[u8],
    digest: &[u8; 32],
    context: &Context<'_>,
) -> Result<Signer, StatementError> {
    if statement.len() != DOWNGRADE_BODY + SIGNATURE_LEN {
        return Err(StatementError::Length);
    }
    if statement[..8] != DOWNGRADE_MAGIC {
        return Err(StatementError::Magic);
    }
    let key_id = array(&statement[8..16]);
    let (key, trust) = context.ring.find(&key_id).map_err(StatementError::Key)?;
    if !verify(
        &key,
        &statement[..DOWNGRADE_BODY],
        &statement[DOWNGRADE_BODY..],
    ) {
        return Err(StatementError::Signature);
    }

    if statement[16..48] != context.device {
        return Err(StatementError::Device);
    }
    if statement[48..80] != *digest {
        return Err(StatementError::Digest);
    }
    if u32_at(statement, 80) != context.floor {
        return Err(StatementError::Floor);
    }
    Ok(Signer { key_id, trust })
}

/// A key statement's effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStatement {
    /// Retire this key id for good.
    Revoke(KeyId),
    /// Fill the release slot with this key.
    Add([u8; 32]),
}

/// Accept a key statement only if a built-in key that is not retired
/// signed it.
pub fn admit_statement(
    statement: &[u8],
    ring: &KeyRing<'_>,
) -> Result<KeyStatement, StatementError> {
    if statement.len() < STATEMENT_HEADER {
        return Err(StatementError::Length);
    }
    let magic = array::<8>(statement);
    let body = match magic {
        REVOKE_MAGIC => REVOKE_BODY,
        KEY_ADD_MAGIC => KEY_ADD_BODY,
        _ => return Err(StatementError::Magic),
    };
    if statement.len() != body + SIGNATURE_LEN {
        return Err(StatementError::Length);
    }

    let key = ring
        .statement_signer(&array(&statement[8..16]))
        .map_err(StatementError::Key)?;
    if !verify(&key, &statement[..body], &statement[body..]) {
        return Err(StatementError::Signature);
    }

    let fields = &statement[STATEMENT_HEADER..];
    Ok(if magic == REVOKE_MAGIC {
        KeyStatement::Revoke(array(fields))
    } else {
        KeyStatement::Add(array(fields))
    })
}
