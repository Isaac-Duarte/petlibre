//! A feeder task's log lines name the feeder, through the span the task
//! runs in, without each line repeating it.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use wellfed_api::FeederId;
use wellfed_mirror::{Config, Discovery, Feeder, Mirror, Timing};
use wellfed_server_testkit::fixed::{FixedClock, SeededRandom};
use wellfed_server_testkit::{TempStore, until};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

/// What the subscriber wrote, shared with the test.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

// A current-thread runtime, so the task runs on the thread whose default
// subscriber the test set.
#[tokio::test]
async fn every_line_from_a_feeder_task_names_the_feeder() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);
    let temp = TempStore::new();
    let mirror = Mirror::new(
        temp.open().await,
        Config {
            discovery: Discovery::Off,
            timing: Timing {
                retry_min: Duration::from_millis(20),
                retry_max: Duration::from_millis(40),
                time_sync: Duration::from_secs(3600),
                answer: Duration::from_secs(1),
            },
        },
        Arc::new(FixedClock),
        Arc::new(SeededRandom::fresh()),
        CancellationToken::new(),
    );
    let id = FeederId(Uuid::from_u128(0x5eed));

    // Nothing listens on port 1, so every connect fails and is logged. The
    // start runs inside a request's span, as an adoption over HTTP does; the
    // task must not inherit it.
    let _updates = mirror
        .start(Feeder {
            id,
            identity: [0x42; 32],
            secret: [0x33; 32],
            address: Some("127.0.0.1:1".into()),
        })
        .instrument(tracing::info_span!("request", id = "adoption"))
        .await;
    until("two failed connects are logged", || {
        let text = captured.text();
        async move { text.matches("connect failed").count() >= 2 }
    })
    .await;
    mirror.shutdown().await;

    let text = captured.text();
    let span = format!("feeder{{id={id}}}");
    for line in text.lines() {
        assert!(line.contains(&span), "{line}");
        assert!(!line.contains("request"), "{line}");
    }
}
