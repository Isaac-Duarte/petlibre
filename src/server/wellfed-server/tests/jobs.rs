//! The background jobs' supervisor on a manual clock: each job runs at
//! its time, one at a time, catching up once after downtime, and stops
//! when the server does. Then the real jobs on the service: expiry ends
//! sessions, and readiness shows each job's last run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_api::{Credentials, ErrorCode, Job, JobOutcome, JobRun, NewUser, Role};
use wellfed_server::jobs::{self, Runner};
use wellfed_server_client::StatusCode;
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestServer, WAIT, ada_login, ada_setup, refused, until};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

const HOUR: Duration = Duration::from_secs(3600);

fn at(text: &str) -> Timestamp {
    text.parse().expect("a test time parses")
}

/// A runner that logs each job's start, and holds each run until the test
/// lets it finish when it has a gate.
struct Fake {
    clock: ManualClock,
    zone: TimeZone,
    last: Vec<JobRun>,
    started: mpsc::UnboundedSender<(Job, Timestamp)>,
    gate: Option<Arc<Semaphore>>,
    running: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
    /// Whether the release check is scheduled, as `releases.fetch` says.
    fetching: Arc<AtomicBool>,
}

/// What a test watches a [`Fake`] by.
struct Watch {
    started: mpsc::UnboundedReceiver<(Job, Timestamp)>,
    gate: Arc<Semaphore>,
    most: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
    fetching: Arc<AtomicBool>,
}

impl Watch {
    /// The next `n` starts, in order.
    async fn starts(&mut self, n: usize) -> Vec<(Job, Timestamp)> {
        let mut starts = Vec::new();
        for _ in 0..n {
            let start = tokio::time::timeout(WAIT, self.started.recv()).await;
            starts.push(
                start
                    .expect("a job starts in time")
                    .expect("the runner lives"),
            );
        }
        starts
    }

    /// Nothing more starts for a moment.
    async fn quiet(&mut self) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(self.started.try_recv().is_err(), "no other job starts");
    }
}

fn runner(clock: &ManualClock, zone: &str, last: Vec<JobRun>, gated: bool) -> (Fake, Watch) {
    let (tx, rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Semaphore::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let fetching = Arc::new(AtomicBool::new(true));
    let fake = Fake {
        clock: clock.clone(),
        zone: TimeZone::get(zone).expect("the host's tzdb knows the zone"),
        last,
        started: tx,
        gate: gated.then(|| Arc::clone(&gate)),
        running: Arc::new(AtomicUsize::new(0)),
        most: Arc::clone(&most),
        finished: Arc::clone(&finished),
        fetching: Arc::clone(&fetching),
    };
    let watch = Watch {
        started: rx,
        gate,
        most,
        finished,
        fetching,
    };
    (fake, watch)
}

impl Runner for Fake {
    async fn run(&self, job: Job) -> JobRun {
        let started = wellfed_service::Clock::now(&self.clock);
        let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(running, Ordering::SeqCst);
        self.started.send((job, started)).expect("the test watches");
        if let Some(gate) = &self.gate {
            gate.acquire().await.expect("the gate stays open").forget();
        }

        self.running.fetch_sub(1, Ordering::SeqCst);
        self.finished.fetch_add(1, Ordering::SeqCst);
        JobRun {
            job,
            started,
            finished: wellfed_service::Clock::now(&self.clock),
            outcome: JobOutcome::Succeeded,
        }
    }

    async fn last_runs(&self) -> Vec<JobRun> {
        self.last.clone()
    }

    fn scheduled(&self, job: Job) -> bool {
        job != Job::ReleaseCheck || self.fetching.load(Ordering::SeqCst)
    }

    fn zone(&self) -> TimeZone {
        self.zone.clone()
    }
}

#[tokio::test]
async fn each_job_runs_at_its_time() {
    let clock = ManualClock::new();
    clock.advance_to(at("2026-10-27T00:00:00Z"));
    let (fake, mut watch) = runner(&clock, "Europe/Berlin", Vec::new(), false);
    let cancel = CancellationToken::new();
    let task = jobs::spawn(fake, Arc::new(clock.clone()), cancel.clone());

    // The hourly jobs have never run, so they run at once; the daily ones
    // wait for their time in Berlin (UTC+1 on this date).
    let midnight = at("2026-10-27T00:00:00Z");
    assert_eq!(
        watch.starts(2).await,
        [(Job::Rollups, midnight), (Job::Expiry, midnight)]
    );
    watch.quiet().await;

    clock.advance(HOUR);
    let one = at("2026-10-27T01:00:00Z");
    assert_eq!(
        watch.starts(2).await,
        [(Job::Rollups, one), (Job::Expiry, one)]
    );

    // 03:00 Berlin: retention and the hourly jobs, due at the same time,
    // in the jobs' order.
    clock.advance(HOUR);
    let two = at("2026-10-27T02:00:00Z");
    assert_eq!(
        watch.starts(3).await,
        [
            (Job::Retention, two),
            (Job::Rollups, two),
            (Job::Expiry, two)
        ]
    );
    watch.quiet().await;

    // Not a moment early: 03:29:59 runs nothing; 03:30 the backup.
    clock.advance(Duration::from_secs(30 * 60 - 1));
    watch.quiet().await;
    clock.advance(Duration::from_secs(1));
    assert_eq!(
        watch.starts(1).await,
        [(Job::Backup, at("2026-10-27T02:30:00Z"))]
    );

    // 04:00 Berlin: the hourly jobs and the release check.
    clock.advance(Duration::from_secs(30 * 60));
    let three = at("2026-10-27T03:00:00Z");
    assert_eq!(
        watch.starts(3).await,
        [
            (Job::Rollups, three),
            (Job::Expiry, three),
            (Job::ReleaseCheck, three)
        ]
    );
    watch.quiet().await;

    cancel.cancel();
    task.await.expect("the supervisor ends cleanly");
}

#[tokio::test]
async fn jobs_never_overlap_and_catch_up_once() {
    let clock = ManualClock::new();
    clock.advance_to(at("2026-10-27T12:00:00Z"));
    // The server was down for three days: every job is overdue.
    let last = Job::ALL
        .into_iter()
        .map(|job| JobRun {
            job,
            started: at("2026-10-24T12:00:00Z"),
            finished: at("2026-10-24T12:00:01Z"),
            outcome: JobOutcome::Succeeded,
        })
        .collect();
    let (fake, mut watch) = runner(&clock, "UTC", last, true);
    let cancel = CancellationToken::new();
    let task = jobs::spawn(fake, Arc::new(clock.clone()), cancel.clone());

    // Each overdue job runs once, in the order they fell due, and none
    // starts until the one before it has finished, though the clock moves
    // on meanwhile.
    let mut starts = watch.starts(1).await;
    clock.advance(HOUR / 2);
    watch.quiet().await;
    for _ in 1..Job::ALL.len() {
        watch.gate.add_permits(1);
        starts.extend(watch.starts(1).await);
        watch.quiet().await;
    }
    watch.gate.add_permits(1);
    let (noon, later) = (at("2026-10-27T12:00:00Z"), at("2026-10-27T12:30:00Z"));
    assert_eq!(
        starts,
        [
            (Job::Rollups, noon),       // due 2026-10-24 13:00
            (Job::Expiry, later),       // the same, but after rollups
            (Job::Retention, later),    // due 2026-10-25 03:00
            (Job::Backup, later),       // 03:30
            (Job::ReleaseCheck, later), // 04:00
        ]
    );
    watch.quiet().await;
    assert_eq!(watch.most.load(Ordering::SeqCst), 1, "one at a time");
    until("the last run finishes", || async {
        watch.finished.load(Ordering::SeqCst) == Job::ALL.len()
    })
    .await;

    cancel.cancel();
    task.await.expect("the supervisor ends cleanly");
}

#[tokio::test]
async fn shutdown_ends_a_running_job_and_a_waiting_supervisor() {
    let clock = ManualClock::new();
    let (fake, mut watch) = runner(&clock, "UTC", Vec::new(), true);
    let cancel = CancellationToken::new();
    let task = jobs::spawn(fake, Arc::new(clock.clone()), cancel.clone());

    // Mid-run: the run is dropped, not finished or recorded.
    let started = watch.starts(1).await;
    assert_eq!(started[0].0, Job::Rollups);
    cancel.cancel();
    tokio::time::timeout(WAIT, task)
        .await
        .expect("the supervisor stops in time")
        .expect("and cleanly");
    assert_eq!(watch.finished.load(Ordering::SeqCst), 0);

    // Waiting for the next job's time.
    let (fake, mut watch) = runner(&clock, "UTC", Vec::new(), false);
    let cancel = CancellationToken::new();
    let task = jobs::spawn(fake, Arc::new(clock.clone()), cancel.clone());
    watch.starts(2).await;
    watch.quiet().await;
    cancel.cancel();
    tokio::time::timeout(WAIT, task)
        .await
        .expect("the supervisor stops in time")
        .expect("and cleanly");
}

#[tokio::test]
async fn a_release_check_turned_on_is_picked_up_at_the_next_wake() {
    let clock = ManualClock::new();
    clock.advance_to(at("2026-10-27T12:00:00Z"));
    let ran = |job, started: &str| JobRun {
        job,
        started: at(started),
        finished: at(started),
        outcome: JobOutcome::Succeeded,
    };
    let last = vec![
        ran(Job::Retention, "2026-10-27T03:00:00Z"),
        ran(Job::Rollups, "2026-10-27T11:30:00Z"),
        ran(Job::Expiry, "2026-10-27T11:30:00Z"),
        ran(Job::Backup, "2026-10-27T03:30:00Z"),
        ran(Job::ReleaseCheck, "2026-10-20T04:00:00Z"),
    ];
    let (fake, mut watch) = runner(&clock, "UTC", last, false);
    watch.fetching.store(false, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let task = jobs::spawn(fake, Arc::new(clock.clone()), cancel.clone());
    watch.quiet().await;

    // Turned on while the supervisor sleeps: at its next wake (here the
    // hourly jobs' 12:30, at most an hour on) the overdue check runs at
    // once, first, since it fell due a week ago.
    watch.fetching.store(true, Ordering::SeqCst);
    watch.quiet().await;
    clock.advance(HOUR / 2);
    let half = at("2026-10-27T12:30:00Z");
    assert_eq!(
        watch.starts(3).await,
        [
            (Job::ReleaseCheck, half),
            (Job::Rollups, half),
            (Job::Expiry, half)
        ]
    );
    watch.quiet().await;

    cancel.cancel();
    task.await.expect("the supervisor ends cleanly");
}

fn bob() -> Credentials {
    Credentials {
        name: "bob".into(),
        password: "battery staple".into(),
    }
}

#[tokio::test]
async fn expiry_ends_an_expired_accounts_sessions_and_readiness_shows_the_runs() {
    let clock = ManualClock::new();
    let server = TestServer::start_with_clock(Arc::new(clock.clone())).await;
    let mut owner = server.client();
    owner.setup(&ada_setup()).await.unwrap();
    let start = wellfed_service::Clock::now(&clock);
    let expires = start
        .checked_add(jiff::SignedDuration::from_mins(59))
        .unwrap();
    let Credentials { name, password } = bob();
    owner
        .create_user(&NewUser {
            name,
            password,
            role: Role::Member,
            expires: Some(expires),
        })
        .await
        .unwrap();
    let mut member = server.client();
    member.login(&bob()).await.unwrap();

    let service = server.service().clone();
    assert!(!service.job_scheduled(Job::ReleaseCheck), "fetching is off");
    assert!(service.job_scheduled(Job::Backup));
    let cancel = CancellationToken::new();
    let task = jobs::spawn(service.clone(), service.clock(), cancel.clone());
    until("the hourly jobs run at start", || async {
        service.job_runs().await.unwrap().len() == 2
    })
    .await;
    let ran = |job| JobRun {
        job,
        started: start,
        finished: start,
        outcome: JobOutcome::Succeeded,
    };
    let status = owner.status().await.unwrap();
    assert_eq!(status.jobs, [ran(Job::Rollups), ran(Job::Expiry)]);

    // Bob uses his session at 58 minutes, so at the hour it has not idled
    // out (the test's sessions last an hour) but his account, expiring at
    // 59, has; the owner's login, unused since the start, has idled out.
    // The job ends both: bob's session is gone rather than refused as
    // disabled, which only the expired-account clause can do.
    clock.advance(Duration::from_secs(58 * 60));
    member.feeders().await.unwrap();
    clock.advance(Duration::from_secs(2 * 60));
    let later = wellfed_service::Clock::now(&clock);
    until("the expiry job runs again", || async {
        let runs = service.job_runs().await.unwrap();
        runs.iter()
            .any(|run| run.job == Job::Expiry && run.started == later)
    })
    .await;
    refused(
        member.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    refused(
        owner.status().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    owner.login(&ada_login()).await.unwrap();
    let status = owner.status().await.unwrap();
    let ran = |job| JobRun {
        job,
        started: later,
        finished: later,
        outcome: JobOutcome::Succeeded,
    };
    assert_eq!(status.jobs, [ran(Job::Rollups), ran(Job::Expiry)]);

    cancel.cancel();
    task.await.expect("the supervisor ends cleanly");
    server.stop().await;
}
