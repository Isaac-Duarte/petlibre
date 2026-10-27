//! The audit log: account changes recorded with who made them and through
//! which token, failed logins against the account tried, journal writes
//! with their state as it stands, and each user reading their own.

use std::net::{IpAddr, Ipv4Addr};

use wellfed_api::{
    AuditAction, AuditEntry, CommandOutcome, CommandState, Connection as Link, Credentials,
    HouseholdPatch, LoginFailure, NewToken, NewUser, PageQuery, Role, User, UserPatch,
};
use wellfed_server_testkit::feeder;
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{HERE, TestService, ada_login, ada_setup, local};
use wellfed_service::{Caller, Error, Invalid, Service};

const AWAY: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

fn all() -> PageQuery {
    PageQuery::default()
}

fn login(name: &str, password: &str) -> Credentials {
    Credentials {
        name: name.into(),
        password: password.into(),
    }
}

async fn entries(service: &Service, reader: &User) -> Vec<AuditEntry> {
    service.audit(reader, all()).await.unwrap().items
}

fn actions(entries: &[AuditEntry]) -> Vec<&AuditAction> {
    entries.iter().map(|entry| &entry.action).collect()
}

async fn add_member(service: &Service, owner: &User) -> User {
    let request = NewUser {
        name: "bob".into(),
        password: "temporary pass".into(),
        role: Role::Member,
        expires: None,
    };
    let owner = Caller::from(owner.clone());
    service.create_user(&owner, request).await.unwrap()
}

/// Every account change names who made it, when, through which token,
/// and what it changed; a refused change leaves nothing.
#[test]
fn account_changes_are_recorded_with_their_actor_and_token() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let (ada, _) = service.setup(ada_setup()).await.unwrap();
        let owner = Caller::from(ada.clone());

        let bob = add_member(service, &ada).await;
        let promote = UserPatch {
            role: Some(Role::Viewer),
            expires: None,
            password: Some("another temporary".into()),
        };
        service
            .update_user(&owner, None, bob.id, promote)
            .await
            .unwrap();
        let phone = NewToken {
            name: "phone".into(),
            expires: None,
        };
        let created = service.create_token(&owner, phone).await.unwrap();
        let through_phone = Caller {
            user: ada.clone(),
            token: Some(created.token.id),
        };
        let renamed = HouseholdPatch {
            name: Some("Flat".into()),
            ..HouseholdPatch::default()
        };
        let household = service
            .update_household(&through_phone, renamed)
            .await
            .unwrap();
        service
            .revoke_token(&through_phone, created.token.id)
            .await
            .unwrap();
        let refused = service.create_user(
            &owner,
            NewUser {
                name: "x".into(),
                password: "short".into(),
                role: Role::Viewer,
                expires: None,
            },
        );
        assert!(matches!(
            refused.await,
            Err(Error::Invalid(Invalid::Password))
        ));
        service.delete_user(&owner, bob.id).await.unwrap();

        let log = entries(service, &ada).await;
        let (account, bob_name) = (bob.id, "bob".to_owned());
        assert_eq!(
            actions(&log),
            [
                &AuditAction::Setup,
                &AuditAction::UserCreated {
                    account,
                    name: bob_name.clone(),
                    role: Role::Member,
                    expires: None,
                },
                &AuditAction::UserChanged {
                    account,
                    name: bob_name.clone(),
                    role: Role::Viewer,
                    expires: None,
                },
                &AuditAction::PasswordSet {
                    account,
                    name: bob_name.clone(),
                },
                &AuditAction::TokenCreated {
                    token: created.token.id,
                    account: ada.id,
                    token_name: "phone".into(),
                },
                &AuditAction::HouseholdChanged { household },
                &AuditAction::TokenRevoked {
                    token: created.token.id,
                    account: ada.id,
                    token_name: "phone".into(),
                },
                &AuditAction::UserDeleted {
                    account,
                    name: bob_name,
                },
            ]
        );
        assert!(log.iter().all(|entry| entry.user == Some(ada.id)
            && entry.user_name.as_deref() == Some("ada")
            && entry.at == FixedClock.now()));
        let tokens: Vec<_> = log.iter().map(|entry| entry.token.is_some()).collect();
        assert_eq!(
            tokens,
            [false, false, false, false, false, true, true, false],
            "the household change and the revocation came through the phone"
        );
        server.shutdown().await;
    });
}

/// A login and a failed one name the client's address. A failure with an
/// account's name is that account's entry; one with no account's name
/// names nobody, so it is not recorded and cannot fill the log.
#[test]
fn logins_and_failures_are_recorded_against_the_account() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let (ada, _) = service.setup(ada_setup()).await.unwrap();

        service.login(ada_login(), AWAY).await.unwrap();
        service
            .login(login("ada", "wrong horse"), HERE)
            .await
            .unwrap_err();
        service
            .login(login("nobody", "wrong horse"), HERE)
            .await
            .unwrap_err();

        let log = entries(service, &ada).await;
        assert_eq!(
            actions(&log),
            [
                &AuditAction::Setup,
                &AuditAction::Login { from: AWAY },
                &AuditAction::LoginFailed {
                    from: HERE,
                    reason: LoginFailure::BadPassword,
                },
            ]
        );
        server.shutdown().await;
    });
}

/// A disabled account's right password is a failure of its own kind.
#[test]
fn a_disabled_accounts_login_is_recorded_as_disabled() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let (ada, _) = service.setup(ada_setup()).await.unwrap();
        let owner = Caller::from(ada.clone());
        let sitter = NewUser {
            name: "sitter".into(),
            password: "temporary pass".into(),
            role: Role::Member,
            expires: Some(FixedClock.now()),
        };
        let sitter = service.create_user(&owner, sitter).await.unwrap();

        service
            .login(login("sitter", "temporary pass"), HERE)
            .await
            .unwrap_err();

        let own = entries(service, &sitter).await;
        assert_eq!(
            actions(&own),
            [&AuditAction::LoginFailed {
                from: HERE,
                reason: LoginFailure::Disabled,
            }]
        );
        server.shutdown().await;
    });
}

/// A member reads only what they did; an owner reads everyone's, in
/// pages.
#[test]
fn each_user_reads_their_own_entries_and_an_owner_reads_all_in_pages() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let (ada, _) = service.setup(ada_setup()).await.unwrap();
        add_member(service, &ada).await;
        let (bob, _) = service
            .login(login("bob", "temporary pass"), HERE)
            .await
            .unwrap();
        service.login(ada_login(), HERE).await.unwrap();

        let own = entries(service, &bob).await;
        assert_eq!(actions(&own), [&AuditAction::Login { from: HERE }]);
        assert!(own.iter().all(|entry| entry.user == Some(bob.id)));

        let first = service
            .audit(
                &ada,
                PageQuery {
                    after: None,
                    limit: Some(3),
                },
            )
            .await
            .unwrap();
        assert_eq!(first.items.len(), 3);
        let next = first.next.expect("a fourth entry follows");
        let rest = service
            .audit(
                &ada,
                PageQuery {
                    after: Some(next),
                    limit: Some(3),
                },
            )
            .await
            .unwrap();
        assert_eq!(actions(&rest.items), [&AuditAction::Login { from: HERE }]);
        assert_eq!(rest.next, None);

        for bad in [
            PageQuery {
                after: Some("x".into()),
                limit: None,
            },
            PageQuery {
                after: None,
                limit: Some(0),
            },
        ] {
            assert!(matches!(
                service.audit(&ada, bad).await,
                Err(Error::Invalid(_))
            ));
        }
        server.shutdown().await;
    });
}

/// A write to a feeder is in the log from the moment it is journalled,
/// with its state and outcome as the journal holds them; a write the
/// feeder never received leaves no entry.
#[test]
fn a_journal_write_is_an_entry_with_its_current_state() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let service = server.service();
        let (ada, _) = service.setup(ada_setup()).await.unwrap();
        let owner = Caller::from(ada.clone());
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;

        service
            .set_lock(&owner, id, true, Some([0x11; 16]))
            .await
            .unwrap();
        sim.stop().await;
        server.link(id, Link::Offline).await;
        service
            .set_lock(&owner, id, false, Some([0x22; 16]))
            .await
            .unwrap_err();

        let log = entries(service, &ada).await;
        let commands: Vec<_> = log
            .iter()
            .filter_map(|entry| match &entry.action {
                AuditAction::Command { command } => Some(command),
                _ => None,
            })
            .collect();
        assert_eq!(commands.len(), 1, "the unsent write left no entry");
        let lock = commands[0];
        assert_eq!(lock.key.as_bytes(), &[0x11; 16]);
        assert_eq!(
            (lock.state, lock.outcome.clone()),
            (CommandState::Answered, Some(CommandOutcome::Done))
        );
        let entry = log
            .iter()
            .find(|entry| matches!(entry.action, AuditAction::Command { .. }))
            .unwrap();
        assert_eq!(
            (entry.user, entry.user_name.as_deref(), entry.at),
            (Some(ada.id), Some("ada"), lock.sent)
        );
        server.shutdown().await;
    });
}
