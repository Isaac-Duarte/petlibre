//! The device request id a journalled write goes out under.

use sha2::{Digest, Sha256};
use wellfed_api::{CommandKind, FeederId, User};

use crate::{IdempotencyKey, RequestId};

/// The device request id for a write. The feeder remembers writes by id
/// alone and every user shares the server's key, so the id is bound to who
/// sent it, to which feeder, to what kind of write and to its key.
pub(super) fn write_id(
    user: &User,
    feeder: FeederId,
    kind: CommandKind,
    key: &IdempotencyKey,
) -> RequestId {
    let digest = Sha256::new()
        .chain_update(b"wellfed-server write v1")
        .chain_update(user.id.0.as_bytes())
        .chain_update(feeder.0.as_bytes())
        .chain_update([kind_code(kind)])
        .chain_update(key)
        .finalize();
    let mut request = [0; 16];
    request.copy_from_slice(&digest[..16]);
    request
}

/// A write's kind as it is hashed into its request id: these never change.
const fn kind_code(kind: CommandKind) -> u8 {
    match kind {
        CommandKind::Settings => 1,
        CommandKind::Lock => 2,
        CommandKind::Feed => 3,
        CommandKind::Cancel => 4,
        CommandKind::Invite => 5,
        CommandKind::Revoke => 6,
        CommandKind::Wifi => 7,
        CommandKind::Reboot => 8,
        CommandKind::Play => 9,
        CommandKind::Remove => 10,
        CommandKind::Desiccant => 11,
        CommandKind::Plans => 12,
    }
}
