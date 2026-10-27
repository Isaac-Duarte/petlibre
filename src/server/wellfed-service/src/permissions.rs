//! What each role may do: the whole matrix in one table, which the
//! server's extractors ask per route.
//!
//! Every account sees every feeder; there are no per-feeder grants. A
//! member feeds and changes settings; a viewer only reads; an owner does
//! everything, including what touches other accounts, the household, the
//! feeders' own administration and the server's data.

use wellfed_api::Role;

/// Something a route does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Status, history, samples, plans and the food views.
    Read,
    /// Feed, and cancel a feed.
    Feed,
    /// Settings, the lock, a tune, and a clock sync that does not step the
    /// clock.
    Control,
    /// An invite for the user's own phone.
    InviteOwnPhone,
    /// The user's own commands and audit entries.
    OwnHistory,
    /// The user's own account: their password and their tokens.
    OwnAccount,
    /// Everyone's commands and audit entries.
    AllHistory,
    /// Users and other users' tokens.
    Users,
    /// The household and the feeders' time settings.
    Household,
    /// Pets, foods and refills.
    OwnerData,
    /// Wi-Fi, reboot, and listing and revoking a feeder's clients.
    FeederAdmin,
    /// Adopting and removing feeders.
    Adopt,
    /// Releases, installs, backups, export and import.
    ServerData,
}

/// Whether `role` may do `permission`.
pub const fn can(role: Role, permission: Permission) -> bool {
    use Permission::{
        Adopt, AllHistory, Control, Feed, FeederAdmin, Household, InviteOwnPhone, OwnAccount,
        OwnHistory, OwnerData, Read, ServerData, Users,
    };

    match role {
        Role::Owner => true,
        Role::Member => match permission {
            Read | Feed | Control | InviteOwnPhone | OwnHistory | OwnAccount => true,
            AllHistory | Users | Household | OwnerData | FeederAdmin | Adopt | ServerData => false,
        },
        Role::Viewer => match permission {
            Read | OwnHistory | OwnAccount => true,
            Feed | Control | InviteOwnPhone | AllHistory | Users | Household | OwnerData
            | FeederAdmin | Adopt | ServerData => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Permission::*;

    /// The matrix as the spec lays it out, row by row: owner, member,
    /// viewer.
    #[test]
    fn the_table_is_the_matrix() {
        let rows = [
            (Read, [true, true, true]),
            (Feed, [true, true, false]),
            (Control, [true, true, false]),
            (InviteOwnPhone, [true, true, false]),
            (OwnHistory, [true, true, true]),
            (OwnAccount, [true, true, true]),
            (AllHistory, [true, false, false]),
            (Users, [true, false, false]),
            (Household, [true, false, false]),
            (OwnerData, [true, false, false]),
            (FeederAdmin, [true, false, false]),
            (Adopt, [true, false, false]),
            (ServerData, [true, false, false]),
        ];
        for (permission, allowed) in rows {
            let got = [Role::Owner, Role::Member, Role::Viewer].map(|role| can(role, permission));
            assert_eq!(got, allowed, "{permission:?}");
        }
    }
}
