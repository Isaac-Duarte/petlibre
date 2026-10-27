//! `wellfed server …` as a process against the server in process and the
//! simulated feeder: the login and its token file, a feed, `--json`, the
//! exit statuses, a feed whose answer is lost, which exits 3 with its key
//! and is never sent again, an install followed to its end, the feeder's
//! stock backup read, shown and exported, and never signed when it is not
//! stock's pinned image, and a feeder's clients and restart, and an export saved
//! whole.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use wellfed_server_client::api::{
    AdoptFeeder, ClientOwner, ClientRole, Command, CommandOutcome, CommandState, Connection,
    ErrorCode, FeedOutcome, Feeder, FeederClient, FeederId, Hex, Install, InstallState, Problem,
    StockBackup, StockSource, StockState,
};
use wellfed_server_client::{Client, Uploaded};
use wellfed_server_testkit::feeder::{self, Banks, Flash, Relay, Running};
use wellfed_server_testkit::release;
use wellfed_server_testkit::{TestServer, ada_login, ada_setup, config, local, until};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const BUILD: [u8; 32] = [0x77; 32];

/// What one run of the CLI printed, and how it exited.
struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Ran {
    #[track_caller]
    fn exited(&self, code: i32) -> &Self {
        assert_eq!(self.code, Some(code), "{}\n{}", self.stdout, self.stderr);
        self
    }

    /// Each line of standard output as one JSON body.
    fn bodies<T: serde::de::DeserializeOwned>(&self) -> Vec<T> {
        self.stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is one body"))
            .collect()
    }
}

/// The CLI's token file, in a directory `login` creates.
struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Self {
        Self(tempfile::tempdir().expect("a temporary directory"))
    }

    fn token_file(&self) -> PathBuf {
        self.0.path().join("private").join("wellfed-server.json")
    }

    /// Run `wellfed server <args>` with this token file, `stdin` on its
    /// standard input. Awaited, so the server and the simulator keep
    /// running on this test's thread meanwhile.
    async fn run(&self, args: &[&str], stdin: &str) -> Ran {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_wellfed"))
            .arg("server")
            .args(args)
            .arg("--token-file")
            .arg(self.token_file())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("the CLI starts");

        let mut input = child.stdin.take().expect("a piped standard input");
        input
            .write_all(stdin.as_bytes())
            .await
            .expect("write stdin");
        drop(input);

        let output = child.wait_with_output().await.expect("the CLI ends");
        Ran {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).expect("UTF-8 output"),
            stderr: String::from_utf8(output.stderr).expect("UTF-8 errors"),
        }
    }

    async fn login(&self, server: &TestServer) -> Ran {
        let url = format!("http://{}", server.address());
        let password = format!("{}\n", ada_login().password);
        self.run(&["login", "--server", &url, "--name", "ada"], &password)
            .await
    }
}

/// Adopt `sim` reached at `address` over HTTP, and wait until the server
/// is connected.
async fn adopt(http: &Client, sim: &Running, address: String) -> FeederId {
    let request = AdoptFeeder {
        invite: feeder::invite(sim, ClientRole::Server).await,
        name: "kitchen".into(),
        address: Some(address),
    };
    let id = http.adopt(&request).await.unwrap().id;
    until("the server connects to the feeder", || async {
        http.feeders().await.unwrap()[0].connection == Connection::Online
    })
    .await;
    id
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_login_keeps_a_private_token_that_feeds_and_logout_revokes_it() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&http, &sim, sim.address.to_string()).await;
        let home = Home::new();

        // A wrong password: the problem as the body, exit 1, no file.
        let url = format!("http://{}", server.address());
        let wrong = ["login", "--server", &url, "--name", "ada", "--json"];
        let refused = home.run(&wrong, "not the password\n").await;
        let problems: Vec<Problem> = refused.exited(1).bodies();
        assert_eq!(problems[0].code, ErrorCode::BadCredentials);
        assert!(!home.token_file().exists());

        // The token is kept mode 0600, and the login session is ended.
        home.login(&server).await.exited(0);
        assert_eq!(mode(&home.token_file()), 0o600);
        let tokens = http.tokens().await.unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].name, "wellfed-cli");
        let again = home.login(&server).await;
        assert!(again.exited(1).stderr.contains("already logged in"));

        let listed = home.run(&["feeders", "--json"], "").await;
        let feeders: Vec<Vec<Feeder>> = listed.exited(0).bodies();
        assert_eq!(feeders[0].len(), 1);
        assert_eq!(feeders[0][0].id, kitchen);

        // A feed with --json: the entry the feeder reserved, then the one
        // its history settled, both journalled under the token.
        let fed = home.run(&["feed", "--portions", "2", "--json"], "").await;
        let entries: Vec<Command> = fed.exited(0).bodies();
        assert_eq!(entries.len(), 2, "{}", fed.stdout);
        assert_eq!(entries[0].feed, Some(1));
        let settled = &entries[1];
        assert_eq!(settled.key, entries[0].key);
        assert_eq!(settled.state, CommandState::Resolved);
        let Some(CommandOutcome::Fed {
            completed, outcome, ..
        }) = settled.outcome
        else {
            panic!("{settled:?}");
        };
        assert_eq!((completed, outcome), (2, FeedOutcome::Done));
        assert_eq!(settled.token, Some(tokens[0].id));
        assert_eq!(feeder::feeds(&flash), 1);

        // The same as text.
        let fed = home.run(&["feed", "--feeder", "kitchen"], "").await;
        let text = &fed.exited(0).stdout;
        assert!(text.contains("Feed 2 started"), "{text}");
        assert!(text.contains("feed 2: 1 of 1 portions, done"), "{text}");
        assert_eq!(feeder::feeds(&flash), 2);

        let status = home.run(&["status"], "").await;
        assert!(status.exited(0).stdout.contains("kitchen"));
        home.run(&["status", "--feeder", "attic"], "")
            .await
            .exited(1);

        // Logout revokes the token and deletes the file.
        home.run(&["logout"], "").await.exited(0);
        assert!(!home.token_file().exists());
        assert!(http.tokens().await.unwrap().is_empty());
        let out = home.run(&["feeders"], "").await;
        assert!(out.exited(1).stderr.contains("not logged in"));

        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn an_uncertain_feed_exits_3_with_its_key_and_is_never_resent() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        // The server gives up on an answer after 2 s, not 12.
        let mut quick = config();
        quick.timing.answer = std::time::Duration::from_secs(2);
        let server = TestServer::start_with(quick).await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        adopt(&http, &sim, relay.address.to_string()).await;
        let home = Home::new();
        home.login(&server).await.exited(0);

        // The feeder takes the feed, but its answer never comes back.
        relay.hold_answers();
        let lost = home.run(&["feed"], "").await;
        let text = &lost.exited(3).stdout;
        let key = text
            .lines()
            .find_map(|line| line.strip_prefix("Key: "))
            .and_then(|key| Uuid::parse_str(key).ok())
            .unwrap_or_else(|| panic!("the key is printed: {text}"));
        assert!(text.contains(&format!("wellfed server command {key}")));
        assert_eq!(
            http.command(key).await.unwrap().state,
            CommandState::Unknown
        );
        home.run(&["command", &key.to_string()], "").await.exited(3);

        // Once the server reconnects, the feeder's history settles it.
        relay.cut();
        relay.resume();
        until("the feed is settled", || async {
            http.command(key)
                .await
                .is_ok_and(|entry| entry.state == CommandState::Resolved)
        })
        .await;
        let resolved = home.run(&["command", &key.to_string(), "--json"], "").await;
        let entries: Vec<Command> = resolved.exited(0).bodies();
        assert!(matches!(
            entries[0].outcome,
            Some(CommandOutcome::Fed {
                outcome: FeedOutcome::Done,
                ..
            })
        ));

        // One feed: the CLI never sent it again.
        assert_eq!(feeder::feeds(&flash), 1);
        server.stop().await;
        sim.stop().await;
    });
}

/// `clients` lists whose each client is, `revoke` drops one by a prefix of
/// its key, and `reboot` restarts the feeder, on a server adopted with an
/// owner's invite.
#[test]
fn clients_revoke_and_reboot_go_through_the_server() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let request = AdoptFeeder {
            invite: feeder::invite(&sim, ClientRole::Owner).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        };
        http.adopt(&request).await.unwrap();
        until("the server connects to the feeder", || async {
            http.feeders().await.unwrap()[0].connection == Connection::Online
        })
        .await;
        let home = Home::new();
        home.login(&server).await.exited(0);

        // A client that joined at the feeder, not through the server.
        let invite = feeder::invite(&sim, ClientRole::Member).await;
        feeder::join_phone(&sim, &invite, [9; 32]).await;
        let stranger = Hex(feeder::phone_key([9; 32])).to_string();

        let listed = home.run(&["clients", "--json"], "").await;
        let clients: Vec<Vec<FeederClient>> = listed.exited(0).bodies();
        let theirs = clients[0]
            .iter()
            .find(|client| client.key.to_string() == stranger)
            .unwrap();
        assert_eq!(theirs.owner, ClientOwner::Unowned);
        let text = home.run(&["clients"], "").await;
        assert!(text.exited(0).stdout.contains("the server"));

        home.run(&["revoke", "abc"], "").await.exited(1);
        home.run(&["revoke", &stranger[..12]], "").await.exited(0);
        assert!(
            !feeder::client_keys(&sim)
                .await
                .contains(&feeder::phone_key([9; 32]))
        );

        let restarted = home.run(&["reboot"], "").await;
        assert!(restarted.exited(0).stdout.contains("restarts"));
        server.stop().await;
        sim.stop().await;
    });
}

/// `install` starts a kept release and follows it to activation, reads it
/// back without a release, and a second run of the build the feeder now
/// runs ends `already_running`, with nothing sent.
#[test]
fn install_follows_a_release_to_activation() {
    local(async {
        let banks = Banks::new();
        let sim = feeder::start_updatable(&banks).await;
        let server = TestServer::start_with(release::trusting(config())).await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let request = AdoptFeeder {
            invite: feeder::invite(&sim, ClientRole::Owner).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        };
        let kitchen = http.adopt(&request).await.unwrap().id;
        let file = release::signed(&release::project(), 7, BUILD, 16 * release::SECTOR);
        let Uploaded::New(kept) = http.upload_release(file).await.unwrap() else {
            panic!("a first upload is new");
        };
        until("the server reads the feeder", || async {
            let detail = http.feeder(kitchen).await.ok();
            let status = detail.and_then(|detail| detail.status);
            status.is_some_and(|status| status.live)
        })
        .await;
        let home = Home::new();
        home.login(&server).await.exited(0);
        let id = kept.id.to_string();

        let ran = home.run(&["install", &id, "--json"], "").await;
        let bodies: Vec<Install> = ran.exited(0).bodies();
        assert_eq!(bodies.first().map(|b| b.state), Some(InstallState::Running));
        let last = bodies.last().unwrap();
        assert_eq!(
            (last.state, last.written),
            (InstallState::Activated, kept.image_len)
        );
        let read = home.run(&["install"], "").await;
        assert!(
            read.exited(0).stdout.contains("activated"),
            "{}",
            read.stdout
        );

        until("the feeder answers from the release", || async {
            let detail = http.feeder(kitchen).await.ok();
            let status = detail.and_then(|detail| detail.status);
            status.is_some_and(|status| status.live && status.status.bank == 1)
        })
        .await;
        let again = home.run(&["install", &id], "").await;
        assert!(
            again.exited(0).stdout.contains("already_running"),
            "{}",
            again.stdout
        );
        home.run(&["install", "not-an-id"], "").await.exited(2);

        server.stop().await;
        sim.stop().await;
    });
}

/// `stock read` keeps bank 1 read from the replacement on bank 0, `stock`
/// shows it, `stock export` copies it byte for byte to a new file, and
/// `stock restore` checks the server's copy against stock's pinned image
/// before signing anything: the simulator's synthetic stock is refused,
/// with nothing sent.
#[test]
fn stock_is_read_shown_exported_and_never_signed_unless_pinned() {
    local(async {
        let banks = Banks::new();
        let stock = banks.borrow().bank1().to_vec();
        let sim = feeder::start_updatable(&banks).await;
        let config = release::keeping_stock(release::trusting(config()), &banks);
        let server = TestServer::start_with(config).await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let request = AdoptFeeder {
            invite: feeder::invite(&sim, ClientRole::Owner).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        };
        let kitchen = http.adopt(&request).await.unwrap().id;
        feeder::stock_gave_way(&sim, &banks);
        until("the feeder serves bank 1", || async {
            let state = http.stock(kitchen).await.ok();
            state
                .and_then(|state| state.feeder_state)
                .is_some_and(|feeder| feeder.readable)
        })
        .await;
        let home = Home::new();
        home.login(&server).await.exited(0);

        let read = home.run(&["stock", "read", "--json"], "").await;
        let [kept]: [StockBackup; 1] = read.exited(0).bodies().try_into().unwrap();
        assert_eq!(kept.source, StockSource::Feeder);
        let shown = home.run(&["stock", "--json"], "").await;
        let [state]: [StockState; 1] = shown.exited(0).bodies().try_into().unwrap();
        assert_eq!(state.backup, Some(kept.clone()));
        let text = home.run(&["stock"], "").await;
        assert!(
            text.exited(0).stdout.contains("verified"),
            "{}",
            text.stdout
        );

        let output = home.0.path().join("stock.bin");
        let output_arg = output.to_str().unwrap();
        home.run(&["stock", "export", "--output", output_arg], "")
            .await
            .exited(0);
        let exported = std::fs::read(&output).unwrap();
        assert_eq!(&exported[32..], &stock[..]);
        // An existing file is never replaced.
        home.run(&["stock", "export", "--output", output_arg], "")
            .await
            .exited(1);

        let restore = home.run(&["stock", "restore", "--version", "1"], "").await;
        assert!(
            restore.exited(1).stderr.contains("pinned image"),
            "{}",
            restore.stderr
        );
        assert_eq!(http.stock(kitchen).await.unwrap().restore, None);


#[test]
fn export_saves_the_archive_once_it_is_whole() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        adopt(&http, &sim, sim.address.to_string()).await;
        let home = Home::new();
        home.login(&server).await.exited(0);
        let file = home.0.path().join("home.tar");

        let saved = home
            .run(&["export", file.to_str().unwrap(), "--json"], "")
            .await;
        let bodies: Vec<serde_json::Value> = saved.exited(0).bodies();
        let archive = std::fs::read(&file).unwrap();
        assert_eq!(bodies[0]["bytes"], archive.len());
        assert!(!home.0.path().join("home.tar.partial").exists());
        // The archive the server streamed, whole: it imports.
        let fresh = TestServer::start().await;
        let imported = fresh.client().import(archive).await.unwrap();
        assert_eq!(imported.tables[0].rows, 1);

        let csv = home.0.path().join("home-csv.tar");
        let saved = home
            .run(&["export", csv.to_str().unwrap(), "--csv"], "")
            .await;
        assert!(saved.exited(0).stdout.starts_with("Saved "));
        let archive = std::fs::read(&csv).unwrap();
        assert!(archive.windows(9).any(|name| name == b"users.csv"));

        fresh.stop().await;
        server.stop().await;
        sim.stop().await;
    });
}
