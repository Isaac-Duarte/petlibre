//! The audit log: the command journal and the account changes, oldest
//! first. Each user reads their own entries; an owner reads everyone's.
//!
//! An account change is recorded after it is made. A change made but not
//! recorded (the database failing between the two) is logged, and the
//! request still succeeds: refusing it would tell the caller a change
//! failed that did not. A write to a feeder cannot miss its entry: the
//! journal row that precedes every send brings it.

use wellfed_api::{AuditAction, AuditEntry, AuditPage, PageQuery, User};
use wellfed_store::{AuditFilter, AuditRecord, AuditRow, NewAudit};
use tracing::error;

use crate::journal::view;
use crate::{Caller, Permission, Result, Service, can, page};

impl Service {
    /// A page of the log: the caller's own entries, or everyone's for an
    /// owner.
    pub async fn audit(&self, user: &User, query: PageQuery) -> Result<AuditPage> {
        let limit = page::limit(&query)?;
        let after = page::after_seq(&query)?;

        // One more than the page shows whether another page follows.
        let rows = self
            .shared
            .store
            .audit(AuditFilter {
                user: (!can(user.role, Permission::AllHistory)).then_some(user.id),
                after,
                limit: limit + 1,
            })
            .await?;
        let (rows, next) = page::split(rows, limit, |row| row.seq.to_string());
        Ok(AuditPage {
            items: rows.into_iter().map(entry).collect(),
            next,
        })
    }

    /// Record what `caller` did.
    pub(crate) async fn record(&self, caller: &Caller, action: AuditAction) {
        self.record_as(Some(&caller.user), caller.token, action)
            .await;
    }

    /// Record `action`, done by `user` (through `token`) or by the host.
    pub(crate) async fn record_as(
        &self,
        user: Option<&User>,
        token: Option<wellfed_api::TokenId>,
        action: AuditAction,
    ) {
        let entry = NewAudit {
            at: self.shared.clock.now(),
            user: user.map(|user| user.id),
            user_name: user.map(|user| user.name.as_str()),
            token,
            action: &action,
        };
        if let Err(failure) = self.shared.store.record_audit(entry).await {
            error!(?action, %failure, "a change was made but not recorded in the audit log");
        }
    }
}

fn entry(row: AuditRow) -> AuditEntry {
    let action = match row.record {
        AuditRecord::Change(action) => action,
        AuditRecord::Command(command) => AuditAction::Command {
            command: view(*command),
        },
    };
    AuditEntry {
        at: row.at,
        user: row.user,
        user_name: row.user_name,
        token: row.token,
        action,
    }
}
