//! A trial-enabled `serve` holds the bench lease beside its config for its
//! whole lifetime, since it dispatches installs; a plain `serve` takes none.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use wellfed_formats::bench::lease::Holder;

/// A config in a fresh directory, standing as `private/`.
fn configured() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    wellfed_stock::config::Config,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = common::address();
    config.http_listen = common::address();
    wellfed_stock::storage::write_json(&path, &config).unwrap();
    (dir, path, config)
}

fn lease(directory: &Path) -> Option<Holder> {
    let data = std::fs::read(directory.join("bench.lease")).ok()?;
    Some(serde_json::from_slice(&data).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trial_server_holds_the_bench_until_it_stops() {
    let (dir, path, config) = configured();

    let service = common::start_mode(&path, true);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    common::ready(&config, &http).await;

    let holder = lease(dir.path()).expect("the trial server holds the lease");
    assert_eq!(holder.command.get(1).map(String::as_str), Some("serve"));

    // Stopped by its interrupt, it releases the bench.
    drop(service);
    assert!(lease(dir.path()).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_server_takes_no_lease() {
    let (dir, path, config) = configured();

    let _service = common::start(&path);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    common::ready(&config, &http).await;

    assert!(lease(dir.path()).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_symlinked_config_keeps_the_lease_where_it_is_named() {
    // A worktree's private/ linking the main checkout's config.
    let (main, target, config) = configured();
    let worktree = tempfile::tempdir().unwrap();
    let link = worktree.path().join("server.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let _service = common::start_mode(&link, true);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    common::ready(&config, &http).await;

    assert!(lease(worktree.path()).is_some());
    assert!(lease(main.path()).is_none());
}

/// Another process holding the bench, killed when the test ends, passed
/// or not.
struct Other(std::process::Child);

impl Drop for Other {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_trial_server_refuses_a_held_bench() {
    let (dir, path, _) = configured();
    let other = Other(Command::new("sleep").arg("60").spawn().unwrap());
    let holder = Holder::of(other.0.id(), vec!["wellfed-bench".into(), "on".into()]).unwrap();
    wellfed_formats::private::write_json(&dir.path().join("bench.lease"), &holder).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wellfed-stock"))
        .args(["serve", "--enable-ota-trial", "--config"])
        .arg(&path)
        .env(wellfed_stock::build_info::ALLOW_UNREVIEWED_ENV, "1")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("The bench is held by pid"), "{stderr}");
}
