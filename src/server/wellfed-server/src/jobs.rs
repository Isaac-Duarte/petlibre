//! The background jobs' supervisor: one task that runs each [`Job`] when
//! its [`Schedule`] says, one job at a time, until the server shuts down.
//!
//! Each job's body is a service method that takes the time it runs at;
//! the supervisor only decides when. It waits on the service's [`Clock`]
//! ([`Clock::sleep_until`]), so a test moves a manual clock and the jobs
//! due by then run, with no real time passing.
//!
//! - **One at a time.** Jobs run in turn on the one task, so no two runs
//!   (of one job or of two) ever overlap, and a job that takes long only
//!   delays the next.
//! - **Catching up.** A job's next time follows its last recorded run, so
//!   a job due while the server was down runs once when it starts, not
//!   once for every time it missed.
//! - **Shutdown.** Cancelling the token ends the task at once: a job cut
//!   short is not recorded, and runs again at its next time.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use jiff::civil::{self, Time};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info, info_span, warn};
use wellfed_api::{Job, JobRun};
use wellfed_service::{Clock, Service};

/// How long the supervisor sleeps at most before working its schedule out
/// again, so a change of the household's zone or of what is scheduled
/// (the owner allowing release fetches) moves the times within this.
const RECHECK: Duration = Duration::from_secs(3600);

/// What the supervisor runs jobs on: the service, or a test's stand-in.
pub trait Runner: Send + Sync + 'static {
    /// Run `job` now and record its run.
    fn run(&self, job: Job) -> impl Future<Output = JobRun> + Send;

    /// Every job's last recorded run; none when they cannot be read, so
    /// each job runs as if it never had.
    fn last_runs(&self) -> impl Future<Output = Vec<JobRun>> + Send;

    /// Whether `job` runs on its schedule at all.
    fn scheduled(&self, job: Job) -> bool;

    /// The zone the daily jobs keep their time of day in.
    fn zone(&self) -> TimeZone;
}

impl Runner for Service {
    fn run(&self, job: Job) -> impl Future<Output = JobRun> + Send {
        self.run_job(job)
    }

    async fn last_runs(&self) -> Vec<JobRun> {
        self.job_runs().await.unwrap_or_else(|error| {
            warn!("read the jobs' last runs: {error}");
            Vec::new()
        })
    }

    fn scheduled(&self, job: Job) -> bool {
        self.job_scheduled(job)
    }

    fn zone(&self) -> TimeZone {
        self.household_zone()
    }
}

/// When a job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// This long after its last run started, and at once when it never
    /// has run.
    Every(Duration),
    /// Each day at this time in the household's zone, at the first such
    /// time after its last run started, or after the supervisor started
    /// when it never has run.
    Daily(Time),
}

/// Each job's schedule. The night jobs are spread out so they do not all
/// start at once on a small host.
pub const fn schedule(job: Job) -> Schedule {
    const HOUR: Duration = Duration::from_secs(3600);
    match job {
        Job::Expiry | Job::Rollups => Schedule::Every(HOUR),
        Job::Retention => Schedule::Daily(civil::time(3, 0, 0, 0)),
        Job::Backup => Schedule::Daily(civil::time(3, 30, 0, 0)),
        Job::ReleaseCheck => Schedule::Daily(civil::time(4, 0, 0, 0)),
    }
}

impl Schedule {
    /// When a job on this schedule is next due: after its last run
    /// started (`last`), or, when it never has run, after the supervisor
    /// started (`started`).
    pub fn due(self, last: Option<Timestamp>, started: Timestamp, zone: &TimeZone) -> Timestamp {
        match self {
            Self::Every(period) => last.map_or(started, |last| later(last, period)),
            Self::Daily(time) => next_daily(last.unwrap_or(started), time, zone),
        }
    }
}

/// The first `time` of day in `zone` strictly after `after`. A time a DST
/// change skips is taken as the same moment the clocks show it would have
/// been, the way jiff's `compatible` reads it.
fn next_daily(after: Timestamp, time: Time, zone: &TimeZone) -> Timestamp {
    let today = after.to_zoned(zone.clone()).date();
    let days = [Ok(today), today.tomorrow()];
    for day in days.into_iter().flatten() {
        let Ok(at) = day.to_datetime(time).to_zoned(zone.clone()) else {
            continue;
        };
        if at.timestamp() > after {
            return at.timestamp();
        }
    }
    // Only at the end of the calendar: a day on.
    later(after, Duration::from_secs(24 * 3600))
}

/// `at` plus `by`, or the latest time there is.
fn later(at: Timestamp, by: Duration) -> Timestamp {
    SignedDuration::try_from(by)
        .ok()
        .and_then(|by| at.checked_add(by).ok())
        .unwrap_or(Timestamp::MAX)
}

/// Start the supervisor on `runner`, reading `clock`, until `cancel`.
pub fn spawn<R: Runner>(
    runner: R,
    clock: Arc<dyn Clock>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(supervise(runner, clock, cancel).instrument(info_span!(parent: None, "jobs")))
}

async fn supervise<R: Runner>(runner: R, clock: Arc<dyn Clock>, cancel: CancellationToken) {
    let started = clock.now();
    let mut last: HashMap<Job, Timestamp> = runner
        .last_runs()
        .await
        .into_iter()
        .map(|run| (run.job, run.started))
        .collect();

    loop {
        let zone = runner.zone();
        let next = Job::ALL
            .into_iter()
            .filter(|job| runner.scheduled(*job))
            .map(|job| {
                let due = schedule(job).due(last.get(&job).copied(), started, &zone);
                (due, job)
            })
            .min_by_key(|(due, _)| *due);

        let now = clock.now();
        if let Some((due, job)) = next
            && due <= now
        {
            info!(job = job.code(), "running");
            let run = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                run = runner.run(job) => run,
            };
            last.insert(job, run.started);
            continue;
        }

        let recheck = later(now, RECHECK);
        let wake = next.map_or(recheck, |(due, _)| due.min(recheck));
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = clock.sleep_until(wake) => {}
        }
    }
    info!("stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn an_hourly_job_runs_an_hour_after_its_last_start_or_at_once() {
        let hourly = Schedule::Every(Duration::from_secs(3600));
        let started = at("2026-10-27T12:00:00Z");

        assert_eq!(hourly.due(None, started, &TimeZone::UTC), started);
        let last = at("2026-10-27T10:15:00Z");
        assert_eq!(
            hourly.due(Some(last), started, &TimeZone::UTC),
            at("2026-10-27T11:15:00Z"),
            "overdue, so it runs at once"
        );
    }

    #[test]
    fn a_daily_job_runs_at_its_local_time_after_its_last_start() {
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        let backup = schedule(Job::Backup);
        let started = at("2026-10-27T12:00:00Z");

        // Never run: the next 03:30 in Berlin (UTC+1 in winter).
        assert_eq!(
            backup.due(None, started, &berlin),
            at("2026-10-28T02:30:00Z")
        );
        // Run at 03:30 today: tomorrow's.
        assert_eq!(
            backup.due(Some(at("2026-10-27T02:30:00Z")), started, &berlin),
            at("2026-10-28T02:30:00Z")
        );
        // Last run days ago: due once, at the first 03:30 after it.
        assert_eq!(
            backup.due(Some(at("2026-10-20T02:30:00Z")), started, &berlin),
            at("2026-10-21T01:30:00Z"),
            "summer time then: UTC+2"
        );
    }

    #[test]
    fn a_time_the_clocks_skip_runs_once_that_day() {
        // Europe/Berlin skips 02:00-03:00 on 2026-03-29.
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        let skipped = Schedule::Daily(civil::time(2, 30, 0, 0));
        let before = at("2026-03-28T12:00:00Z");

        let due = skipped.due(Some(before), before, &berlin);

        assert_eq!(due, at("2026-03-29T01:30:00Z"), "03:30 summer time");
        let after = skipped.due(Some(due), before, &berlin);
        assert_eq!(after, at("2026-03-30T00:30:00Z"));
    }

    #[test]
    fn a_time_the_clocks_repeat_runs_once_that_day() {
        // Europe/Berlin shows 02:00-03:00 twice on 2026-10-25.
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        let repeated = Schedule::Daily(civil::time(2, 30, 0, 0));
        let before = at("2026-10-24T12:00:00Z");

        let due = repeated.due(Some(before), before, &berlin);

        assert_eq!(due, at("2026-10-25T00:30:00Z"), "the first, summer time");
        let after = repeated.due(Some(due), before, &berlin);
        assert_eq!(
            after,
            at("2026-10-26T01:30:00Z"),
            "not the second 02:30 an hour later, but the next day's"
        );
    }
}
