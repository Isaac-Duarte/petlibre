//! FW45: the feeder's identity key and its enrolled clients.
//!
//! A client is its static key: an X25519 key for the device API's Noise
//! sessions, or an Ed25519 key for an owner's update signatures. The whole
//! table is one record, written to the other of two erase units on every
//! change, so an enrollment or revocation lands entirely or not at all:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..8 | magic `PLFCLI01` |
//! | 8..12 | version (u16), record length (u16) |
//! | 12..20 | generation (u64) |
//! | 20..52 | the feeder's X25519 secret |
//! | 52 | 1 when the secret is set |
//! | 64 + 64 n | client `n` of [`CLIENTS`]: key (32), label (16), role, enrolled generation |
//! | 576 | CRC-32/ISO-HDLC of bytes 0..576 |
//! | 580 | `CMIT`, programmed last |
//!
//! Unused bytes are zero, so one encoding exists per table. The newest valid
//! record wins; a torn newer unit falls back to the older one. The sectors
//! are this store's alone, so an unreadable record is a save of ours cut
//! short; only a newer schema makes the store read-only.
//!
//! The identity secret sits in plain flash: the board has no secure element,
//! so anyone who can read the flash chip can impersonate this feeder. The
//! factory reset ([`ClientStore::reset`], FP15) forgets every client and
//! takes a new identity, so old clients must pair again.

use core::ffi::c_uint;

use crate::store::slot::{
    COMMIT, JournalIo, LENGTH_AT, VERSION_AT, crc32, get_u16, get_u32, get_u64, put_u16, put_u32,
    put_u64,
};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFCLI01";
/// Enrolled clients, apart from the four concurrent sessions (FW44).
pub const CLIENTS: usize = 8;
pub const LABEL_LEN: usize = 16;

const IDENTITY_AT: usize = 20;
const HAS_IDENTITY_AT: usize = 52;
const ENTRIES_AT: usize = 64;
const ENTRY_BYTES: usize = 64;
const CRC_AT: usize = ENTRIES_AT + CLIENTS * ENTRY_BYTES;
const COMMIT_AT: usize = CRC_AT + 4;
/// The whole record.
pub const RECORD_BYTES: usize = COMMIT_AT + COMMIT.len();
/// [`COMMIT_AT`] and [`RECORD_BYTES`] as the I/O and the header store them.
const COMMIT_OFFSET: u32 = 580;
const RECORD_LEN: u16 = 584;

// Within an entry.
const LABEL_AT: usize = 32;
const ROLE_AT: usize = LABEL_AT + LABEL_LEN;
const ENROLLED_AT: usize = 52;

const _: () = assert!(ENROLLED_AT + 8 <= ENTRY_BYTES && RECORD_BYTES <= 4096);
const _: () = assert!(COMMIT_OFFSET as usize == COMMIT_AT && RECORD_LEN as usize == RECORD_BYTES);

named_enum! {
    /// What a client may do; the device API enforces it (FW47).
    pub enum Role {
        /// Everything, including inviting and revoking clients.
        Owner = "owner",
        /// Feeding and settings, but no client management.
        Member = "member",
        /// The optional server: history and release fetching.
        Server = "server",
        /// An owner's Ed25519 key for update signatures (FW48).
        UpdateSigner = "update-signer",
    }
}

impl Role {
    const fn code(self) -> u8 {
        match self {
            Self::Owner => 1,
            Self::Member => 2,
            Self::Server => 3,
            Self::UpdateSigner => 4,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Owner),
            2 => Some(Self::Member),
            3 => Some(Self::Server),
            4 => Some(Self::UpdateSigner),
            _ => None,
        }
    }
}

/// A client's name as the owner sees it: 1-16 bytes, no NUL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label {
    bytes: [u8; LABEL_LEN],
    len: u8,
}

impl Label {
    pub fn new(text: &[u8]) -> Option<Self> {
        if text.is_empty() || text.len() > LABEL_LEN || text.contains(&0) {
            return None;
        }
        let mut bytes = [0; LABEL_LEN];
        bytes[..text.len()].copy_from_slice(text);
        Some(Self {
            bytes,
            len: u8::try_from(text.len()).ok()?,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Client {
    pub key: [u8; 32],
    pub label: Label,
    pub role: Role,
    /// The store generation that enrolled it.
    pub enrolled: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientStatus {
    Unopened,
    /// No record: no identity and no clients, writable.
    Empty,
    Loaded,
    /// A newer schema version: read-only.
    ReadOnly,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// Unopened, read-only or failed earlier.
    NotWritable,
    Full,
    /// The key is already enrolled.
    Duplicate,
    NotFound,
    /// The identity is already set; only a reset replaces it.
    IdentitySet,
    /// The write failed; the previous table stands in memory.
    Io,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Table {
    identity: Option<[u8; 32]>,
    clients: [Option<Client>; CLIENTS],
}

impl Table {
    const EMPTY: Self = Self {
        identity: None,
        clients: [None; CLIENTS],
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Erased,
    /// The generation; the table went to the caller's buffer.
    Valid(u64),
    Future,
    Invalid,
}

#[derive(Clone, Copy)]
pub struct ClientStore {
    io: JournalIo,
    pub status: ClientStatus,
    pub generation: u64,
    pub slot: c_uint,
    table: Table,
}

impl core::fmt::Debug for ClientStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClientStore")
            .field("status", &self.status)
            .field("generation", &self.generation)
            .field("slot", &self.slot)
            .field("clients", &self.clients().count())
            .finish_non_exhaustive()
    }
}

impl Default for ClientStore {
    fn default() -> Self {
        Self {
            io: JournalIo::default(),
            status: ClientStatus::Unopened,
            generation: 0,
            slot: 0,
            table: Table::EMPTY,
        }
    }
}

fn encode(generation: u64, table: &Table) -> [u8; RECORD_BYTES] {
    let mut p = [0u8; RECORD_BYTES];
    p[..8].copy_from_slice(&MAGIC);
    put_u16(&mut p[VERSION_AT..], VERSION);
    put_u16(&mut p[LENGTH_AT..], RECORD_LEN);
    put_u64(&mut p[12..], generation);
    if let Some(secret) = &table.identity {
        p[IDENTITY_AT..IDENTITY_AT + 32].copy_from_slice(secret);
        p[HAS_IDENTITY_AT] = 1;
    }
    for (entry, client) in p[ENTRIES_AT..CRC_AT]
        .as_chunks_mut::<ENTRY_BYTES>()
        .0
        .iter_mut()
        .zip(&table.clients)
    {
        let Some(client) = client else { continue };
        entry[..32].copy_from_slice(&client.key);
        entry[LABEL_AT..ROLE_AT].copy_from_slice(&client.label.bytes);
        entry[ROLE_AT] = client.role.code();
        put_u64(&mut entry[ENROLLED_AT..], client.enrolled);
    }
    let crc = crc32(&p[..CRC_AT]);
    put_u32(&mut p[CRC_AT..], crc);
    p[COMMIT_AT..].copy_from_slice(&COMMIT);
    p
}

fn decode_entry(entry: &[u8]) -> Result<Option<Client>, ()> {
    if entry.iter().all(|&b| b == 0) {
        return Ok(None);
    }
    let role = Role::from_code(entry[ROLE_AT]).ok_or(())?;
    let label_len = entry[LABEL_AT..ROLE_AT]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(LABEL_LEN);
    let label = Label::new(&entry[LABEL_AT..LABEL_AT + label_len]).ok_or(())?;
    if entry[LABEL_AT + label_len..ROLE_AT].iter().any(|&b| b != 0)
        || entry[ROLE_AT + 1..ENROLLED_AT].iter().any(|&b| b != 0)
        || entry[ENROLLED_AT + 8..].iter().any(|&b| b != 0)
    {
        return Err(());
    }
    let mut key = [0; 32];
    key.copy_from_slice(&entry[..32]);
    Ok(Some(Client {
        key,
        label,
        role,
        enrolled: get_u64(&entry[ENROLLED_AT..]),
    }))
}

/// Decode `p`, leaving a valid record's table in `table` (any other result
/// leaves `table` unspecified).
fn decode(p: &[u8; RECORD_BYTES], table: &mut Table) -> Scan {
    if p.iter().all(|&b| b == u8::MAX) {
        return Scan::Erased;
    }
    if p[..8] != MAGIC
        || usize::from(get_u16(&p[LENGTH_AT..])) != RECORD_BYTES
        || p[COMMIT_AT..] != COMMIT
        || get_u32(&p[CRC_AT..]) != crc32(&p[..CRC_AT])
    {
        return Scan::Invalid;
    }
    let version = get_u16(&p[VERSION_AT..]);
    if version > VERSION {
        return Scan::Future;
    }
    let generation = get_u64(&p[12..]);
    if version != VERSION
        || generation == 0
        || p[HAS_IDENTITY_AT + 1..ENTRIES_AT].iter().any(|&b| b != 0)
    {
        return Scan::Invalid;
    }

    *table = Table::EMPTY;
    match p[HAS_IDENTITY_AT] {
        0 if p[IDENTITY_AT..HAS_IDENTITY_AT].iter().all(|&b| b == 0) => {}
        1 => {
            let mut secret = [0; 32];
            secret.copy_from_slice(&p[IDENTITY_AT..HAS_IDENTITY_AT]);
            table.identity = Some(secret);
        }
        _ => return Scan::Invalid,
    }
    for (slot, entry) in table
        .clients
        .iter_mut()
        .zip(p[ENTRIES_AT..CRC_AT].as_chunks::<ENTRY_BYTES>().0)
    {
        let Ok(client) = decode_entry(entry) else {
            return Scan::Invalid;
        };
        *slot = client;
    }
    Scan::Valid(generation)
}

impl ClientStore {
    /// Scan `slot` into the store's own table, so open never holds two
    /// decoded tables on the stack.
    fn scan(&mut self, slot: c_uint) -> Option<Scan> {
        let mut record = [0u8; RECORD_BYTES];
        self.io
            .read(slot, 0, &mut record)
            .then(|| decode(&record, &mut self.table))
    }

    /// Bind to two exclusive erase units and load the newest valid table.
    ///
    /// # Safety
    /// The I/O callbacks must be sound for `io.context` while the store is used.
    #[allow(unsafe_code)]
    pub unsafe fn open(&mut self, io: JournalIo) -> ClientStatus {
        *self = Self {
            io,
            ..Self::default()
        };
        if io.read.is_none() || (io.unit_bytes as usize) < RECORD_BYTES {
            self.status = ClientStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = ClientStatus::Io;
            return self.status;
        };

        let pick = match (a, b) {
            (Scan::Valid(ga), Scan::Valid(gb)) => Some(if ga >= gb { (0, ga) } else { (1, gb) }),
            (Scan::Valid(g), _) => Some((0, g)),
            (_, Scan::Valid(g)) => Some((1, g)),
            _ => None,
        };
        // Both scans wrote the table; reload the winner's, or start empty.
        self.table = Table::EMPTY;
        self.status = if a == Scan::Future || b == Scan::Future {
            ClientStatus::ReadOnly
        } else if let Some((slot, generation)) = pick {
            if self.scan(slot) != Some(Scan::Valid(generation)) {
                self.table = Table::EMPTY;
                self.status = ClientStatus::Io;
                return self.status;
            }
            self.slot = slot;
            self.generation = generation;
            ClientStatus::Loaded
        } else {
            ClientStatus::Empty
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, ClientStatus::Empty | ClientStatus::Loaded)
    }

    /// The feeder's X25519 secret, once set.
    pub fn identity(&self) -> Option<&[u8; 32]> {
        self.table.identity.as_ref()
    }

    pub fn clients(&self) -> impl Iterator<Item = &Client> {
        self.table.clients.iter().flatten()
    }

    pub fn find(&self, key: &[u8; 32]) -> Option<&Client> {
        self.clients().find(|client| &client.key == key)
    }

    /// Set the identity on first boot; a set identity changes only by reset.
    pub fn set_identity(&mut self, secret: &[u8; 32]) -> Result<(), ClientError> {
        if self.table.identity.is_some() {
            return Err(ClientError::IdentitySet);
        }
        let mut table = self.table;
        table.identity = Some(*secret);
        self.save(table)
    }

    /// Enroll a client; its `enrolled` field is set from the new generation.
    pub fn enroll(&mut self, key: [u8; 32], label: Label, role: Role) -> Result<(), ClientError> {
        if self.find(&key).is_some() {
            return Err(ClientError::Duplicate);
        }
        let mut table = self.table;
        let free = table
            .clients
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(ClientError::Full)?;
        *free = Some(Client {
            key,
            label,
            role,
            enrolled: self.generation + 1,
        });
        self.save(table)
    }

    /// Forget one client. Its sessions are the caller's to close.
    pub fn revoke(&mut self, key: &[u8; 32]) -> Result<(), ClientError> {
        let mut table = self.table;
        let slot = table
            .clients
            .iter_mut()
            .find(|slot| slot.is_some_and(|client| &client.key == key))
            .ok_or(ClientError::NotFound)?;
        *slot = None;
        self.save(table)
    }

    /// FP15's reset: no clients, and `identity` as the feeder's new key.
    pub fn reset(&mut self, identity: &[u8; 32]) -> Result<(), ClientError> {
        self.save(Table {
            identity: Some(*identity),
            ..Table::EMPTY
        })
    }

    /// Write `table` to the other unit: erase, program, verify, commit,
    /// verify. A failure keeps the previous table in memory.
    fn save(&mut self, table: Table) -> Result<(), ClientError> {
        if !self.writable() || self.io.program.is_none() || self.io.erase.is_none() {
            return Err(ClientError::NotWritable);
        }
        let generation = self.generation + 1;
        let slot = if self.generation == 0 {
            0
        } else {
            1 - self.slot
        };
        let bytes = encode(generation, &table);
        let mut checked = [0u8; RECORD_BYTES];
        let ok = self.io.erase(slot)
            && self.io.read(slot, 0, &mut checked)
            && checked.iter().all(|&b| b == u8::MAX)
            && self.io.program(slot, 0, &bytes[..COMMIT_AT])
            && self.io.program(slot, COMMIT_OFFSET, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;
        if !ok {
            self.status = ClientStatus::Io;
            return Err(ClientError::Io);
        }
        self.slot = slot;
        self.generation = generation;
        self.table = table;
        Ok(())
    }
}
