//! The suite's error type: the shared waits refuse with their own variants,
//! each scenario module with its own enum, and the rig's error is wrapped
//! whole.
//!
//! A scenario's failure is recorded as this error's `Display` text in the
//! run record, so the text is what the operator and the bench page read.
//! The operator's interrupt arrives as the rig's
//! [`wellfed_rig::Error::Interrupted`]; [`Error::interrupted`] tells it
//! apart, so a wait that tolerates other failures still stops on it.

use std::sync::Arc;

use wellfed_rig::commands::Image;
use wellfed_rig::device::{Answer, DeviceError};
use wellfed_rig::server::{NotReady, StockNotReady};
use wellfed_rig::tools::PushError;

use crate::feeds::FeedsError;
use crate::live::LiveError;
use crate::scenarios::ScenariosError;
use crate::stock::StockError;
use crate::{SCENARIOS, SILENT_SECONDS, compact};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Rig(#[from] wellfed_rig::Error),
    #[error(transparent)]
    Scenarios(#[from] ScenariosError),
    #[error(transparent)]
    Feeds(#[from] FeedsError),
    #[error(transparent)]
    Stock(#[from] StockError),
    #[error(transparent)]
    Live(#[from] LiveError),

    /// A rig without the capability a scenario asked for (the test rigs).
    #[error("This rig {lacks}")]
    Unsupported { lacks: &'static str },
    /// No UART record matching `what` arrived in time.
    #[error("No {what} within {seconds:.0} s")]
    NoRecord { what: String, seconds: f64 },
    /// A status poll never passed; `last` is the reason it last gave.
    #[error("No {what} within {seconds:.0} s ({last})")]
    Waited {
        what: String,
        seconds: f64,
        last: Box<Error>,
    },
    #[error("Status answered {}", compact(answer))]
    StatusAnswer { answer: Answer },
    #[error("The clock sync answered {}", compact(answer))]
    ClockSync { answer: Answer },
    /// The scenarios need a trial and the device API, which stock has not.
    #[error(
        "The integration suite installs replacement builds only; offer stock with `wellfed-bench ota`"
    )]
    StockInstall,
    #[error("The update over the device API ended before any UART outcome: {0}")]
    PushEnded(Arc<PushError>),
    /// `build` is the offered build's first 12 digits.
    #[error("No update outcome for {build} within {minutes:.0} min")]
    NoOutcome { build: String, minutes: f64 },
    #[error("No confirmed baseline build")]
    NoBaseline,
    #[error("Signing {label} failed")]
    Unsigned { label: String },
    #[error("Building or signing {image} failed")]
    Unbuilt { image: Image },
    /// The feeder looks hung or bricked; the run stops with the recovery
    /// commands.
    #[error("no UART record or device API answer for {:.0} s", SILENT_SECONDS)]
    Silent,
    #[error("Unknown scenario {name:?}; --list shows them ({})", scenario_names())]
    UnknownScenario { name: String },
}

impl Error {
    /// The operator's interrupt: every wait passes it on as it is.
    pub fn interrupted(&self) -> bool {
        matches!(self, Self::Rig(wellfed_rig::Error::Interrupted))
    }
}

fn scenario_names() -> String {
    SCENARIOS
        .iter()
        .map(|s| s.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the rig's own `?` conversions accept, wrapped as the rig wraps it.
macro_rules! through_rig {
    ($($from:ty),* $(,)?) => {$(
        impl From<$from> for Error {
            fn from(error: $from) -> Self {
                Self::Rig(error.into())
            }
        }
    )*};
}

through_rig!(
    NotReady,
    StockNotReady,
    DeviceError,
    std::io::Error,
    serde_json::Error,
    wellfed_stock_api::ApiError,
    wellfed_formats::Error,
);

pub type Result<T, E = Error> = std::result::Result<T, E>;
