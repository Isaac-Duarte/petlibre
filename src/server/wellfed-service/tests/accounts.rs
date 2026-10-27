//! Household accounts: the first owner's setup, login and logout.

use wellfed_api::{Credentials, Role, Setup};
use wellfed_server_testkit::{TestService, local};
use wellfed_service::{AccountError, Error, Invalid};

#[test]
fn the_first_owner_sets_up_once_and_logs_in() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        assert!(service.setup_open().await.unwrap());

        let weak = Setup {
            name: "ada".into(),
            password: "nine char".into(),
            zone: "UTC".into(),
        };
        assert!(matches!(
            service.setup(weak).await,
            Err(Error::Invalid(Invalid::Password))
        ));
        let setup = Setup {
            name: " ada ".into(),
            password: "correct horse".into(),
            zone: "UTC".into(),
        };
        let credentials = Credentials {
            name: "ada".into(),
            password: setup.password.clone(),
        };
        let (user, token) = service.setup(setup.clone()).await.unwrap();
        assert_eq!((user.name.as_str(), user.role), ("ada", Role::Owner));
        assert!(!service.setup_open().await.unwrap());
        assert!(matches!(
            service.setup(setup).await,
            Err(Error::Account(AccountError::SetupClosed))
        ));
        assert_eq!(
            service.authenticate(&token).await.unwrap(),
            Some(user.clone())
        );

        let wrong = Credentials {
            password: "wrong horse".into(),
            ..credentials.clone()
        };
        assert!(matches!(
            service.login(wrong).await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        let nobody = Credentials {
            name: "bob".into(),
            ..credentials.clone()
        };
        assert!(matches!(
            service.login(nobody).await,
            Err(Error::Account(AccountError::BadCredentials))
        ));
        let (again, second) = service.login(credentials).await.unwrap();
        assert_eq!(again, user);

        service.logout(&token).await.unwrap();
        assert_eq!(service.authenticate(&token).await.unwrap(), None);
        assert_eq!(service.authenticate(&second).await.unwrap(), Some(user));
        server.shutdown().await;
    });
}
