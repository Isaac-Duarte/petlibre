//! The accounts owners manage: roles, expiry, temporary passwords, the
//! last owner, sliding sessions and `reset-owner`.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{Credentials, NewUser, PasswordChange, Role, User, UserPatch};
use wellfed_server_testkit::fixed::{ManualClock, SeededRandom};
use wellfed_server_testkit::{TestService, ada_login, ada_setup, local};
use wellfed_service::{AccountError, Caller, Error, Invalid, Service, SessionToken, reset_owner};

const HOUR: Duration = Duration::from_secs(3600);

fn new_user(name: &str, role: Role, expires: Option<Timestamp>) -> NewUser {
    NewUser {
        name: name.into(),
        password: "temporary pass".into(),
        role,
        expires,
    }
}

fn login(name: &str, password: &str) -> Credentials {
    Credentials {
        name: name.into(),
        password: password.into(),
    }
}

/// An owner asking from a login, as account changes must.
fn from_login() -> Caller {
    wellfed_server_testkit::ada().into()
}

async fn setup(service: &Service) -> (User, SessionToken) {
    service.setup(ada_setup()).await.unwrap()
}

#[test]
fn owners_add_change_and_delete_users_but_never_the_last_owner() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let (ada, _) = setup(service).await;

        let bob = service
            .create_user(&from_login(), new_user("bob", Role::Member, None))
            .await
            .unwrap();
        assert!(matches!(
            service
                .create_user(&from_login(), new_user(" bob ", Role::Viewer, None))
                .await,
            Err(Error::Account(AccountError::NameTaken))
        ));
        let short = NewUser {
            password: "nine char".into(),
            ..new_user("cy", Role::Member, None)
        };
        assert!(matches!(
            service.create_user(&from_login(), short).await,
            Err(Error::Invalid(Invalid::Password))
        ));
        let expiring_owner = new_user("cy", Role::Owner, Some(Timestamp::UNIX_EPOCH));
        assert!(matches!(
            service.create_user(&from_login(), expiring_owner).await,
            Err(Error::Invalid(Invalid::OwnerExpires))
        ));
        let names: Vec<_> = service.users().await.unwrap();
        assert_eq!(names, [ada.clone(), bob.clone()]);

        // The only owner keeps the role and the account.
        let demote = UserPatch {
            role: Some(Role::Member),
            ..UserPatch::default()
        };
        assert!(matches!(
            service
                .update_user(&ada.clone().into(), None, ada.id, demote.clone())
                .await,
            Err(Error::Account(AccountError::LastOwner))
        ));
        assert!(matches!(
            service.delete_user(ada.id).await,
            Err(Error::Account(AccountError::LastOwner))
        ));

        // A second owner lets the first step down, and go.
        let promote = UserPatch {
            role: Some(Role::Owner),
            ..UserPatch::default()
        };
        let bob = service
            .update_user(&ada.clone().into(), None, bob.id, promote)
            .await
            .unwrap();
        assert_eq!(bob.role, Role::Owner);
        let ada = service
            .update_user(&ada.clone().into(), None, ada.id, demote)
            .await
            .unwrap();
        assert_eq!(ada.role, Role::Member);
        service.delete_user(ada.id).await.unwrap();
        assert!(matches!(
            service.delete_user(ada.id).await,
            Err(Error::Account(AccountError::NotFound))
        ));
        assert_eq!(service.users().await.unwrap(), [bob]);
        server.shutdown().await;
    });
}

#[test]
fn deleting_a_user_ends_their_sessions() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        setup(service).await;
        let bob = service
            .create_user(&from_login(), new_user("bob", Role::Viewer, None))
            .await
            .unwrap();
        let (_, token) = service.login(login("bob", "temporary pass")).await.unwrap();
        assert!(service.authenticate(&token).await.unwrap().is_some());

        service.delete_user(bob.id).await.unwrap();

        assert_eq!(service.authenticate(&token).await.unwrap(), None);
        server.shutdown().await;
    });
}

#[test]
fn a_password_change_ends_the_users_other_sessions() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let (ada, ada_token) = setup(service).await;
        let bob = service
            .create_user(&from_login(), new_user("bob", Role::Member, None))
            .await
            .unwrap();
        let (_, bob_token) = service.login(login("bob", "temporary pass")).await.unwrap();

        let reset = UserPatch {
            password: Some("another temporary".into()),
            ..UserPatch::default()
        };
        service
            .update_user(&ada.clone().into(), None, bob.id, reset)
            .await
            .unwrap();
        assert_eq!(service.authenticate(&bob_token).await.unwrap(), None);
        assert!(matches!(
            service.login(login("bob", "temporary pass")).await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        service
            .login(login("bob", "another temporary"))
            .await
            .unwrap();

        // An owner setting their own keeps the session they asked on and
        // ends the others.
        let (_, elsewhere) = service.login(ada_login()).await.unwrap();
        let own = UserPatch {
            password: Some("a better horse".into()),
            ..UserPatch::default()
        };
        service
            .update_user(&ada.clone().into(), Some(&ada_token), ada.id, own)
            .await
            .unwrap();
        assert!(service.authenticate(&ada_token).await.unwrap().is_some());
        assert_eq!(service.authenticate(&elsewhere).await.unwrap(), None);
        let short = UserPatch {
            password: Some("short".into()),
            ..UserPatch::default()
        };
        assert!(matches!(
            service
                .update_user(&ada.clone().into(), None, ada.id, short)
                .await,
            Err(Error::Invalid(Invalid::Password))
        ));
        server.shutdown().await;
    });
}

/// Any role changes its own password with the current one; the session it
/// asked on stays and the others end.
#[test]
fn a_user_changes_their_own_password() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        setup(service).await;
        service
            .create_user(&from_login(), new_user("vic", Role::Viewer, None))
            .await
            .unwrap();
        let (vic, here) = service.login(login("vic", "temporary pass")).await.unwrap();
        let (_, there) = service.login(login("vic", "temporary pass")).await.unwrap();

        let wrong = PasswordChange {
            current: "not the password".into(),
            password: "my own secret".into(),
        };
        assert!(matches!(
            service
                .change_password(&vic.clone().into(), Some(&here), wrong)
                .await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        let short = PasswordChange {
            current: "temporary pass".into(),
            password: "too short".into(),
        };
        assert!(matches!(
            service
                .change_password(&vic.clone().into(), Some(&here), short)
                .await,
            Err(Error::Invalid(Invalid::Password))
        ));
        assert!(service.authenticate(&there).await.unwrap().is_some());

        let change = PasswordChange {
            current: "temporary pass".into(),
            password: "my own secret".into(),
        };
        service
            .change_password(&vic.clone().into(), Some(&here), change)
            .await
            .unwrap();
        assert!(service.authenticate(&here).await.unwrap().is_some());
        assert_eq!(service.authenticate(&there).await.unwrap(), None);
        assert!(matches!(
            service.login(login("vic", "temporary pass")).await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        service.login(login("vic", "my own secret")).await.unwrap();
        server.shutdown().await;
    });
}

/// Past its expiry an account is disabled, not deleted: it cannot log in,
/// its session stops, and an owner extending it lets it log in afresh.
#[test]
fn an_expired_account_is_disabled_until_an_owner_extends_it() {
    local(async {
        let clock = ManualClock::new();
        let server = TestService::start_with_clock(Arc::new(clock.clone())).await;
        let service = server.service();
        let (ada, _) = setup(service).await;
        let expires = clock_now(&clock) + HOUR / 2;
        let sitter = service
            .create_user(
                &from_login(),
                new_user("sitter", Role::Member, Some(expires)),
            )
            .await
            .unwrap();
        let (_, token) = service
            .login(login("sitter", "temporary pass"))
            .await
            .unwrap();

        clock.advance(HOUR * 2 / 3);
        assert!(matches!(
            service.authenticate(&token).await,
            Err(Error::Account(AccountError::Disabled))
        ));
        assert!(matches!(
            service.login(login("sitter", "temporary pass")).await,
            Err(Error::Account(AccountError::Disabled))
        ));
        assert!(matches!(
            service.login(login("sitter", "wrong password")).await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        assert!(
            service.users().await.unwrap().contains(&sitter),
            "kept, not deleted"
        );

        let extend = UserPatch {
            expires: Some(Some(clock_now(&clock) + 24 * HOUR)),
            ..UserPatch::default()
        };
        service
            .update_user(&ada.clone().into(), None, sitter.id, extend)
            .await
            .unwrap();
        assert_eq!(
            service.authenticate(&token).await.unwrap(),
            None,
            "the old session does not come back"
        );
        service
            .login(login("sitter", "temporary pass"))
            .await
            .unwrap();

        let expiring_owner = UserPatch {
            role: Some(Role::Owner),
            ..UserPatch::default()
        };
        assert!(matches!(
            service
                .update_user(&ada.clone().into(), None, sitter.id, expiring_owner)
                .await,
            Err(Error::Invalid(Invalid::OwnerExpires))
        ));
        server.shutdown().await;
    });
}

/// A session lasts its lifetime (an hour in the kit) from its last use.
#[test]
fn a_session_slides_with_use_and_ends_when_idle() {
    local(async {
        let clock = ManualClock::new();
        let server = TestService::start_with_clock(Arc::new(clock.clone())).await;
        let service = server.service();
        let (_, token) = setup(service).await;

        for _ in 0..3 {
            clock.advance(HOUR * 5 / 6);
            assert!(
                service.authenticate(&token).await.unwrap().is_some(),
                "used within the hour"
            );
        }
        clock.advance(HOUR + Duration::from_secs(1));
        assert_eq!(service.authenticate(&token).await.unwrap(), None);
        assert_eq!(
            service.authenticate(&token).await.unwrap(),
            None,
            "and gone"
        );
        server.shutdown().await;
    });
}

#[test]
fn reset_owner_gives_the_sole_owner_a_new_password() {
    local(async {
        let server = TestService::start().await;
        let (ada, token) = setup(server.service()).await;
        server
            .service()
            .create_user(&from_login(), new_user("bob", Role::Member, None))
            .await
            .unwrap();
        let temp = server.stop().await;

        let store = temp.open().await;
        let random = SeededRandom::fresh();
        let (owner, password) = reset_owner(&store, None, &random).await.unwrap();
        assert_eq!(owner, ada);
        assert!(password.len() >= 10, "{password}");
        assert!(matches!(
            reset_owner(&store, Some("bob"), &random).await,
            Err(Error::Account(AccountError::NotAnOwner(name))) if name == "bob"
        ));
        assert!(matches!(
            reset_owner(&store, Some("nobody"), &random).await,
            Err(Error::Account(AccountError::NotFound))
        ));
        store.close().await;

        let server = TestService::reopen(temp).await;
        let service = server.service();
        assert_eq!(service.authenticate(&token).await.unwrap(), None);
        assert!(matches!(
            service.login(ada_login()).await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        service.login(login("ada", &password)).await.unwrap();

        // With two owners, the host names one.
        let promote = UserPatch {
            role: Some(Role::Owner),
            ..UserPatch::default()
        };
        let bob = service.users().await.unwrap()[1].clone();
        service
            .update_user(&ada.clone().into(), None, bob.id, promote)
            .await
            .unwrap();
        let temp = server.stop().await;
        let store = temp.open().await;
        assert!(matches!(
            reset_owner(&store, None, &random).await,
            Err(Error::Account(AccountError::OwnerAmbiguous))
        ));
        let (owner, _) = reset_owner(&store, Some("bob"), &random).await.unwrap();
        assert_eq!(owner.name, "bob");
        store.close().await;
    });
}

fn clock_now(clock: &ManualClock) -> Timestamp {
    wellfed_service::Clock::now(clock)
}
