//! Serve one reviewed replacement build for stock to install; never
//! publishes a device command.
//!
//! `ota-images` runs this HTTP server on the fixed trial port for a single
//! peer and a 15-minute accept window. The build is bound to its frozen
//! review's `ota_sha256` and held in memory once loaded, so changing the files
//! afterwards cannot change what is served. A private lease beside the config
//! records the random URL token, the served digest and the build's identity;
//! trial dispatch requires that lease and a live listener.
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
use chrono::Utc;
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use wellfed_formats::{
    canonical,
    manifest::BuildManifest,
    review::{ImageKind, ImageReview},
};
use tokio::{net::TcpListener, task::JoinSet};
use uuid::Uuid;

use wellfed_core::ota::MARKER;

use crate::{config::Config, storage};

/// Stock's own image size, the most it accepts.
const IMAGE_BYTES: usize = 1_376_288;

/// Trial URLs are built with this port; the server cannot be moved elsewhere.
pub const PORT: u16 = 8088;
pub const ACCEPT_WINDOW_SECONDS: i64 = 15 * 60;

/// The one file the server holds, under its token.
pub const ROUTE: &str = "replacement.bin";

/// The OTA container's header precedes the bank contents.
pub const CONTAINER_HEADER: usize = 32;

/// The bank contents of an OTA container: everything after its header,
/// which must start with the loader's marker.
pub fn payload(container: &[u8]) -> Result<&[u8]> {
    ensure!(
        container.len() > CONTAINER_HEADER + MARKER.len(),
        "image too short"
    );
    let payload = &container[CONTAINER_HEADER..];
    ensure!(
        payload[..MARKER.len()] == MARKER,
        "image payload lacks the loader marker"
    );
    Ok(payload)
}

/// Private record of the running image server, written beside the config.
pub use wellfed_formats::stock::ImageLease as Lease;

/// The lease, only when its file parses and the listener accepts TCP.
pub async fn current(config_path: &Path, host: std::net::IpAddr) -> Option<Lease> {
    let data = tokio::fs::read(Lease::path(config_path)).await.ok()?;
    let lease: Lease = serde_json::from_slice(&data).ok()?;
    let connect = tokio::net::TcpStream::connect((host, PORT));
    matches!(
        tokio::time::timeout(Duration::from_secs(1), connect).await,
        Ok(Ok(_))
    )
    .then_some(lease)
}

/// A replacement build, loaded once from its frozen review.
pub struct Build {
    image: Bytes,
    /// Recorded in the lease.
    sha256: String,
    /// Recorded in the lease.
    build: String,
}

/// The startup identity the review's frozen `build-manifest.json` records,
/// bound to the review by its digest.
fn build_identity(review: &Path, frozen: &ImageReview) -> Result<String> {
    let raw = std::fs::read(review.join("build-manifest.json"))
        .context("read the review's build-manifest.json")?;
    ensure!(
        frozen.manifest_sha256.as_deref() == Some(canonical::sha256(&raw).as_str()),
        "build-manifest.json does not match its review"
    );

    let manifest: BuildManifest =
        serde_json::from_slice(&raw).context("parse build-manifest.json")?;
    let id = manifest.startup_identity.unwrap_or_default();
    ensure!(
        id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "build-manifest.json has no 64-hex startup_identity"
    );
    Ok(id.to_ascii_lowercase())
}

/// Read at most `IMAGE_BYTES`.
fn read_image(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).context("open the review's image.bin")?;
    let size = file.metadata()?.len();
    ensure!(size <= IMAGE_BYTES as u64, "unexpected OTA file size");

    let mut data = Vec::with_capacity(IMAGE_BYTES);
    file.take((IMAGE_BYTES + 1) as u64).read_to_end(&mut data)?;
    ensure!(data.len() as u64 == size, "OTA size changed while reading");
    Ok(data)
}

impl Build {
    /// A frozen review's `image.bin`, bound to the review's `ota_sha256`.
    /// The review's layout audit is what establishes the container header,
    /// byte-sum and inner images stock checks; the digest binds these bytes
    /// to it. The loader-marker check repeats `wellfed-build sign`'s.
    pub fn load(review: &Path) -> Result<Self> {
        let frozen: ImageReview =
            wellfed_formats::load(&review.join("review.json")).context("read review.json")?;
        ensure!(
            frozen.image_kind == ImageKind::Build,
            "review is not of a replacement build"
        );

        let data = read_image(&review.join("image.bin"))?;
        let sha256 = canonical::sha256(&data);
        ensure!(
            sha256 == frozen.ota_sha256,
            "image.bin does not match its review"
        );
        payload(&data)?;
        let build = build_identity(review, &frozen)?;

        Ok(Self {
            image: Bytes::from(data),
            sha256,
            build,
        })
    }
}

struct ImageState {
    build: Build,
    peer: Ipv4Addr,
    token: Uuid,
}

fn response(
    state: &ImageState,
    address: std::net::SocketAddr,
    headers: &HeaderMap,
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

    let data = state.build.image.clone();
    tracing::info!(
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
    if request.uri().query().is_some()
        || request.uri().path() != format!("/ota/{}/{ROUTE}", state.token)
    {
        return StatusCode::NOT_FOUND.into_response();
    }

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
        request.method() == Method::HEAD,
    )
}

async fn run(
    listener: TcpListener,
    build: Build,
    peer: Ipv4Addr,
    token: Uuid,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let state = Arc::new(ImageState { build, peer, token });
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
    review: &Path,
    peer: Ipv4Addr,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let std::net::IpAddr::V4(host) = config.mqtt_listen.ip() else {
        anyhow::bail!("OTA image host must use IPv4");
    };
    ensure!(
        host.is_private() && peer.is_private() && host != peer,
        "OTA image server requires distinct private IPv4 host/peer addresses"
    );

    let build = Build::load(review)?;
    let listener = TcpListener::bind((host, PORT))
        .await
        .context("bind OTA image server")?;

    let lease = Lease {
        token: Uuid::new_v4(),
        peer,
        expires_at: Utc::now() + chrono::Duration::seconds(ACCEPT_WINDOW_SECONDS),
        sha256: build.sha256.clone(),
        build: build.build.clone(),
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
        println!("Build: http://{host}:{PORT}/ota/{}/{ROUTE}", lease.token);
    }

    let result = run(listener, build, peer, lease.token, async {
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let token = Uuid::new_v4();
        let build = Build {
            image: Bytes::from_static(b"test-image"),
            sha256: String::new(),
            build: String::new(),
        };

        let worker = tokio::spawn(run(listener, build, peer, token, std::future::pending()));
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
    async fn raw_http_uses_stock_compatible_header_case_and_exact_complete_objects() {
        let (address, token, worker) = start(Ipv4Addr::LOCALHOST).await;

        let reply = exchange(
            address,
            &format!("GET /ota/{token}/{ROUTE} HTTP/1.1\r\nHost: lab\r\n\r\n"),
        )
        .await;
        let (headers, body) = split(&reply);
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains("\r\nContent-Length: 10\r\n"));
        assert!(headers.contains("\r\nConnection: close\r\n"));
        assert!(!headers.contains("content-length:") && !headers.contains("Transfer-Encoding:"));
        assert!(!headers.contains("Content-Encoding:"));
        assert_eq!(body, b"test-image");

        let reply = exchange(
            address,
            &format!("HEAD /ota/{token}/{ROUTE} HTTP/1.1\r\nHost: lab\r\n\r\n"),
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
            ("GET", format!("/ota/{}/{ROUTE}", Uuid::nil()), "", 404),
            ("GET", format!("/ota/{token}/original.bin"), "", 404),
            ("GET", format!("/ota/{token}/test.bin"), "", 404),
            ("POST", format!("/ota/{token}/{ROUTE}"), "", 405),
            (
                "GET",
                format!("/ota/{token}/{ROUTE}"),
                "Range: bytes=0-9\r\n",
                400,
            ),
            (
                "GET",
                format!("/ota/{token}/{ROUTE}"),
                "If-None-Match: x\r\n",
                400,
            ),
            ("GET", format!("/ota/{token}/{ROUTE}?x=1"), "", 404),
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
            &format!("GET /ota/{token}/{ROUTE} HTTP/1.1\r\nHost: lab\r\n\r\n"),
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

        assert!(current(&config, host).await.is_none());

        let lease = Lease {
            token: Uuid::new_v4(),
            peer: Ipv4Addr::new(10, 0, 0, 2),
            expires_at: Utc::now(),
            sha256: canonical::sha256(b"build"),
            build: "ab".repeat(32),
        };

        storage::write_json(&Lease::path(&config), &lease).unwrap();
        assert!(current(&config, host).await.is_none());

        let listener = TcpListener::bind((host, PORT)).await.unwrap();
        assert_eq!(current(&config, host).await, Some(lease));

        std::fs::write(Lease::path(&config), b"{\"token\":\"x\"}").unwrap();
        assert!(current(&config, host).await.is_none());

        drop(listener);
    }

    /// A review directory holding `image`, a manifest naming `identity`, and
    /// a review naming `digest` and that manifest.
    fn review_of(image: &[u8], digest: &str, kind: &str, identity: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("image.bin"), image).unwrap();
        let manifest = serde_json::json!({"startup_identity": identity, "profile": "firstboot"});
        let manifest = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(directory.path().join("build-manifest.json"), &manifest).unwrap();
        let review = serde_json::json!({
            "schema_version": 1,
            "kind": "plaf103_development_image_review",
            "image_kind": kind,
            "ota_sha256": digest,
            "payload_sha256": "",
            "payload_bytes": 0,
            "manifest_sha256": canonical::sha256(&manifest),
            "review": {"stock_sha256": ""},
            "reviewer_sha256": "",
            "application_boot_verified": false,
        });
        storage::write_json(&directory.path().join("review.json"), &review).unwrap();
        directory
    }

    fn review(image: &[u8], digest: &str, kind: &str) -> tempfile::TempDir {
        review_of(image, digest, kind, &"AB".repeat(32))
    }

    #[test]
    fn a_build_loads_only_when_its_review_binds_it() {
        let mut image = vec![0xff; 32];
        image.extend_from_slice(b"81958711");
        image.extend_from_slice(&[0x5a; 64]);
        let digest = canonical::sha256(&image);

        let build = Build::load(review(&image, &digest, "build").path()).unwrap();
        assert_eq!(build.image.as_ref(), image.as_slice());
        assert_eq!(build.sha256, digest);
        assert_eq!(build.build, "ab".repeat(32));

        assert!(Build::load(review_of(&image, &digest, "build", "").path()).is_err());
        assert!(Build::load(review_of(&image, &digest, "build", &"g".repeat(64)).path()).is_err());
        let tampered = review(&image, &digest, "build");
        std::fs::write(
            tampered.path().join("build-manifest.json"),
            br#"{"startup_identity":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}"#,
        )
        .unwrap();
        assert!(Build::load(tampered.path()).is_err());

        let other = canonical::sha256(b"other");
        assert!(Build::load(review(&image, &other, "build").path()).is_err());
        assert!(Build::load(review(&image, &digest, "stock").path()).is_err());

        let mut unmarked = image.clone();
        unmarked[32] = 0;
        let unmarked_digest = canonical::sha256(&unmarked);
        assert!(Build::load(review(&unmarked, &unmarked_digest, "build").path()).is_err());

        let mut oversized = image.clone();
        oversized.resize(IMAGE_BYTES + 1, 0xff);
        let oversized_digest = canonical::sha256(&oversized);
        assert!(Build::load(review(&oversized, &oversized_digest, "build").path()).is_err());
    }
}
