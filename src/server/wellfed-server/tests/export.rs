//! Export and import over HTTP: an owner's archive of a server with a
//! feeder, accounts and owner data imports onto an empty server and
//! exports again byte for byte; it carries no secret; the CSV form has a
//! header and a record per row; and an import onto a server with an
//! account, or of a damaged archive, changes nothing.

use std::pin::pin;

use futures_util::TryStreamExt;
use jiff::SignedDuration;
use wellfed_api::{
    AdoptFeeder, ClientRole, CommandState, Connection, Credentials, ErrorCode, ExportFormat,
    FeederId, NewFood, NewPet, NewRefill, NewToken, NewUser, NewWeight, Role, SampleValue, Species,
};
use wellfed_formats::canonical::sha256;
use wellfed_formats::export::{Manifest, Table};
use wellfed_server_client::{Auth, Client, StatusCode};
use wellfed_server_testkit::feeder::{self, Running};
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{TestServer, ada_login, ada_setup, local, refused, until};
use wellfed_service::{SystemRandom, reset_owner};
use wellfed_store::NewSample;
use uuid::Uuid;

/// A server with rows in most tables: two accounts and a token, a feeder
/// that fed once and was locked, a pet with a weight on that feeder, and a
/// food with a refill. The owner's client comes with it.
async fn household(sim: &Running) -> (TestServer, Client) {
    let server = TestServer::start().await;
    let mut owner = server.client();
    owner.setup(&ada_setup()).await.unwrap();
    owner
        .create_user(&NewUser {
            name: "bob".into(),
            password: "battery staple".into(),
            role: Role::Member,
            expires: None,
        })
        .await
        .unwrap();
    owner
        .create_token(&NewToken {
            name: "Ada's phone".into(),
            expires: None,
        })
        .await
        .unwrap();

    let feeder = adopt(&owner, sim).await;
    let key = Uuid::from_u128(0x0192c5d0_4444_7000_8000_000000000001);
    owner.feed(feeder, 1, key).await.unwrap();
    until("the feed settles", || async {
        owner.command(key).await.unwrap().state == CommandState::Resolved
    })
    .await;
    let lock = Uuid::from_u128(0x0192c5d0_4444_7000_8000_000000000002);
    owner
        .set_lock(feeder, wellfed_server_client::Lock::Locked, lock)
        .await
        .unwrap();

    let pet = owner
        .create_pet(&NewPet {
            name: "Tom".into(),
            species: Species::Cat,
            birth_date: None,
            daily_kcal: Some(250),
        })
        .await
        .unwrap();
    owner
        .add_weight(
            pet.id,
            &NewWeight {
                grams: 4200,
                at: None,
            },
        )
        .await
        .unwrap();
    owner.assign_feeder(pet.id, feeder).await.unwrap();
    let food = owner
        .create_food(&NewFood {
            brand: "Acme, \"Fine\" Foods".into(),
            name: "Dry".into(),
            kcal_per_kg: 3600,
            bag_grams: 2000,
            grams_per_cup: 110,
        })
        .await
        .unwrap();
    owner.set_feeder_food(feeder, food.id).await.unwrap();
    owner.add_refill(feeder, &NewRefill::FullBag).await.unwrap();
    (server, owner)
}

async fn adopt(owner: &Client, sim: &Running) -> FeederId {
    let id = owner
        .adopt(&AdoptFeeder {
            invite: feeder::invite(sim, ClientRole::Server).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        })
        .await
        .unwrap()
        .id;
    until("the server connects to the feeder", || async {
        owner.feeders().await.unwrap()[0].connection == Connection::Online
    })
    .await;
    id
}

async fn export(client: &Client, format: ExportFormat) -> Vec<u8> {
    let mut chunks = pin!(client.export(format).await.unwrap());
    let mut archive = Vec::new();
    while let Some(chunk) = chunks.try_next().await.unwrap() {
        archive.extend_from_slice(&chunk);
    }
    archive
}

/// Each file in a tar the server wrote, by name.
fn entries(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut at = 0;
    while at + 512 <= archive.len() {
        let block = &archive[at..at + 512];
        if block.iter().all(|&byte| byte == 0) {
            break;
        }
        let name = block[..100].split(|&byte| byte == 0).next().unwrap();
        let size = std::str::from_utf8(&block[124..135]).unwrap();
        let size = usize::from_str_radix(size, 8).unwrap();
        at += 512;
        files.push((
            String::from_utf8(name.to_vec()).unwrap(),
            archive[at..at + size].to_vec(),
        ));
        at += size.div_ceil(512) * 512;
    }
    files
}

fn manifest(archive: &[u8]) -> Manifest {
    let (name, bytes) = entries(archive).swap_remove(0);
    assert_eq!(name, "manifest.json");
    serde_json::from_slice(&bytes).unwrap()
}

fn rows(manifest: &Manifest, table: Table) -> u64 {
    let file = manifest.tables.iter().find(|file| file.table == table);
    file.unwrap().rows
}

#[test]
fn an_archive_imports_onto_an_empty_server_and_exports_again_byte_for_byte() {
    local(async {
        let sim = feeder::start().await;
        let (first, owner) = household(&sim).await;

        let archive = export(&owner, ExportFormat::Json).await;
        let written = manifest(&archive);
        for table in [
            Table::Users,
            Table::Feeders,
            Table::Events,
            Table::Feeds,
            Table::LockChanges,
            Table::Commands,
            Table::Audit,
            Table::Pets,
            Table::PetWeights,
            Table::PetFeeders,
            Table::Foods,
            Table::FeederFoods,
            Table::Refills,
        ] {
            assert!(rows(&written, table) > 0, "{table:?} has rows");
        }
        assert_eq!(rows(&written, Table::Users), 2);
        // Each file is what the manifest says it is.
        for (name, bytes) in entries(&archive).into_iter().skip(1) {
            let file = written
                .tables
                .iter()
                .find(|file| format!("{}.jsonl", file.table.name()) == name)
                .unwrap();
            assert_eq!(
                (bytes.len() as u64, sha256(&bytes)),
                (file.bytes, file.sha256.clone())
            );
        }

        let second = TestServer::start().await;
        let imported = second.client().import(archive.clone()).await.unwrap();
        assert_eq!(imported.tables.len(), Table::ALL.len());
        assert_eq!(imported.tables[0].table, "users");
        assert_eq!(imported.tables[0].rows, 2);

        // Exported again before anyone logs in, since a login (and the
        // owner's reset) is itself an audit entry: the same bytes.
        let chunks = second.service().export(ExportFormat::Json, &written.server);
        let mut chunks = pin!(chunks.await.unwrap());
        let mut again = Vec::new();
        while let Some(chunk) = chunks.try_next().await.unwrap() {
            again.extend_from_slice(&chunk);
        }
        assert!(again == archive, "the round trip changed the archive");

        // No password came with the archive: neither the old one nor an
        // empty one (the stored hash is empty) opens it, and the host gives
        // the owner a temporary one.
        let mut ada = second.client();
        let empty = Credentials {
            name: "ada".into(),
            password: String::new(),
        };
        for credentials in [ada_login(), empty] {
            refused(
                ada.login(&credentials).await,
                StatusCode::UNAUTHORIZED,
                ErrorCode::BadCredentials,
            );
        }
        let store = second.store().open().await;
        let reset = reset_owner(&store, Some("ada"), &SystemRandom, FixedClock.now());
        let (_, password) = reset.await.unwrap();
        ada.login(&Credentials {
            name: "ada".into(),
            password,
        })
        .await
        .unwrap();

        // The feeder is the server's again only once it is adopted again.
        let feeders = ada.feeders().await.unwrap();
        assert_eq!(feeders.len(), 1);
        assert_eq!(feeders[0].connection, Connection::Offline);
        assert!(feeders[0].archived.is_some(), "{feeders:?}");

        store.close().await;
        second.stop().await;
        first.stop().await;
    });
}

#[test]
fn the_archive_carries_no_secret() {
    local(async {
        let sim = feeder::start().await;
        let (server, owner) = household(&sim).await;
        let token = owner
            .create_token(&NewToken {
                name: "the CLI".into(),
                expires: None,
            })
            .await
            .unwrap();
        let Some(Auth::Session(cookie)) = owner.auth().cloned() else {
            panic!("the owner logged in with a session");
        };
        let store = server.store().open().await;
        let sealed = store.feeders().await.unwrap()[0].key.sealed.clone();

        let archive = export(&owner, ExportFormat::Json).await;

        let text = String::from_utf8_lossy(&archive);
        let hex = |bytes: &[u8]| wellfed_formats::hex::encode(bytes);
        let secrets = [
            ("a password hash", "$argon2".to_owned()),
            ("a token", token.secret.clone()),
            ("a token's digits", token.secret["plt_".len()..].to_owned()),
            (
                "a token's hash",
                sha256(&wellfed_formats::hex::decode_vec(&token.secret["plt_".len()..]).unwrap()),
            ),
            ("a session", cookie.clone()),
            (
                "a session's hash",
                sha256(&wellfed_formats::hex::decode_vec(&cookie).unwrap()),
            ),
            ("the sealed feeder key", hex(&sealed)),
        ];
        for (what, secret) in secrets {
            assert!(!text.contains(&secret), "the archive holds {what}");
        }
        let names: Vec<String> = entries(&archive)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for kept_out in ["sessions", "tokens", "job_runs"] {
            assert!(
                !names.iter().any(|name| name.starts_with(kept_out)),
                "{names:?}"
            );
        }

        store.close().await;
        server.stop().await;
    });
}

#[test]
fn a_csv_archive_has_a_header_and_a_record_per_row() {
    local(async {
        let sim = feeder::start().await;
        let (server, owner) = household(&sim).await;

        let archive = export(&owner, ExportFormat::Csv).await;

        let files = entries(&archive);
        assert_eq!(files.len(), 1 + Table::ALL.len());
        let foods = &files
            .iter()
            .find(|(name, _)| name == "foods.csv")
            .unwrap()
            .1;
        let foods = String::from_utf8(foods.clone()).unwrap();
        let lines: Vec<&str> = foods.split_terminator("\r\n").collect();
        assert_eq!(
            lines[0],
            "id,brand,name,kcal_per_kg,bag_grams,grams_per_cup,created_ms"
        );
        assert_eq!(lines.len(), 2);
        assert!(
            lines[1].contains(",\"Acme, \"\"Fine\"\" Foods\",Dry,3600,2000,110,"),
            "{}",
            lines[1]
        );
        let users = &files
            .iter()
            .find(|(name, _)| name == "users.csv")
            .unwrap()
            .1;
        assert!(
            String::from_utf8_lossy(users).starts_with("id,name,role,created_ms,expires_ms\r\n")
        );

        // Only the JSON archive imports.
        let fresh = TestServer::start().await;
        refused(
            fresh.client().import(archive).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ArchiveInvalid,
        );
        assert!(fresh.client().setup_state().await.unwrap().open);

        fresh.stop().await;
        server.stop().await;
    });
}

#[test]
fn an_import_needs_a_server_with_no_account_and_a_whole_archive() {
    local(async {
        let sim = feeder::start().await;
        let (server, owner) = household(&sim).await;
        let archive = export(&owner, ExportFormat::Json).await;

        // Not onto a server with an account: not anonymously, and not by
        // its owner either.
        refused(
            server.client().import(archive.clone()).await,
            StatusCode::CONFLICT,
            ErrorCode::ImportClosed,
        );
        refused(
            owner.import(archive.clone()).await,
            StatusCode::CONFLICT,
            ErrorCode::ImportClosed,
        );

        // A changed byte, a cut archive, one longer than its manifest says
        // or no archive imports nothing.
        let fresh = TestServer::start().await;
        let mut changed = archive.clone();
        let at = changed
            .windows(5)
            .position(|window| window == b"\"bob\"")
            .unwrap();
        changed[at + 1] = b'r';
        let cut = archive[..archive.len() / 2].to_vec();
        let mut long = archive.clone();
        long.extend_from_slice(&[0; 512]);
        let uploads = [
            ("changed", changed),
            ("cut", cut),
            ("long", long),
            ("empty", Vec::new()),
        ];
        for (what, upload) in uploads {
            let result = fresh.client().import(upload).await;
            refused(
                result,
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ArchiveInvalid,
            );
            assert!(fresh.client().setup_state().await.unwrap().open, "{what}");
        }

        // The whole archive still imports there, and closes setup.
        fresh.client().import(archive).await.unwrap();
        assert!(!fresh.client().setup_state().await.unwrap().open);
        refused(
            fresh.client().setup(&ada_setup()).await,
            StatusCode::CONFLICT,
            ErrorCode::SetupClosed,
        );

        fresh.stop().await;
        server.stop().await;
    });
}

/// Setup and an import racing on a fresh server: exactly one wins, and the
/// other is refused as closed.
#[test]
fn setup_and_an_import_racing_end_with_one_household() {
    local(async {
        let sim = feeder::start().await;
        let (server, owner) = household(&sim).await;
        let archive = export(&owner, ExportFormat::Json).await;

        for _ in 0..4 {
            let fresh = TestServer::start().await;
            let mut setting_up = fresh.client();
            let importing = fresh.client();
            let first_owner = ada_setup();
            let (setup, import) = tokio::join!(
                setting_up.setup(&first_owner),
                importing.import(archive.clone())
            );

            match (setup, import) {
                (Ok(_), import) => {
                    refused(import, StatusCode::CONFLICT, ErrorCode::ImportClosed);
                    assert_eq!(setting_up.users().await.unwrap().len(), 1);
                }
                (setup, Ok(_)) => {
                    refused(setup, StatusCode::CONFLICT, ErrorCode::SetupClosed);
                    let store = fresh.store().open().await;
                    assert_eq!(store.users().await.unwrap().len(), 2);
                    store.close().await;
                }
                (setup, import) => panic!("neither won: {setup:?} {import:?}"),
            }
            fresh.stop().await;
        }

        server.stop().await;
    });
}

/// An export nobody reads: a second is refused while it runs, and the
/// server stops without waiting for the reader, whose export ends in an
/// error.
#[test]
fn one_export_runs_at_a_time_and_none_holds_up_a_stop() {
    local(async {
        let sim = feeder::start().await;
        let (server, owner) = household(&sim).await;
        // Far more than the export's queue holds, so its writer waits on
        // the reader.
        let feeder = owner.feeders().await.unwrap()[0].id;
        let store = server.store().open().await;
        let samples: Vec<NewSample> = (0..20_000)
            .map(|n| NewSample {
                received: FixedClock.now() + SignedDuration::from_secs(n),
                uptime_ms: 0,
                value: SampleValue::Rssi { dbm: -60 },
                raw: vec![0x77],
            })
            .collect();
        store.record_samples(feeder, &samples).await.unwrap();
        store.close().await;

        let mut held = Box::pin(owner.export(ExportFormat::Csv).await.unwrap());
        held.try_next().await.unwrap().unwrap();
        refused(
            owner.export(ExportFormat::Json).await.map(drop),
            StatusCode::CONFLICT,
            ErrorCode::ExportRunning,
        );

        // The server stops meanwhile: the reader sees the export end in
        // an error, never in a clean end it could take for a whole one.
        let stopped = tokio::task::spawn_local(server.stop());
        let ended = loop {
            match held.try_next().await {
                Ok(Some(_)) => {}
                Ok(None) => break None,
                Err(error) => break Some(error),
            }
        };
        assert!(ended.is_some(), "the cut export ended cleanly");
        tokio::time::timeout(std::time::Duration::from_secs(10), stopped)
            .await
            .expect("the stop does not wait for the export's reader")
            .unwrap();
    });
}
