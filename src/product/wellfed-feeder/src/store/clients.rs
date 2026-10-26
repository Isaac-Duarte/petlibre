//! The feeder's identity key and its enrolled clients.
//!
//! A client is its static key: an X25519 key for the device API's Noise
//! sessions, or an Ed25519 key for an owner's update signatures. The whole
//! table is one record in the dual-slot store
//! (`wellfed_core::store::slot`), written to the other of two erase units
//! on every change, so an enrollment or revocation lands entirely or not at
//! all:
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
//! factory reset ([`ClientStore::reset`]) forgets every client and
//! takes a new identity, so old clients must pair again.

use core::ffi::c_uint;

use wellfed_core::store::slot::{
    DualSlot, Extent, Found, Layout, Pick, Prepare, SlotIo, get_u64, newest, put_u64, seal, target,
};
use wellfed_device_api::clients::{CLIENTS, Client, LABEL_LEN, Label, Role};
use wellfed_device_api::mailbox::KeyResult;

pub const VERSION: u16 = 1;
const LAYOUT: Layout = Layout {
    magic: *b"PLFCLI01",
    version: VERSION,
    extent: Extent::Record,
};

const GENERATION_AT: usize = 12;
const IDENTITY_AT: usize = 20;
const HAS_IDENTITY_AT: usize = 52;
const ENTRIES_AT: usize = 64;
const ENTRY_BYTES: usize = 64;
/// The checksum, then the marker.
const CRC_AT: usize = ENTRIES_AT + CLIENTS * ENTRY_BYTES;
/// The whole record.
pub const RECORD_BYTES: usize = CRC_AT + 8;

// Within an entry.
const LABEL_AT: usize = 32;
const ROLE_AT: usize = LABEL_AT + LABEL_LEN;
const ENROLLED_AT: usize = 52;

const _: () = assert!(ENROLLED_AT + 8 <= ENTRY_BYTES && RECORD_BYTES <= 4096);
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

impl From<ClientError> for KeyResult {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::NotWritable | ClientError::IdentitySet => Self::NotWritable,
            ClientError::Full => Self::Full,
            ClientError::Duplicate => Self::Duplicate,
            ClientError::NotFound => Self::NotFound,
            ClientError::Io => Self::Io,
        }
    }
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

#[derive(Clone, Copy)]
pub struct ClientStore<I> {
    units: DualSlot<I>,
    pub status: ClientStatus,
    pub generation: u64,
    pub slot: c_uint,
    table: Table,
}

impl<I: SlotIo> core::fmt::Debug for ClientStore<I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClientStore")
            .field("status", &self.status)
            .field("generation", &self.generation)
            .field("slot", &self.slot)
            .field("clients", &self.clients().count())
            .finish_non_exhaustive()
    }
}

impl<I> Default for ClientStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: ClientStatus::Unopened,
            generation: 0,
            slot: 0,
            table: Table::EMPTY,
        }
    }
}

fn encode(generation: u64, table: &Table) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u64::<GENERATION_AT, _>(&mut p, generation);
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
        let label = client.label.as_bytes();
        entry[LABEL_AT..LABEL_AT + label.len()].copy_from_slice(label);
        entry[ROLE_AT] = client.role.code();
        put_u64::<ENROLLED_AT, _>(entry, client.enrolled);
    }
    seal(&mut p);
    p
}

fn decode_entry(entry: &[u8; ENTRY_BYTES]) -> Result<Option<Client>, ()> {
    if entry.iter().all(|&b| b == 0) {
        return Ok(None);
    }
    let role = Role::from_byte(entry[ROLE_AT]).ok_or(())?;
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
        enrolled: get_u64::<ENROLLED_AT, _>(entry),
    }))
}

/// Decode `p` into `table`: the generation of a valid record (anything
/// else leaves `table` unspecified).
fn decode(p: &[u8; RECORD_BYTES], table: &mut Table) -> Option<u64> {
    let generation = get_u64::<GENERATION_AT, _>(p);
    if generation == 0 || p[HAS_IDENTITY_AT + 1..ENTRIES_AT].iter().any(|&b| b != 0) {
        return None;
    }

    *table = Table::EMPTY;
    match p[HAS_IDENTITY_AT] {
        0 if p[IDENTITY_AT..HAS_IDENTITY_AT].iter().all(|&b| b == 0) => {}
        1 => {
            let mut secret = [0; 32];
            secret.copy_from_slice(&p[IDENTITY_AT..HAS_IDENTITY_AT]);
            table.identity = Some(secret);
        }
        _ => return None,
    }
    for (slot, entry) in table
        .clients
        .iter_mut()
        .zip(p[ENTRIES_AT..CRC_AT].as_chunks::<ENTRY_BYTES>().0)
    {
        *slot = decode_entry(entry).ok()?;
    }
    Some(generation)
}

impl<I: SlotIo> ClientStore<I> {
    /// Scan `slot` into the store's own table, so open never holds two
    /// decoded tables on the stack.
    fn scan(&mut self, slot: c_uint) -> Option<Found<u64>> {
        let table = &mut self.table;
        self.units.scan(slot, |p| decode(p, table))
    }

    /// Bind to two exclusive erase units and load the newest valid table.
    pub fn open(&mut self, io: I) -> ClientStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = ClientStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = ClientStatus::Io;
            return self.status;
        };

        // Both scans wrote the table; reload the winner's, or start empty.
        self.table = Table::EMPTY;
        self.status = match newest(a, b, |&generation| generation) {
            Pick::Future => ClientStatus::ReadOnly,
            Pick::Newest(slot, generation) => {
                if self.scan(slot) != Some(Found::Valid(generation)) {
                    self.table = Table::EMPTY;
                    self.status = ClientStatus::Io;
                    return self.status;
                }
                self.slot = slot;
                self.generation = generation;
                ClientStatus::Loaded
            }
            Pick::Nothing => ClientStatus::Empty,
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
        self.save(&table)
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
        self.save(&table)
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
        self.save(&table)
    }

    /// The reset: no clients, and `identity` as the feeder's new key.
    pub fn reset(&mut self, identity: &[u8; 32]) -> Result<(), ClientError> {
        self.save(&Table {
            identity: Some(*identity),
            ..Table::EMPTY
        })
    }

    /// Write `table` to the other unit by the torn-write rule. A failure
    /// keeps the previous table in memory.
    fn save(&mut self, table: &Table) -> Result<(), ClientError> {
        if !self.writable() || !self.units.writable() {
            return Err(ClientError::NotWritable);
        }
        let generation = self.generation + 1;
        let slot = target(self.generation, self.slot);
        if !self
            .units
            .write(slot, &encode(generation, table), Prepare::Erase)
        {
            self.status = ClientStatus::Io;
            return Err(ClientError::Io);
        }
        self.slot = slot;
        self.generation = generation;
        self.table = *table;
        Ok(())
    }
}
