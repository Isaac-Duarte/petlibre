//! `GET /audit` over HTTP: everyone reads their own entries, an owner
//! everyone's, each naming who acted, when and through which token.

use wellfed_api::{
    AuditAction, Credentials, ErrorCode, NewToken, NewUser, PageQuery, Role, UserPatch,
};
use wellfed_server_client::{Auth, Client, StatusCode};
use wellfed_server_testkit::{TestServer, ada_setup, local, refused};

const PASSWORD: &str = "temporary pass";

async fn login(server: &TestServer, name: &str) -> Client {
    let mut client = server.client();
    let credentials = Credentials {
        name: name.into(),
        password: PASSWORD.into(),
    };
    client.login(&credentials).await.unwrap();
    client
}

fn kinds(page: &wellfed_api::AuditPage) -> Vec<String> {
    page.items
        .iter()
        .map(|entry| {
            let json = serde_json::to_value(&entry.action).unwrap();
            json["kind"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn each_role_reads_its_own_entries_and_an_owner_reads_all() {
    local(async {
        let server = TestServer::start().await;
        let mut owner = server.client();
        let ada = owner.setup(&ada_setup()).await.unwrap();
        for (name, role) in [("bob", Role::Member), ("vic", Role::Viewer)] {
            let new = NewUser {
                name: name.into(),
                password: PASSWORD.into(),
                role,
                expires: None,
            };
            owner.create_user(&new).await.unwrap();
        }
        let bob_session = login(&server, "bob").await;
        let phone = NewToken {
            name: "phone".into(),
            expires: None,
        };
        let token = bob_session.create_token(&phone).await.unwrap();
        let bob = server
            .client()
            .with_auth(Some(Auth::Token(token.secret.clone())));
        let vic = login(&server, "vic").await;
        let demote = UserPatch {
            role: Some(Role::Viewer),
            ..UserPatch::default()
        };
        let users = owner.users().await.unwrap();
        let bob_user = users.iter().find(|user| user.name == "bob").unwrap();
        owner.update_user(bob_user.id, &demote).await.unwrap();

        let all = owner.audit(&PageQuery::default()).await.unwrap();
        assert_eq!(
            kinds(&all),
            [
                "setup",
                "user_created",
                "user_created",
                "login",
                "token_created",
                "login",
                "user_changed"
            ]
        );
        assert_eq!(all.items[0].user, Some(ada.id));

        let own = bob.audit(&PageQuery::default()).await.unwrap();
        assert_eq!(kinds(&own), ["login", "token_created"]);
        assert!(
            own.items
                .iter()
                .all(|entry| entry.user == Some(bob_user.id))
        );
        let AuditAction::TokenCreated { token: made, .. } = own.items[1].action else {
            panic!("a token's entry");
        };
        assert_eq!(made, token.token.id);
        assert_eq!(own.items[1].token, None, "made from a login");

        let viewer = vic.audit(&PageQuery::default()).await.unwrap();
        assert_eq!(kinds(&viewer), ["login"]);

        refused(
            server.client().audit(&PageQuery::default()).await,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );
        let bad = PageQuery {
            after: Some("not a cursor".into()),
            limit: None,
        };
        refused(
            owner.audit(&bad).await,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        );
        server.stop().await;
    });
}
