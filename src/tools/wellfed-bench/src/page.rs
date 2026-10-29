//! `serve --ui`: the read-only progress page beside the server, fed by its
//! live state. The page is optional: a binary built without the `ui`
//! feature refuses `--ui` before the server opens anything.

use std::path::Path;
use std::thread::JoinHandle;

use wellfed_bench_server::{Outputs, Store};
use wellfed_rig::Cancel;

/// Where the page serves and what it reports.
pub(crate) struct Page<'a> {
    pub root: &'a Path,
    pub flash_bin: &'a Path,
    /// Loopback, or one named interface such as Tailscale.
    pub address: &'a str,
    pub port: u16,
}

#[cfg(feature = "ui")]
mod built {
    use std::sync::Arc;

    use super::{Cancel, JoinHandle, Outputs, Page, Store};
    use tokio::sync::watch;
    use wellfed_bench_server::JobOutput;
    use wellfed_formats::bench::live::{BenchState, JobId};

    /// The server's live state and its jobs' output, in this process.
    struct ServerLive {
        store: Store,
        outputs: Outputs,
    }

    impl wellfed_dashboard::Live for ServerLive {
        fn subscribe(&self) -> watch::Receiver<BenchState> {
            self.store.subscribe()
        }

        fn output(&self, id: JobId) -> Option<Arc<dyn wellfed_dashboard::JobOutput>> {
            let output = self.outputs.get(id)?;
            Some(Arc::new(Output(output)))
        }
    }

    struct Output(JobOutput);

    impl wellfed_dashboard::JobOutput for Output {
        fn read_from(&self, cursor: usize) -> (Vec<String>, usize) {
            self.0.read_from(cursor)
        }

        fn subscribe(&self) -> watch::Receiver<u64> {
            self.0.subscribe()
        }
    }

    pub(crate) fn available() -> anyhow::Result<()> {
        Ok(())
    }

    impl Page<'_> {
        /// Bind the page and serve it until `stop`.
        pub(crate) fn start(
            &self,
            stop: &Cancel,
            store: Store,
            outputs: Outputs,
        ) -> anyhow::Result<JoinHandle<()>> {
            let listen = wellfed_dashboard::Listen::new(self.address, self.port)?;
            let handle = wellfed_dashboard::start(
                self.root.to_path_buf(),
                self.flash_bin.to_path_buf(),
                &listen,
                stop.clone(),
                Arc::new(ServerLive { store, outputs }),
            )?;
            wellfed_rig::say!("Bench page on {}", listen.url());
            Ok(handle)
        }
    }
}

#[cfg(not(feature = "ui"))]
mod built {
    use super::{Cancel, JoinHandle, Outputs, Page, Store};

    pub(crate) fn available() -> anyhow::Result<()> {
        anyhow::bail!(
            "wellfed-bench was built without the web page; run `make ui`, then build it with \
             `cargo build --locked --release -p wellfed-bench --features ui`"
        )
    }

    impl Page<'_> {
        pub(crate) fn start(
            &self,
            _stop: &Cancel,
            _store: Store,
            _outputs: Outputs,
        ) -> anyhow::Result<JoinHandle<()>> {
            available()?;
            anyhow::bail!(
                "the page for {}:{} (from {}, reporting {}) is not built in",
                self.address,
                self.port,
                self.root.display(),
                self.flash_bin.display()
            )
        }
    }
}

pub(crate) use built::available;
