//! `GET /wait?for=...&timeout_ms=...`: hold the request until a condition
//! holds, so a client (the MCP server, a script) waits on the bench rather
//! than polling it:
//! - `for=job&id=N`: job `N` ended, or a later job started, so it had;
//! - `for=line&text=T`: a UART, device log or job output line containing
//!   `T` arrived after the wait began;
//! - `for=event&kind=K`: a device event of kind `K` arrived after it began;
//! - `for=status&build=B&bank=N`: the feeder's last status names a build
//!   starting with `B`, from bank `N` (either may be left out, not both);
//! - `for=timer&label=L`: the operator's timer `L` is due, seen or missed;
//! - `for=idle`: no job runs and the flash client holds no UART.
//!
//! 200 with `met` and how, or `timed-out`; 400 for a condition missing
//! what it needs. A wait only reads: it changes nothing on the bench.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Json, Response};
use tokio::sync::broadcast::error::RecvError;
use wellfed_formats::bench::live::{BenchState, JobId, Uart};
use wellfed_formats::bench::notes::TimerState;
use wellfed_formats::bench::now;

use crate::api::{Refusal, WAIT_MAX_MS, Wait, WaitError, WaitFor, Waited};
use crate::notes::{Heard, event_kind};
use crate::server::Shared;

/// How often a wait looks at the clock, for a timer falling due.
const TICK: Duration = Duration::from_millis(500);

/// A checked wait.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Condition {
    Job(JobId),
    Line(String),
    Event(String),
    Status {
        build: Option<String>,
        bank: Option<u8>,
    },
    Timer(String),
    Idle,
}

impl TryFrom<&Wait> for Condition {
    type Error = WaitError;

    fn try_from(wait: &Wait) -> Result<Self, WaitError> {
        let missing = |field: &str| WaitError::Missing {
            what: format!("{:?}", wait.what).to_lowercase(),
            field: field.into(),
        };
        wellfed_formats::require(wait.timeout_ms <= WAIT_MAX_MS, || WaitError::TooLong {
            asked_ms: wait.timeout_ms,
            max_ms: WAIT_MAX_MS,
        })?;

        Ok(match wait.what {
            WaitFor::Job => Self::Job(wait.id.ok_or_else(|| missing("id"))?),
            WaitFor::Line => {
                let text = wait.text.clone().ok_or_else(|| missing("text"))?;
                wellfed_formats::require(!text.is_empty(), || WaitError::EmptyText)?;
                Self::Line(text)
            }
            WaitFor::Event => {
                let kind = wait.kind.clone().ok_or_else(|| missing("kind"))?;
                event_kind(&kind).map_err(|_| WaitError::UnknownEvent { kind: kind.clone() })?;
                Self::Event(kind)
            }
            WaitFor::Status => {
                wellfed_formats::require(wait.build.is_some() || wait.bank.is_some(), || {
                    missing("build or bank")
                })?;
                Self::Status {
                    build: wait.build.as_ref().map(|build| build.to_ascii_lowercase()),
                    bank: wait.bank,
                }
            }
            WaitFor::Timer => Self::Timer(wait.label.clone().ok_or_else(|| missing("label"))?),
            WaitFor::Idle => Self::Idle,
        })
    }
}

impl Condition {
    /// Whether the state already says so, and how.
    fn held(&self, state: &BenchState) -> Option<String> {
        match self {
            Self::Job(id) => {
                let job = state.job.as_ref()?;
                if job.id > *id {
                    return Some(format!("job {id} ended before job {} began", job.id));
                }
                let end = job.end.as_ref().filter(|_| job.id == *id)?;
                Some(match &end.error {
                    Some(error) => format!("job {id} stopped ({}): {error}", end.code),
                    None => format!("job {id} ended with code {}", end.code),
                })
            }
            Self::Status { build, bank } => {
                let status = state.feeder.status.as_ref()?;
                let build_ok = build
                    .as_ref()
                    .is_none_or(|build| status.build.starts_with(build.as_str()));
                let bank_ok = bank.is_none_or(|bank| status.bank == bank);
                (build_ok && bank_ok)
                    .then(|| format!("the feeder runs {} from bank {}", status.build, status.bank))
            }
            Self::Timer(label) => {
                let timer = state
                    .operator
                    .timers
                    .iter()
                    .find(|timer| timer.label == *label)?;
                match &timer.state {
                    TimerState::Seen { line, .. } => Some(format!("timer {label} saw {line:?}")),
                    TimerState::Missed { .. } => Some(format!("timer {label} was missed")),
                    TimerState::Waiting => {
                        (now() >= timer.due).then(|| format!("timer {label} is due"))
                    }
                }
            }
            Self::Idle => {
                let running = state.job.as_ref().is_some_and(|job| job.end.is_none());
                let uart = matches!(state.uart, Uart::Writing { .. } | Uart::Loader);
                (!running && !uart).then(|| "the bench is idle".to_string())
            }
            Self::Line(_) | Self::Event(_) => None,
        }
    }

    /// Whether `heard` meets it, and how.
    fn hears(&self, heard: &Heard) -> Option<String> {
        match (self, heard) {
            (Self::Line(text), Heard::Line(line)) if line.contains(text.as_str()) => {
                Some(line.clone())
            }
            (Self::Event(kind), Heard::Event(seen)) if kind == seen => {
                Some(format!("{seen} event"))
            }
            _ => None,
        }
    }
}

pub(crate) async fn wait(State(shared): State<Arc<Shared>>, Query(wait): Query<Wait>) -> Response {
    let condition = match Condition::try_from(&wait) {
        Ok(condition) => condition,
        Err(error) => return Refusal::Wait { error }.into_response(),
    };
    Json(until(&shared, &condition, Duration::from_millis(wait.timeout_ms)).await).into_response()
}

async fn until(shared: &Shared, condition: &Condition, timeout: Duration) -> Waited {
    // Both taken before the first look, so nothing that happens between
    // the look and the wait is missed.
    let mut states = shared.store.subscribe();
    let mut heard = shared.notes.listen();
    let started = Instant::now();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut ticks = tokio::time::interval(TICK);

    loop {
        let held = condition.held(&states.borrow_and_update());
        if let Some(detail) = held {
            return Waited::Met { detail };
        }
        tokio::select! {
            changed = states.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            got = heard.recv() => match got {
                Ok(got) => {
                    if let Some(detail) = condition.hears(&got) {
                        return Waited::Met { detail };
                    }
                }
                // Fell behind a burst: what was dropped is gone, the rest
                // still counts.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            _ = ticks.tick() => {}
            () = &mut deadline => break,
        }
    }
    Waited::TimedOut {
        waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(what: WaitFor) -> Wait {
        Wait {
            what,
            id: None,
            text: None,
            kind: None,
            build: None,
            bank: None,
            label: None,
            timeout_ms: 10,
        }
    }

    #[test]
    fn a_wait_names_what_it_needs_and_lasts_at_most_ten_minutes() {
        assert!(matches!(
            Condition::try_from(&asked(WaitFor::Job)),
            Err(WaitError::Missing { field, .. }) if field == "id"
        ));
        assert!(matches!(
            Condition::try_from(&Wait {
                kind: Some("feed".into()),
                ..asked(WaitFor::Event)
            }),
            Err(WaitError::UnknownEvent { .. })
        ));
        assert!(matches!(
            Condition::try_from(&Wait {
                timeout_ms: WAIT_MAX_MS + 1,
                ..asked(WaitFor::Idle)
            }),
            Err(WaitError::TooLong { .. })
        ));
        assert_eq!(
            Condition::try_from(&Wait {
                build: Some("A1B2".into()),
                ..asked(WaitFor::Status)
            }),
            Ok(Condition::Status {
                build: Some("a1b2".into()),
                bank: None
            })
        );
    }

    #[test]
    fn a_wait_reads_back_from_its_query() {
        let wait = Wait {
            text: Some("PLFW1 start bank=1".into()),
            timeout_ms: 1500,
            ..asked(WaitFor::Line)
        };

        let query = serde_urlencoded::to_string(&wait).unwrap();

        assert!(query.starts_with("for=line&text=PLFW1+start"), "{query}");
        assert_eq!(serde_urlencoded::from_str::<Wait>(&query).unwrap(), wait);
    }

    #[test]
    fn a_line_or_event_meets_only_its_own_condition() {
        let line = Condition::Line("PLFW1 start".into());
        let event = Condition::Event("boot".into());

        assert!(
            line.hears(&Heard::Line("x PLFW1 start build=00".into()))
                .is_some()
        );
        assert!(line.hears(&Heard::Event("boot".into())).is_none());
        assert_eq!(
            event.hears(&Heard::Event("boot".into())).as_deref(),
            Some("boot event")
        );
        assert!(event.hears(&Heard::Event("fed".into())).is_none());
    }
}
