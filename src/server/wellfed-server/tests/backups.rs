//! Backups: an owner takes and lists them, the oldest past the count go,
//! the daily job takes one, and a backup restores with the key file kept
//! apart from it.

use std::num::NonZeroU32;
use std::os::unix::fs::PermissionsExt;

use jiff::SignedDuration;

use wellfed_api::{
    BackupPage, Connection, Credentials, ErrorCode, Job, JobOutcome, NewUser, Role,
};
use wellfed_server::key::{KEY_DIR, KEY_FILE, KeyError, sealing_key};
use wellfed_server_client::StatusCode;
use wellfed_server_testkit::fixed::{FixedClock, SeededRandom};
use wellfed_server_testkit::{
    TempStore, TestServer, TestService, ada_setup, config, feeder, local, refused,
};

#[test]
fn an_owner_takes_backups_and_the_oldest_past_the_count_go() {
    local(async {
        let mut config = config();
        config.backups.keep = NonZeroU32::new(2).unwrap();
        let server = TestServer::start_with(config).await;
        let mut owner = server.client();
        owner.setup(&ada_setup()).await.unwrap();
        assert_eq!(owner.backups().await.unwrap(), BackupPage::default());
        // A partial copy a crash left is not a backup, and the next copy
        // clears it.
        let dir = server.store().backups();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join(".server-20260101T000000Z.db.partial"), b"half").unwrap();
        assert_eq!(owner.backups().await.unwrap(), BackupPage::default());

        // The test's clock stands still, so all three are taken in one
        // second: each after the first is numbered.
        let taken: Vec<_> = [
            owner.backup().await.unwrap(),
            owner.backup().await.unwrap(),
            owner.backup().await.unwrap(),
        ]
        .into_iter()
        .map(|backup| backup.name)
        .collect();
        assert_eq!(taken[0], "server-20260921T141320Z.db");
        assert_eq!(taken[2], "server-20260921T141320Z-3.db");
        let kept: Vec<_> = owner
            .backups()
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|backup| backup.name)
            .collect();
        assert_eq!(kept, [taken[2].clone(), taken[1].clone()], "newest first");

        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(files, ["README.txt", &taken[1], &taken[2]]);
        let readme = std::fs::read_to_string(dir.join("README.txt")).unwrap();
        assert!(readme.contains("keys/server.key"), "{readme}");
        let mode = |path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.clone()), 0o700);
        assert_eq!(mode(dir.join(&taken[2])), 0o600);

        // Backups are the owner's: a member neither takes nor lists them.
        owner
            .create_user(&NewUser {
                name: "bob".into(),
                password: "battery staple".into(),
                role: Role::Member,
                expires: None,
            })
            .await
            .unwrap();
        let mut member = server.client();
        member
            .login(&Credentials {
                name: "bob".into(),
                password: "battery staple".into(),
            })
            .await
            .unwrap();
        refused(
            member.backup().await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        refused(
            member.backups().await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );

        server.stop().await;
    });
}

#[test]
fn the_daily_job_takes_a_backup() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        assert!(service.job_scheduled(Job::Backup));

        let run = service.run_job(Job::Backup).await;

        assert_eq!(run.outcome, JobOutcome::Succeeded);
        let page = service.backups().await.unwrap();
        assert_eq!((page.items.len(), page.next), (1, None));
        server.shutdown().await;
    });
}

#[test]
fn the_copy_just_taken_stays_when_the_clock_stepped_back() {
    local(async {
        let mut config = config();
        config.backups.keep = NonZeroU32::new(2).unwrap();
        let server = TestService::start_with(config).await;
        let service = server.service();
        let at = |hours| {
            FixedClock
                .now()
                .checked_add(SignedDuration::from_hours(hours))
                .unwrap()
        };

        for hours in [2, 3, 1] {
            service.run_backup(at(hours)).await.unwrap();
        }

        // The copy named an hour back sorts oldest, yet it is the newest
        // taken: it stays, and the newest by name keeps the other place.
        let taken: Vec<_> = service
            .backups()
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|backup| backup.taken)
            .collect();
        assert_eq!(taken, [at(3), at(1)]);
        server.shutdown().await;
    });
}

#[test]
fn a_backup_restores_with_the_key_kept_apart() {
    local(async {
        let sim = feeder::start().await;
        let data = TempStore::new();
        let (key, _) = sealing_key(data.dir(), &SeededRandom::new(1)).unwrap();
        let mut keyed = config();
        keyed.key = key;
        let server = TestService::open(data, keyed).await;
        let id = server.adopt(&sim).await;
        server.link(id, Connection::Online).await;
        let backup = server.service().backup().await.unwrap();
        let data = server.stop().await;

        // A fresh data directory with the backup as its database: refused
        // until the key file is back beside it.
        let restored = TempStore::new();
        std::fs::copy(data.backups().join(&backup.name), restored.path()).unwrap();
        let missing = sealing_key(restored.dir(), &SeededRandom::new(2)).unwrap_err();
        assert!(matches!(missing, KeyError::Missing { .. }), "{missing:?}");
        let keys = restored.dir().join(KEY_DIR);
        std::fs::create_dir(&keys).unwrap();
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::copy(data.dir().join(KEY_DIR).join(KEY_FILE), keys.join(KEY_FILE)).unwrap();
        let (again, _) = sealing_key(restored.dir(), &SeededRandom::new(2)).unwrap();
        assert_eq!(again, key);

        // The restored server opens its feeder key and reaches the feeder.
        let mut keyed = config();
        keyed.key = again;
        let server = TestService::open(restored, keyed).await;
        server.link(id, Connection::Online).await;
        server.shutdown().await;
        sim.stop().await;
    });
}
