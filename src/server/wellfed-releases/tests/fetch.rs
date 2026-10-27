//! A check of the project's releases against a local stand-in for
//! GitHub's API (no network): fetched files verified as uploads are.

mod support;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use wellfed_api::{ReleaseRefusal, ReleaseSource};
use wellfed_releases::{Error, Source};
use support::{now, open, project, release, versions as ids};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn a_check_with_fetching_off_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, None).await;

    let error = releases.check(now()).await.unwrap_err();

    assert!(matches!(error, Error::FetchDisabled), "{error:?}");
    assert!(!releases.fetching());
}

/// A stand-in for GitHub: fixed answers by path, and every path asked.
struct Stub {
    address: SocketAddr,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    async fn start(answers: impl FnOnce(&str) -> HashMap<String, (u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let answers = Arc::new(answers(&format!("http://{address}")));
        let asked = Arc::new(Mutex::new(Vec::new()));

        let log = Arc::clone(&asked);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let answers = Arc::clone(&answers);
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let read = stream.read(&mut buffer).await.unwrap();
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    let line = String::from_utf8_lossy(&request);
                    let target = line.split(' ').nth(1).unwrap_or_default();
                    let path = target.split('?').next().unwrap_or_default().to_owned();
                    log.lock().unwrap().push(path.clone());

                    let (status, body) = answers
                        .get(&path)
                        .cloned()
                        .unwrap_or((404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(&body).await.unwrap();
                    stream.shutdown().await.unwrap();
                });
            }
        });
        Self { address, asked }
    }

    fn source(&self) -> Source {
        Source {
            api: format!("http://{}", self.address),
            repository: "owner/wellfed".into(),
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

const LIST: &str = "/repos/owner/wellfed/releases";

fn asset(base: &str, id: u64, name: &str, size: usize) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "size": size,
        "browser_download_url": format!("{base}/download/{name}"),
    })
}

#[tokio::test]
async fn a_check_keeps_the_verified_assets_and_reports_the_rest() {
    let good = release(&project(), 12);
    let mut tampered = release(&project(), 11);
    *tampered.last_mut().unwrap() ^= 0x01;
    let draft = release(&project(), 13);

    let (good_len, tampered_len, draft_len) = (good.len(), tampered.len(), draft.len());
    let stub = Stub::start(|base| {
        let list = serde_json::json!([
            {
                "tag_name": "v0.13.0",
                "draft": true,
                "prerelease": false,
                "assets": [asset(base, 3, "wellfed-13.plota", draft_len)],
            },
            {
                "tag_name": "v0.12.0",
                "draft": false,
                "prerelease": false,
                "assets": [
                    asset(base, 1, "wellfed-12.plota", good_len),
                    asset(base, 4, "wellfed-server-x86_64", 10),
                ],
            },
            {
                "tag_name": "v0.11.0",
                "assets": [asset(base, 2, "wellfed-11.plota", tampered_len)],
            },
        ]);
        HashMap::from([
            (LIST.to_owned(), (200, serde_json::to_vec(&list).unwrap())),
            ("/download/wellfed-12.plota".to_owned(), (200, good)),
            ("/download/wellfed-11.plota".to_owned(), (200, tampered)),
            ("/download/wellfed-13.plota".to_owned(), (200, draft)),
        ])
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, Some(stub.source())).await;

    let check = releases.check(now()).await.unwrap();

    let added: Vec<_> = check
        .added
        .iter()
        .map(|r| (r.version, r.source.clone()))
        .collect();
    assert_eq!(
        added,
        [(
            12,
            ReleaseSource::Github {
                tag: "v0.12.0".into(),
                asset: "wellfed-12.plota".into(),
            }
        )]
    );
    assert_eq!(check.refused.len(), 1);
    assert_eq!(check.refused[0].tag, "v0.11.0");
    assert_eq!(check.refused[0].reason, ReleaseRefusal::Image);
    assert_eq!(ids(&releases).await, [12]);
    assert_eq!(
        stub.asked(),
        [
            LIST,
            "/download/wellfed-12.plota",
            "/download/wellfed-11.plota"
        ]
    );

    // The next check fetches only the list: the kept asset is in the
    // cache, the refused one remembered.
    let again = releases.check(now()).await.unwrap();
    assert!(again.added.is_empty() && again.refused.is_empty());
    assert_eq!(stub.asked().len(), 4);

    // A new process remembers what it kept, not what it refused.
    drop(releases);
    let reopened = open(&dir, Some(stub.source())).await;
    let third = reopened.check(now()).await.unwrap();
    assert_eq!(third.refused.len(), 1);
    assert_eq!(stub.asked()[4..], [LIST, "/download/wellfed-11.plota"]);
}

#[tokio::test]
async fn an_asset_larger_than_any_release_is_not_downloaded() {
    let stub = Stub::start(|base| {
        let list = serde_json::json!([{
            "tag_name": "v1",
            "assets": [asset(base, 9, "huge.plota", 64 << 20)],
        }]);
        HashMap::from([(LIST.to_owned(), (200, serde_json::to_vec(&list).unwrap()))])
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, Some(stub.source())).await;

    let check = releases.check(now()).await.unwrap();

    assert_eq!(check.refused[0].reason, ReleaseRefusal::Size);
    assert_eq!(stub.asked(), [LIST]);
}

#[tokio::test]
async fn a_failing_github_ends_the_check() {
    let stub = Stub::start(|_| HashMap::from([(LIST.to_owned(), (503, b"down".to_vec()))])).await;
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, Some(stub.source())).await;

    let error = releases.check(now()).await.unwrap_err();

    assert!(matches!(error, Error::Unreachable(_)), "{error:?}");

    let garbled = Stub::start(|_| HashMap::from([(LIST.to_owned(), (200, b"{".to_vec()))])).await;
    let releases = open(&dir, Some(garbled.source())).await;
    let error = releases.check(now()).await.unwrap_err();
    assert!(matches!(error, Error::Listing(_)), "{error:?}");
}

#[tokio::test]
async fn an_asset_that_fails_to_download_is_tried_again() {
    let good = release(&project(), 12);
    let good_len = good.len();
    let stub = Stub::start(|base| {
        let list = serde_json::json!([{
            "tag_name": "v0.12.0",
            "assets": [
                asset(base, 1, "gone.plota", good_len),
                asset(base, 2, "wellfed-12.plota", good_len),
            ],
        }]);
        HashMap::from([
            (LIST.to_owned(), (200, serde_json::to_vec(&list).unwrap())),
            ("/download/wellfed-12.plota".to_owned(), (200, good)),
        ])
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, Some(stub.source())).await;

    let check = releases.check(now()).await.unwrap();

    assert_eq!(check.added.len(), 1, "one failed download ends nothing");
    let failed: Vec<&str> = check.failed.iter().map(|a| a.asset.as_str()).collect();
    assert_eq!(failed, ["gone.plota"]);
    assert!(check.refused.is_empty());

    let again = releases.check(now()).await.unwrap();
    assert_eq!(again.failed.len(), 1);
    assert_eq!(
        stub.asked()[3..],
        [LIST, "/download/gone.plota"],
        "only the failed asset is fetched again"
    );
}
