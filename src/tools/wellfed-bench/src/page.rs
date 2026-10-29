//! `serve --ui`: the read-only progress page beside the server, fed by its
//! live state. The page is optional: a binary built without the `ui`
//! feature refuses `--ui` before the server opens anything.

use std::path::Path;
use std::thread::JoinHandle;

use wellfed_bench_server::Store;
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
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::{Cancel, JoinHandle, Page, Store};
    use wellfed_formats::bench::progress::{Current, RelayState};

    /// The running job's record and the board's relays, from the server.
    struct ServerLive(Store);

    impl wellfed_dashboard::Live for ServerLive {
        fn current(&self) -> Option<Current> {
            self.0.read().job.and_then(|job| job.record)
        }

        fn relays(&self) -> Option<BTreeMap<String, RelayState>> {
            Some(self.0.read().relays)
        }
    }

    pub(crate) fn available() -> anyhow::Result<()> {
        Ok(())
    }

    impl Page<'_> {
        /// Bind the page and serve it until `stop`.
        pub(crate) fn start(&self, stop: &Cancel, store: Store) -> anyhow::Result<JoinHandle<()>> {
            let listen = wellfed_dashboard::Listen::new(self.address, self.port)?;
            let handle = wellfed_dashboard::start(
                self.root.to_path_buf(),
                self.flash_bin.to_path_buf(),
                &listen,
                stop.clone(),
                Arc::new(ServerLive(store)),
            )?;
            wellfed_rig::say!("Bench page on {}", listen.url());
            Ok(handle)
        }
    }
}

#[cfg(not(feature = "ui"))]
mod built {
    use super::{Cancel, JoinHandle, Page, Store};

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
