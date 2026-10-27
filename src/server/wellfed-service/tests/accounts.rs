//! Household accounts: the first owner's setup, login and logout.

use wellfed_api::{Credentials, Role};
use wellfed_server_testkit::{TestService, local};
use wellfed_service::{Error, Invalid};

#[test]
fn the_first_owner_sets_up_once_and_logs_in() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        assert!(service.setup_open().await.unwrap());

        let weak = Credentials {
            name: "ada".into(),
            password: "short".into(),
        };
        assert!(matches!(
            service.setup(weak).await,
            Err(Error::Invalid(Invalid::Password))
        ));
        let credentials = Credentials {
            name: " ada ".into(),
            password: "correct horse".into(),
        };
        let (user, token) = service.setup(credentials.clone()).await.unwrap();
        assert_eq!((user.name.as_str(), user.role), ("ada", Role::Owner));
        assert!(!service.setup_open().await.unwrap());
        assert!(matches!(
            service.setup(credentials.clone()).await,
            Err(Error::SetupClosed)
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
            Err(Error::BadCredentials)
        ));
        let nobody = Credentials {
            name: "bob".into(),
            ..credentials.clone()
        };
        assert!(matches!(
            service.login(nobody).await,
            Err(Error::BadCredentials)
        ));
        let (again, second) = service.login(credentials).await.unwrap();
        assert_eq!(again, user);

        service.logout(&token).await.unwrap();
        assert_eq!(service.authenticate(&token).await.unwrap(), None);
        assert_eq!(service.authenticate(&second).await.unwrap(), Some(user));
        server.shutdown().await;
    });
}
