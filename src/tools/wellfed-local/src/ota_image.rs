//! Serve one pinned experiment/original pair, or one reviewed replacement
//! build; never publishes a device command.
//!
//! `ota-images` runs this HTTP server on the fixed trial port for a single
//! peer and a 15-minute accept window. Every image is hash-bound and held in
//! memory once loaded, so changing the files afterwards cannot change what is
//! served: the stock pair by pinned digests, a replacement build by its frozen
//! review's `ota_sha256`. A private lease beside the config records the
//! random URL token and the served digest; trial dispatch requires that lease
//! and a live listener.
use std::{
    convert::Infallible, future::Future, io::Read, net::Ipv4Addr, path::Path, sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use wellfed_formats::{
    canonical,
    review::{ImageKind, ImageReview},
};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, task::JoinSet};
use uuid::Uuid;

use crate::{config::Config, firmware_ota, storage};

pub const ORIGINAL_SHA256: &str =
    "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498";
pub const TEST_SHA256: &str = "3cd8d0454f3eee84b6fc81fdf30999b1527e7ed2aad9e8ec3fadbd906da86503";
pub const C_PROBE_SHA256: &str = "9af0ce801c538f7ff4b6edbe84f4d250791c5dedefbf01f0b18958b7123df6e7";

const IMAGE_BYTES: usize = 1_376_288;

/// Trial URLs are built with this port; the server cannot be moved elsewhere.
pub const PORT: u16 = 8088;
pub const ACCEPT_WINDOW_SECONDS: i64 = 15 * 60;
const LEASE_FILE: &str = "ota-image-server.json";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    StockVersion,
    CExecution,
    /// A replacement-firmware build for stock to install (the stock-to-
    /// replacement POC). `--directory` is its frozen review. There is no
    /// original beside it: the way back to stock is the UART recovery.
    Replacement,
}

impl Profile {
    /// The pinned experiment's file name and digest; `None` for a
    /// replacement build, whose digest comes from its review.
    fn pinned(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::StockVersion => Some(("version-test-1.1.16.bin", TEST_SHA256)),
            Self::CExecution => Some(("c-probe-1.1.17.bin", C_PROBE_SHA256)),
            Self::Replacement => None,
        }
    }

    pub fn route(self) -> &'static str {
        match self {
            Self::StockVersion => "test.bin",
            Self::CExecution => "probe.bin",
            Self::Replacement => "replacement.bin",
        }
    }
}

/// Private record of the running image server, written beside the config.
/// Dispatch requires its token, an unexpired accept window and a live listener.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub token: Uuid,
    pub peer: Ipv4Addr,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub profile: Profile,
    /// The served experiment's SHA-256. Required for a replacement build,
    /// whose digest is not a constant; absent from older leases.
    #[serde(default)]
    pub sha256: Option<String>,
}

impl Lease {
    pub fn path(config_path: &Path) -> std::path::PathBuf {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(LEASE_FILE)
    }

    /// Return the lease only when its file parses and the listener accepts TCP.
    pub async fn current(config_path: &Path, host: std::net::IpAddr) -> Option<Self> {
        let data = tokio::fs::read(Self::path(config_path)).await.ok()?;
        let lease: Self = serde_json::from_slice(&data).ok()?;
        let connect = tokio::net::TcpStream::connect((host, PORT));
        matches!(
            tokio::time::timeout(Duration::from_secs(1), connect).await,
            Ok(Ok(_))
        )
        .then_some(lease)
    }
}

pub struct Images {
    /// `None` for a replacement build, which has no return image here.
    original: Option<Bytes>,
    experiment: Bytes,
    profile: Profile,
    /// The experiment's SHA-256, recorded in the lease.
    sha256: String,
}

/// Read at most `IMAGE_BYTES`; a pinned image must be exactly that long.
fn read_image(path: &Path, exact: bool) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).context("open prepared OTA image")?;
    let size = file.metadata()?.len();
    ensure!(
        if exact {
            size == IMAGE_BYTES as u64
        } else {
            size <= IMAGE_BYTES as u64
        },
        "unexpected OTA file size"
    );

    let mut data = Vec::with_capacity(IMAGE_BYTES);
    file.take((IMAGE_BYTES + 1) as u64).read_to_end(&mut data)?;
    ensure!(data.len() as u64 == size, "OTA size changed while reading");
    Ok(data)
}

impl Images {
    pub fn load(directory: &Path, profile: Profile) -> Result<Self> {
        fn pinned(directory: &Path, filename: &str, expected: &str) -> Result<Bytes> {
            let data = read_image(&directory.join(filename), true)?;
            ensure!(
                canonical::sha256(&data) == expected,
                "OTA image hash mismatch"
            );
            Ok(Bytes::from(data))
        }

        let Some((filename, digest)) = profile.pinned() else {
            return Self::replacement(directory);
        };
        Ok(Self {
            original: Some(pinned(directory, "original-1.1.15.bin", ORIGINAL_SHA256)?),
            experiment: pinned(directory, filename, digest)?,
            profile,
            sha256: digest.to_owned(),
        })
    }

    /// A frozen review's `image.bin`, bound to the review's `ota_sha256`.
    /// The review's layout audit is what establishes the container header,
    /// byte-sum and inner images stock checks; the digest binds these bytes
    /// to it. The loader-marker check repeats `ota-sign`'s.
    fn replacement(review: &Path) -> Result<Self> {
        let frozen: ImageReview =
            wellfed_formats::load(&review.join("review.json")).context("read review.json")?;
        ensure!(
            frozen.image_kind == ImageKind::Build,
            "review is not of a replacement build"
        );

        let data = read_image(&review.join("image.bin"), false)?;
        let sha256 = canonical::sha256(&data);
        ensure!(
            sha256 == frozen.ota_sha256,
            "image.bin does not match its review"
        );
        firmware_ota::payload(&data)?;

        Ok(Self {
            original: None,
            experiment: Bytes::from(data),
            profile: Profile::Replacement,
            sha256,
        })
    }
}

struct ImageState {
    images: Images,
    peer: Ipv4Addr,
    token: Uuid,
}

fn response(
    state: &ImageState,
    address: std::net::SocketAddr,
    headers: &HeaderMap,
    original: bool,
    head: bool,
) -> Response {
    if address.ip() != state.peer {
        return StatusCode::NOT_FOUND.into_response();
    }
    // The reviewed downloader requests a complete object. Do not silently serve
    // a partial representation or conditional 304 for an unverified client.
    if headers.contains_key(header::RANGE)
        || headers.contains_key(header::IF_RANGE)
        || headers.contains_key(header::IF_NONE_MATCH)
        || headers.contains_key(header::IF_MODIFIED_SINCE)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let (data, kind) = match (original, &state.images.original) {
        (true, Some(data)) => (data.clone(), "original"),
        (true, None) => return StatusCode::NOT_FOUND.into_response(),
        (false, _) => (
            state.images.experiment.clone(),
            state.images.profile.route(),
        ),
    };

    tracing::info!(
        image = kind,
        bytes = data.len(),
        "OTA image requested; receipt unconfirmed"
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, data.len())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONNECTION, "close")
        .body(if head {
            Body::empty()
        } else {
            Body::from(data)
        })
        .expect("fixed OTA response headers")
}

fn dispatch(
    state: &ImageState,
    peer: std::net::SocketAddr,
    request: &Request<Incoming>,
) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if request.uri().query().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let original = if request.uri().path()
        == format!("/ota/{}/{}", state.token, state.images.profile.route())
    {
        false
    } else if request.uri().path() == format!("/ota/{}/original.bin", state.token) {
        true
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if request.headers().contains_key(header::TRANSFER_ENCODING)
        || request
            .headers()
            .get(header::CONTENT_LENGTH)
            .is_some_and(|n| n != "0")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }

    response(
        state,
        peer,
        request.headers(),
        original,
        request.method() == Method::HEAD,
    )
}

async fn run(
    listener: TcpListener,
    images: Images,
    peer: Ipv4Addr,
    token: Uuid,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let state = Arc::new(ImageState {
        images,
        peer,
        token,
    });
    let mut connections = JoinSet::new();
    tokio::pin!(stop);

    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let (stream, address) = accepted?;
                if connections.len() >= 4 { continue; }
                let state = Arc::clone(&state);
                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        let state = Arc::clone(&state);
                        async move { Ok::<_, Infallible>(dispatch(&state, address, &request)) }
                    });

                    let mut http = http1::Builder::new();
                    http.title_case_headers(true)
                        .keep_alive(false)
                        .timer(TokioTimer::new())
                        .header_read_timeout(Duration::from_secs(10))
                        .max_headers(16)
                        .max_buf_size(8192);

                    let result = tokio::time::timeout(Duration::from_secs(300),
                        http.serve_connection(TokioIo::new(stream), service)).await;
                    if !matches!(result, Ok(Ok(()))) {
                        tracing::warn!("OTA HTTP connection failed or exceeded its deadline");
                    }
                });
            }
        }
    }

    // Stop admitting connections, but let an already-started transfer finish
    // within its own deadline instead of truncating it at the server expiry.
    drop(listener);
    while connections.join_next().await.is_some() {}
    Ok(())
}

pub async fn serve(
    config: Config,
    config_path: &Path,
    directory: &Path,
    peer: Ipv4Addr,
    profile: Profile,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let std::net::IpAddr::V4(host) = config.mqtt_listen.ip() else {
        anyhow::bail!("OTA image host must use IPv4");
    };
    ensure!(
        host.is_private() && peer.is_private() && host != peer,
        "OTA image server requires distinct private IPv4 host/peer addresses"
    );

    let images = Images::load(directory, profile)?;
    let listener = TcpListener::bind((host, PORT))
        .await
        .context("bind OTA image server")?;

    let lease = Lease {
        token: Uuid::new_v4(),
        peer,
        expires_at: Utc::now() + chrono::Duration::seconds(ACCEPT_WINDOW_SECONDS),
        profile,
        sha256: Some(images.sha256.clone()),
    };

    let lease_path = Lease::path(config_path);
    // The private lease (mode 0600) and operator output are the only places the
    // token appears. No directory listing, upload, generic file route, redirect,
    // firmware command or firewall change exists.
    storage::write_json(&lease_path, &lease)?;
    #[allow(clippy::print_stdout)]
    {
        println!(
            "Image server only; no update requested. Stops accepting at {} or on Ctrl-C or SIGTERM; active transfers drain.",
            lease.expires_at
        );
        println!("Server token: {}", lease.token);
        println!(
            "Experiment: http://{host}:{PORT}/ota/{}/{}",
            lease.token,
            profile.route()
        );
        if images.original.is_some() {
            println!(
                "Original: http://{host}:{PORT}/ota/{}/original.bin",
                lease.token
            );
        }
    }

    let result = run(listener, images, peer, lease.token, async {
        tokio::select! {
            _ = stop => {},
            _ = tokio::time::sleep(Duration::from_secs(ACCEPT_WINDOW_SECONDS as u64)) => {},
        }
    })
    .await;

    // Remove only our own lease; a newer server may have replaced it.
    if std::fs::read(&lease_path)
        .ok()
        .and_then(|data| serde_json::from_slice::<Lease>(&data).ok())
        .is_some_and(|current| current == lease)
    {
        let _ = std::fs::remove_file(&lease_path);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        task::JoinHandle,
    };

    async fn start(peer: Ipv4Addr) -> (SocketAddr, Uuid, JoinHandle<Result<()>>) {
        start_profile(peer, Profile::StockVersion).await
    }

    async fn start_profile(
        peer: Ipv4Addr,
        profile: Profile,
    ) -> (SocketAddr, Uuid, JoinHandle<Result<()>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let token = Uuid::new_v4();
        let images = Images {
            original: (profile != Profile::Replacement)
                .then(|| Bytes::from_static(b"original-image")),
            experiment: Bytes::from_static(b"test-image"),
            profile,
            sha256: String::new(),
        };

        let worker = tokio::spawn(run(listener, images, peer, token, std::future::pending()));
        (address, token, worker)
    }

    async fn exchange(address: SocketAddr, request: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut result = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut result))
            .await
            .unwrap()
            .unwrap();
        result
    }

    fn split(response: &[u8]) -> (&str, &[u8]) {
        let at = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        (
            std::str::from_utf8(&response[..at]).unwrap(),
            &response[at..],
        )
    }

    #[tokio::test]
    async fn c_profile_serves_only_probe_and_original_routes() {
        let (address, token, worker) =
            start_profile(Ipv4Addr::LOCALHOST, Profile::CExecution).await;

        for (file, status, body) in [
            ("probe.bin", 200, b"test-image".as_slice()),
            ("original.bin", 200, b"original-image".as_slice()),
            ("test.bin", 404, b"".as_slice()),
        ] {
            let reply = exchange(
                address,
                &format!("GET /ota/{token}/{file} HTTP/1.1\r\nHost: lab\r\n\r\n"),
            )
            .await;
            assert!(split(&reply).0.starts_with(&format!("HTTP/1.1 {status} ")));
            assert_eq!(split(&reply).1, body);
        }

        worker.abort();
    }

    #[tokio::test]
    async fn raw_http_uses_stock_compatible_header_case_and_exact_complete_objects() {
        let (address, token, worker) = start(Ipv4Addr::LOCALHOST).await;

        for (file, expected) in [
            ("test.bin", &b"test-image"[..]),
            ("original.bin", &b"original-image"[..]),
        ] {
            let reply = exchange(
                address,
                &format!("GET /ota/{token}/{file} HTTP/1.1\r\nHost: lab\r\n\r\n"),
            )
            .await;
            let (headers, body) = split(&reply);
            assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(headers.contains(&format!("\r\nContent-Length: {}\r\n", expected.len())));
            assert!(headers.contains("\r\nConnection: close\r\n"));
            assert!(
                !headers.contains("content-length:") && !headers.contains("Transfer-Encoding:")
            );
            assert!(!headers.contains("Content-Encoding:"));
            assert_eq!(body, expected);
        }

        let reply = exchange(
            address,
            &format!("HEAD /ota/{token}/test.bin HTTP/1.1\r\nHost: lab\r\n\r\n"),
        )
        .await;
        let (headers, body) = split(&reply);
        assert!(headers.contains("Content-Length: 10\r\n"));
        assert!(body.is_empty());

        worker.abort();
    }

    #[tokio::test]
    async fn wrong_peer_path_method_range_and_conditional_requests_cannot_fetch_image() {
        let (address, token, worker) = start(Ipv4Addr::LOCALHOST).await;

        for (method, path, extra, status) in [
            ("GET", "/private/server.json".to_owned(), "", 404),
            ("GET", format!("/ota/{}/test.bin", Uuid::nil()), "", 404),
            ("POST", format!("/ota/{token}/test.bin"), "", 405),
            (
                "GET",
                format!("/ota/{token}/test.bin"),
                "Range: bytes=0-9\r\n",
                400,
            ),
            (
                "GET",
                format!("/ota/{token}/test.bin"),
                "If-None-Match: x\r\n",
                400,
            ),
            ("GET", format!("/ota/{token}/test.bin?x=1"), "", 404),
        ] {
            let reply = exchange(
                address,
                &format!("{method} {path} HTTP/1.1\r\nHost: lab\r\n{extra}\r\n"),
            )
            .await;
            assert!(split(&reply).0.starts_with(&format!("HTTP/1.1 {status} ")));
            assert!(split(&reply).1.is_empty());
        }
        worker.abort();

        let (address, token, worker) = start(Ipv4Addr::new(127, 0, 0, 2)).await;
        let reply = exchange(
            address,
            &format!("GET /ota/{token}/test.bin HTTP/1.1\r\nHost: lab\r\n\r\n"),
        )
        .await;
        assert!(split(&reply).0.starts_with("HTTP/1.1 404 "));
        assert!(split(&reply).1.is_empty());
        worker.abort();
    }

    #[tokio::test]
    async fn lease_requires_a_parseable_file_and_a_live_listener_on_the_trial_port() {
        let host = std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 250));
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("server.json");

        assert!(Lease::current(&config, host).await.is_none());

        let lease = Lease {
            token: Uuid::new_v4(),
            peer: Ipv4Addr::new(10, 0, 0, 2),
            expires_at: Utc::now(),
            profile: Profile::StockVersion,
            sha256: None,
        };

        storage::write_json(&Lease::path(&config), &lease).unwrap();
        assert!(Lease::current(&config, host).await.is_none());

        let listener = TcpListener::bind((host, PORT)).await.unwrap();
        assert_eq!(Lease::current(&config, host).await, Some(lease));

        std::fs::write(Lease::path(&config), b"{\"token\":\"x\"}").unwrap();
        assert!(Lease::current(&config, host).await.is_none());

        drop(listener);
    }

    #[test]
    fn legacy_image_lease_defaults_to_the_version_experiment() {
        let lease: Lease = serde_json::from_value(serde_json::json!({
            "token":Uuid::new_v4(), "peer":"10.0.0.2", "expires_at":Utc::now()
        }))
        .unwrap();
        assert_eq!(lease.profile, Profile::StockVersion);
    }

    #[tokio::test]
    async fn replacement_profile_serves_only_its_build() {
        let (address, token, worker) =
            start_profile(Ipv4Addr::LOCALHOST, Profile::Replacement).await;

        for (file, status, body) in [
            ("replacement.bin", 200, b"test-image".as_slice()),
            ("original.bin", 404, b"".as_slice()),
            ("test.bin", 404, b"".as_slice()),
        ] {
            let reply = exchange(
                address,
                &format!("GET /ota/{token}/{file} HTTP/1.1\r\nHost: lab\r\n\r\n"),
            )
            .await;
            let (headers, received) = split(&reply);
            assert!(headers.starts_with(&format!("HTTP/1.1 {status} ")));
            assert_eq!(received, body);
        }

        worker.abort();
    }

    /// A review directory holding `image` and a review naming `digest`.
    fn review(image: &[u8], digest: &str, kind: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("image.bin"), image).unwrap();
        let review = serde_json::json!({
            "schema_version": 1,
            "kind": "plaf103_development_image_review",
            "image_kind": kind,
            "ota_sha256": digest,
            "payload_sha256": "",
            "payload_bytes": 0,
            "manifest_sha256": null,
            "review": {"stock_sha256": ""},
            "reviewer_sha256": "",
            "application_boot_verified": false,
        });
        storage::write_json(&directory.path().join("review.json"), &review).unwrap();
        directory
    }

    #[test]
    fn a_replacement_build_loads_only_when_its_review_binds_it() {
        let mut image = vec![0xff; 32];
        image.extend_from_slice(b"81958711");
        image.extend_from_slice(&[0x5a; 64]);
        let digest = canonical::sha256(&image);

        let good = review(&image, &digest, "build");
        let images = Images::load(good.path(), Profile::Replacement).unwrap();
        assert!(images.original.is_none());
        assert_eq!(images.experiment.as_ref(), image.as_slice());
        assert_eq!(images.sha256, digest);

        let other = canonical::sha256(b"other");
        assert!(
            Images::load(review(&image, &other, "build").path(), Profile::Replacement).is_err()
        );
        assert!(
            Images::load(
                review(&image, &digest, "stock").path(),
                Profile::Replacement
            )
            .is_err()
        );

        let mut unmarked = image.clone();
        unmarked[32] = 0;
        let unmarked_digest = canonical::sha256(&unmarked);
        assert!(
            Images::load(
                review(&unmarked, &unmarked_digest, "build").path(),
                Profile::Replacement
            )
            .is_err()
        );

        let mut oversized = image.clone();
        oversized.resize(IMAGE_BYTES + 1, 0xff);
        let oversized_digest = canonical::sha256(&oversized);
        assert!(
            Images::load(
                review(&oversized, &oversized_digest, "build").path(),
                Profile::Replacement
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires the private prepared stock-version experiment; run explicitly"]
    fn pinned_real_images_are_immutable_after_load_and_mutated_files_are_refused() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        for (profile, name) in [
            (Profile::StockVersion, "stock-version-1.1.16"),
            (Profile::CExecution, "rust-probe-1.1.17"),
        ] {
            let source = root.join("private/firmware/experiments").join(name);
            let directory = tempfile::tempdir().unwrap();
            let (filename, _) = profile.pinned().unwrap();
            for name in ["original-1.1.15.bin", filename] {
                std::fs::copy(source.join(name), directory.path().join(name)).unwrap();
            }

            let images = Images::load(directory.path(), profile).unwrap();
            assert_eq!(images.original.as_ref().map(Bytes::len), Some(IMAGE_BYTES));
            assert_eq!(images.experiment.len(), IMAGE_BYTES);

            let path = directory.path().join("original-1.1.15.bin");
            let mut data = std::fs::read(&path).unwrap();
            data[0x17040] ^= 1;
            std::fs::write(path, &data).unwrap();
            assert_ne!(
                images.original.as_ref().map(|o| o[0x17040]),
                Some(data[0x17040])
            );
            assert!(Images::load(directory.path(), profile).is_err());

            std::fs::copy(
                source.join("original-1.1.15.bin"),
                directory.path().join("original-1.1.15.bin"),
            )
            .unwrap();

            let path = directory.path().join(filename);
            let mut data = std::fs::read(&path).unwrap();
            data[0x17040] ^= 1;
            std::fs::write(path, &data).unwrap();
            assert!(Images::load(directory.path(), profile).is_err());
        }
    }
}
