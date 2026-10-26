//! The simulated feeder over real TCP on localhost, driven by
//! `wellfed-connect` as the server and the CLI drive a feeder.

use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use wellfed_client::api::Invite;
use wellfed_connect::{Connection, Error as ConnectError, join};
use wellfed_core::clock::ntp;
use wellfed_device_api::mailbox::KeyResult;
use wellfed_device_api::mailbox::Lock;
use wellfed_feeder::panel::{Action, Event as PanelEvent, Source as PanelSource};
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_feeder_sim::serve::Event;
use wellfed_feeder_sim::sim::Record;
use wellfed_model::{
    Answer, ClientRole, EventBody, Generation, Joined, MetricKind, NtpResult, NtpServer, Op,
    Pushed, Refusal, SampleValue, SettingsPatch, Source, Stats, StatsSection, Status, TimeChanges,
    TimeSource, TimeSync, Wifi,
};
use wellfed_proto::api::RequestId;
use wellfed_session::noise::Keypair;
use tokio::task::LocalSet;

const OWNER: [u8; 32] = [0x21; 32];
const MEMBER: [u8; 32] = [0x31; 32];
const WAIT: Duration = Duration::from_secs(5);

/// A September 2026 instant, inside the clock's accepted range.
const NOW_MS: u64 = 1_790_000_000_000;

fn keys(secret: [u8; 32]) -> Keypair {
    Keypair::from_secret(secret)
}

/// The shared harness, with this file's owner and request ids.
struct Running {
    sim: harness::Running,
    ids: u8,
}

impl std::ops::Deref for Running {
    type Target = harness::Running;

    fn deref(&self) -> &harness::Running {
        &self.sim
    }
}

impl std::ops::DerefMut for Running {
    fn deref_mut(&mut self) -> &mut harness::Running {
        &mut self.sim
    }
}

impl Running {
    /// The simulator on `flash`, enrolling the test's owner unless
    /// `enroll` is false.
    async fn start(flash: &Rc<Flash>, enroll: bool) -> Self {
        let owner = enroll.then(|| *keys(OWNER).public());
        Self {
            sim: harness::Running::start(flash, owner).await,
            ids: 0,
        }
    }

    async fn open(&self, secret: [u8; 32]) -> Result<Connection, ConnectError> {
        let ephemeral = [secret[0] ^ 0x80; 32];
        Connection::open(&keys(secret), self.identity, self.address, ephemeral).await
    }

    fn id(&mut self) -> RequestId {
        self.ids += 1;
        [self.ids; 16]
    }

    fn type_line(&self, line: &str) {
        self.sim.type_line(line).unwrap();
    }

    async fn stop(self) {
        self.sim.stop().await;
    }
}

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

/// A test on tokio's paused clock, which jumps ahead whenever every task
/// waits, so the 30 s relock and 90 s idle limit pass at once.
fn paused(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

async fn status(sim: &mut Running, connection: &mut Connection) -> Status {
    let answer = connection.ask(sim.id(), &Op::Status).await.unwrap();
    let Answer::Status(status) = answer else {
        panic!("{answer:?}");
    };
    status
}

/// Every event the subscription replays, until it is quiet.
async fn backlog(connection: &mut Connection) -> Vec<EventBody> {
    connection.subscribe(None, false).await.unwrap();
    let mut events = Vec::new();
    while let Ok(message) = connection.next(Duration::from_millis(500)).await {
        if let Pushed::Event(event) = message {
            events.push(event.body);
        }
    }
    events
}

#[test]
fn an_owner_reads_and_writes_the_feeder() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        assert_eq!(owner.role(), ClientRole::Owner);

        let status = owner.ask(sim.id(), &Op::Status).await.unwrap();
        let Answer::Status(status) = status else {
            panic!("{status:?}");
        };
        assert_eq!(status.locked, Some(true), "the panel boots locked");
        assert_eq!(status.clock, None, "no clock before a sync");

        let changes = SettingsPatch {
            volume: Some(20),
            ..SettingsPatch::default()
        };
        let put = Op::PutSettings {
            base: Generation(0),
            changes,
        };
        let done = owner.ask(sim.id(), &put).await.unwrap();
        assert_eq!(
            done,
            Answer::Done {
                generation: Some(Generation(1))
            }
        );
        let stale = owner.ask(sim.id(), &put).await.unwrap();
        assert_eq!(
            stale,
            Answer::Refused(Refusal::Stale {
                current: Generation(1)
            })
        );
        let settings = owner.ask(sim.id(), &Op::GetSettings).await.unwrap();
        let Answer::Settings(settings) = settings else {
            panic!("{settings:?}");
        };
        assert_eq!((settings.volume, settings.generation), (20, Generation(1)));

        let unlock = owner
            .ask(sim.id(), &Op::SetLock { locked: false })
            .await
            .unwrap();
        assert_eq!(unlock, Answer::Done { generation: None });
        let sync = TimeSync {
            unix_ms: NOW_MS,
            offset_s: -6 * 3600,
            next_change_s: 0,
            next_offset_s: -6 * 3600,
        };
        let synced = owner.ask(sim.id(), &Op::TimeSync(sync)).await.unwrap();
        assert_eq!(synced, Answer::Done { generation: None });
        let status = owner.ask(sim.id(), &Op::Status).await.unwrap();
        let Answer::Status(status) = status else {
            panic!("{status:?}");
        };
        assert_eq!(status.locked, Some(false));
        let synced_at = i64::try_from(NOW_MS).unwrap();
        assert!(
            status
                .clock
                .is_some_and(|clock| clock.as_millisecond() >= synced_at)
        );

        let feed = owner
            .ask(sim.id(), &Op::Feed { portions: 1 })
            .await
            .unwrap();
        assert_eq!(
            feed,
            Answer::Refused(Refusal::Unavailable),
            "no feeding owner yet"
        );
        sim.stop().await;
    });
}

/// An NTP responder on loopback: answers every request with this host's
/// clock until the test ends.
async fn ntp_responder() -> u16 {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    tokio::task::spawn_local(async move {
        let mut request = [0; 64];
        while let Ok((len, from)) = socket.recv_from(&mut request).await {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap();
            let now = u64::try_from(now.as_millis()).unwrap();
            if let Some(answer) = ntp::serve(&request[..len], now, now, 2, 5) {
                let _ = socket.send_to(&answer, from).await;
            }
        }
    });
    port
}

#[test]
fn an_owner_sets_the_zone_and_the_ntp_server_and_the_clock_follows() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();

        let Answer::Time(before) = owner.ask(sim.id(), &Op::GetTime).await.unwrap() else {
            panic!("the time view");
        };
        assert_eq!(before.clock_ms, None, "no clock before a source");
        assert_eq!(before.zone, None);
        assert_eq!(before.ntp, NtpServer::Gateway { port: 123 });

        let port = ntp_responder().await;
        let put = Op::PutTime {
            base: before.generation,
            changes: TimeChanges {
                zone: Some("<+0530>-5:30".to_owned()),
                ntp: Some(NtpServer::Address {
                    ip: std::net::Ipv4Addr::LOCALHOST,
                    port,
                }),
            },
        };
        let put = owner.ask(sim.id(), &put).await.unwrap();
        assert_eq!(
            put,
            Answer::Done {
                generation: Some(Generation(1))
            }
        );

        let mut view = None;
        for _ in 0..50 {
            let Answer::Time(time) = owner.ask(sim.id(), &Op::GetTime).await.unwrap() else {
                panic!("the time view");
            };
            if time.clock_ms.is_some() {
                view = Some(time);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let view = view.expect("the clock set from NTP");
        assert_eq!(view.last, Some(TimeSource::Ntp));
        assert_eq!(view.ntp_last, Some(NtpResult::Used));
        assert_eq!(view.offset_s, Some(5 * 3600 + 1800), "the zone's rule");
        assert!(view.error_ms.is_some_and(|error| error < 1_000));

        // An owner's sync may step the clock past what NTP said.
        let hour_on = TimeSync {
            unix_ms: view.clock_ms.unwrap() + 3_600_000,
            offset_s: 0,
            next_change_s: 0,
            next_offset_s: 0,
        };
        let stepped = owner.ask(sim.id(), &Op::TimeSync(hour_on)).await.unwrap();
        assert_eq!(stepped, Answer::Done { generation: None });
        sim.stop().await;
    });
}

#[test]
fn a_retried_write_gets_its_first_answer_even_after_a_reconnect() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let changes = SettingsPatch {
            sound: Some(false),
            ..SettingsPatch::default()
        };
        let put = Op::PutSettings {
            base: Generation(0),
            changes,
        };
        let id = sim.id();

        let mut first = sim.open(OWNER).await.unwrap();
        let answer = first.ask(id, &put).await.unwrap();
        assert_eq!(
            answer,
            Answer::Done {
                generation: Some(Generation(1))
            }
        );
        drop(first);

        let mut second = sim.open(OWNER).await.unwrap();
        let retried = second.ask(id, &put).await.unwrap();
        assert_eq!(retried, answer, "the same answer, not a stale refusal");
        sim.stop().await;
    });
}

#[test]
fn an_invited_member_joins_and_a_revoke_closes_its_session() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        assert!(matches!(
            sim.open(MEMBER).await,
            Err(ConnectError::UnknownKey)
        ));

        let invite = owner
            .ask(
                sim.id(),
                &Op::CreateInvite {
                    role: ClientRole::Member,
                },
            )
            .await
            .unwrap();
        let Answer::Invite(invite) = invite else {
            panic!("{invite:?}");
        };
        let invite = Invite {
            feeder: sim.identity,
            id: invite.id,
            secret: invite.secret.0,
        };
        let joined = join(&keys(MEMBER), invite, "member", sim.address, [0x91; 32])
            .await
            .unwrap();
        assert_eq!(
            joined,
            Joined::Enrolled {
                role: ClientRole::Member
            }
        );

        let mut member = sim.open(MEMBER).await.unwrap();
        assert_eq!(member.role(), ClientRole::Member);
        let reboot = member.ask(sim.id(), &Op::Reboot).await.unwrap();
        assert_eq!(reboot, Answer::Refused(Refusal::Forbidden));

        let revoke = Op::RevokeClient {
            key: *keys(MEMBER).public(),
        };
        let revoked = owner.ask(sim.id(), &revoke).await.unwrap();
        assert_eq!(revoked, Answer::Done { generation: None });
        assert!(matches!(member.next(WAIT).await, Err(ConnectError::Closed)));
        assert!(matches!(
            sim.open(MEMBER).await,
            Err(ConnectError::UnknownKey)
        ));
        sim.stop().await;
    });
}

#[test]
fn the_flash_keeps_the_identity_clients_and_history_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flash.bin");
    local(async {
        let flash = Flash::file(&path).unwrap();
        let mut sim = Running::start(&flash, true).await;
        let identity = sim.identity;
        let mut owner = sim.open(OWNER).await.unwrap();
        let lock = Op::SetLock { locked: false };
        owner.ask(sim.id(), &lock).await.unwrap();
        drop(owner);
        sim.stop().await;
        drop(flash);

        // Not enrolled again: the client store kept the owner.
        let flash = Flash::file(&path).unwrap();
        let sim = Running::start(&flash, false).await;
        assert_eq!(sim.identity, identity);
        let mut owner = sim.open(OWNER).await.unwrap();
        let history = backlog(&mut owner).await;
        assert!(
            matches!(
                history.as_slice(),
                [
                    EventBody::Boot { .. },
                    EventBody::Lock { locked: false, .. },
                    EventBody::Boot { .. },
                ]
            ),
            "{history:?}"
        );
        sim.stop().await;
    });
}

#[test]
fn a_reboot_drops_the_sessions_and_boots_again() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        // Long enough that the new boot's uptime is surely below it.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let before = status(&mut sim, &mut owner).await;
        let answer = owner.ask(sim.id(), &Op::Reboot).await.unwrap();
        assert_eq!(answer, Answer::Done { generation: None });
        assert!(matches!(owner.next(WAIT).await, Err(ConnectError::Closed)));

        let mut owner = sim.open(OWNER).await.unwrap();
        let after = status(&mut sim, &mut owner).await;
        assert_eq!((before.boot_count, after.boot_count), (1, 2));
        assert!(after.uptime_ms < before.uptime_ms, "uptime starts over");
        assert_eq!(after.locked, Some(true), "the panel boots locked again");
        let boots = backlog(&mut owner)
            .await
            .iter()
            .filter(|body| matches!(body, EventBody::Boot { .. }))
            .count();
        assert_eq!(boots, 2);
        sim.stop().await;
    });
}

#[test]
fn a_fifth_connection_is_turned_away() {
    local(async {
        let sim = Running::start(&Flash::memory(), true).await;
        let mut open = Vec::new();
        for _ in 0..4 {
            open.push(sim.open(OWNER).await.unwrap());
        }
        // Closed at once, its hello unread: the client sees the close or
        // the reset that unread bytes bring.
        let fifth = sim.open(OWNER).await;
        assert!(
            matches!(fifth, Err(ConnectError::Closed | ConnectError::Io(_))),
            "{fifth:?}"
        );
        drop(open);
        sim.stop().await;
    });
}

/// The next panel change the simulator logs.
async fn panel_change(sim: &mut Running) -> (Lock, PanelSource) {
    loop {
        match sim.events.recv().await {
            Some(Event::Sim(record)) => {
                if let Some(PanelEvent::Changed { lock, source }) = record.panel() {
                    return (lock, source);
                }
            }
            Some(_) => {}
            None => panic!("the simulator stopped"),
        }
    }
}

#[test]
fn the_panel_buttons_unlock_and_the_idle_panel_relocks() {
    paused(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        sim.type_line("hold lock");
        assert_eq!(
            panel_change(&mut sim).await,
            (Lock::Unlocked, PanelSource::Button)
        );
        // Stock relocks an idle panel after 30 s.
        assert_eq!(
            panel_change(&mut sim).await,
            (Lock::Locked, PanelSource::Idle)
        );

        let mut owner = sim.open(OWNER).await.unwrap();
        let locks: Vec<EventBody> = backlog(&mut owner)
            .await
            .into_iter()
            .filter(|body| matches!(body, EventBody::Lock { .. }))
            .collect();
        assert_eq!(
            locks,
            [
                EventBody::Lock {
                    locked: false,
                    by: Source::Panel
                },
                EventBody::Lock {
                    locked: true,
                    by: Source::Feeder
                },
            ]
        );
        assert_eq!(status(&mut sim, &mut owner).await.locked, Some(true));
        sim.stop().await;
    });
}

#[test]
fn a_reset_hold_takes_a_new_identity_and_forgets_every_client() {
    local(async {
        // The owner is enrolled at boot only, so the reset sticks.
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        let old = sim.identity;

        // A locked panel refuses the hold; an unlocked one resets, once.
        sim.type_line("hold reset");
        sim.type_line("hold lock");
        sim.type_line("hold reset");
        let (mut asked, mut resets) = (Vec::new(), Vec::new());
        let identity = loop {
            match sim.events.recv().await {
                Some(Event::Started { identity }) => break identity,
                Some(Event::Sim(Record::Keys(result))) => resets.push(result),
                Some(Event::Sim(record)) => {
                    if let Some(PanelEvent::Asked {
                        action: Action::Reset,
                        allowed,
                    }) = record.panel()
                    {
                        asked.push(allowed);
                    }
                }
                Some(_) => {}
                None => panic!("the simulator stopped"),
            }
        };
        // The new identity came only after the second, unlocked hold.
        assert_eq!(asked, [false, true]);
        assert_eq!(resets, [KeyResult::Done], "the writer reset the keys once");
        assert_ne!(identity, old);
        assert!(matches!(
            owner.next(WAIT).await,
            Err(ConnectError::Closed | ConnectError::Io(_))
        ));
        sim.identity = identity;
        assert!(matches!(
            sim.open(OWNER).await,
            Err(ConnectError::UnknownKey)
        ));
        sim.stop().await;
    });
}

#[test]
fn wifi_credentials_are_stored_and_cleared_by_an_owner_only() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        let wifi = Wifi {
            ssid: b"home".to_vec(),
            psk: "correct horse".to_owned(),
        };
        let stored = owner
            .ask(
                sim.id(),
                &Op::PutWifi {
                    base: Generation(0),
                    wifi: Some(wifi),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            stored,
            Answer::Done {
                generation: Some(Generation(1))
            }
        );
        let stale = owner
            .ask(
                sim.id(),
                &Op::PutWifi {
                    base: Generation(0),
                    wifi: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            stale,
            Answer::Refused(Refusal::Stale {
                current: Generation(1)
            })
        );
        let cleared = owner
            .ask(
                sim.id(),
                &Op::PutWifi {
                    base: Generation(1),
                    wifi: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            cleared,
            Answer::Done {
                generation: Some(Generation(2))
            }
        );
        let status = status(&mut sim, &mut owner).await;
        assert_eq!(status.wifi_generation, Generation(2));

        let invite = owner
            .ask(
                sim.id(),
                &Op::CreateInvite {
                    role: ClientRole::Member,
                },
            )
            .await
            .unwrap();
        let Answer::Invite(invite) = invite else {
            panic!("{invite:?}");
        };
        let invite = Invite {
            feeder: sim.identity,
            id: invite.id,
            secret: invite.secret.0,
        };
        join(&keys(MEMBER), invite, "member", sim.address, [0x92; 32])
            .await
            .unwrap();
        let mut member = sim.open(MEMBER).await.unwrap();
        let forbidden = member
            .ask(
                sim.id(),
                &Op::PutWifi {
                    base: Generation(2),
                    wifi: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(forbidden, Answer::Refused(Refusal::Forbidden));

        let history = backlog(&mut owner).await;
        let wifi: Vec<u32> = history
            .iter()
            .filter_map(|body| match body {
                EventBody::Wifi { generation, .. } => Some(generation.0),
                _ => None,
            })
            .collect();
        assert_eq!(wifi, [1, 2]);
        sim.stop().await;
    });
}

#[test]
fn events_that_arrive_while_a_request_waits_are_kept_for_next() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        owner.subscribe(None, false).await.unwrap();
        // Let the backlog (the boot) arrive unread, so the ask reads it
        // ahead of its own answer.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let answer = owner.ask(sim.id(), &Op::Status).await.unwrap();
        assert!(matches!(answer, Answer::Status(_)));
        assert_eq!(owner.queued(), 1);

        let message = owner.next(WAIT).await.unwrap();
        assert!(
            matches!(
                message,
                Pushed::Event(event) if matches!(event.body, EventBody::Boot { .. })
            ),
            "{message:?}"
        );
        assert!(matches!(
            owner.ask(wellfed_connect::KEEPALIVE_ID, &Op::Status).await,
            Err(ConnectError::ReservedId)
        ));
        sim.stop().await;
    });
}

#[test]
fn the_keepalive_holds_a_quiet_session_past_the_idle_limit() {
    paused(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut quiet = sim.open(OWNER).await.unwrap();
        let mut follower = sim.open(OWNER).await.unwrap();
        follower.subscribe(None, false).await.unwrap();

        // The follower waits through the idle limit, keeping itself alive;
        // the quiet session sends nothing and is dropped.
        let limit = Duration::from_millis(wellfed_device_api::IDLE_MS.into());
        let wait = limit + Duration::from_secs(30);
        loop {
            match follower.next(wait).await {
                Ok(_) => {}
                Err(ConnectError::Timeout) => break,
                Err(error) => panic!("the follower was dropped: {error}"),
            }
        }
        let status = follower.ask(sim.id(), &Op::Status).await.unwrap();
        assert!(matches!(status, Answer::Status(_)));
        // It has heard nothing for longer than the silence limit, which says
        // it is gone before it even reads the close.
        assert!(matches!(
            quiet.next(Duration::from_secs(1)).await,
            Err(ConnectError::Silent)
        ));
        sim.stop().await;
    });
}

/// A caller that `select!`s on `next` beside its own work drops the future
/// whenever the other branch wins; the keepalive still goes out and the
/// subscription loses nothing.
#[test]
fn a_feeder_that_goes_silent_without_a_reset_is_noticed() {
    paused(async {
        let sim = Running::start(&Flash::memory(), true).await;
        let relay = harness::Relay::start(sim.address).await;
        let ephemeral = [OWNER[0] ^ 0x80; 32];
        let mut owner = Connection::open(&keys(OWNER), sim.identity, relay.address, ephemeral)
            .await
            .unwrap();
        owner.subscribe(None, false).await.unwrap();
        while owner.next(Duration::from_millis(200)).await.is_ok() {}

        // Power lost: nothing comes back, and no reset says so. Silence
        // counts from the last thing heard, just before the pause.
        relay.pause();
        let started = tokio::time::Instant::now();
        let ended = loop {
            match owner.next(Duration::from_secs(30)).await {
                Err(ConnectError::Timeout) => {}
                other => break other,
            }
        };
        assert!(matches!(ended, Err(ConnectError::Silent)), "{ended:?}");
        let waited = started.elapsed();
        assert!(
            waited + Duration::from_secs(1) >= wellfed_connect::SILENCE
                && waited < 2 * wellfed_connect::SILENCE,
            "{waited:?}"
        );
        sim.stop().await;
    });
}

#[test]
fn a_session_polled_with_dropped_next_futures_stays_alive() {
    paused(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut follower = sim.open(OWNER).await.unwrap();
        follower.subscribe(None, false).await.unwrap();

        let limit = Duration::from_millis(wellfed_device_api::IDLE_MS.into());
        let until = tokio::time::Instant::now() + 2 * limit;
        let mut boots = 0;
        while tokio::time::Instant::now() < until {
            let tick = tokio::time::sleep(Duration::from_millis(700));
            tokio::select! {
                message = follower.next(limit) => {
                    let message = message.expect("the session stays open");
                    if let Pushed::Event(event) = message
                        && matches!(event.body, EventBody::Boot { .. })
                    {
                        boots += 1;
                    }
                }
                () = tick => {}
            }
        }

        assert_eq!(boots, 1, "the backlog arrives once");
        let status = follower.ask(sim.id(), &Op::Status).await.unwrap();
        assert!(matches!(status, Answer::Status(_)));
        sim.stop().await;
    });
}

/// LAN recovery, end to end: the operator unlocks the panel and opens the
/// recovery window at the console, a stranger's key runs the setup
/// exchange over the device API, and the operator keys in its code.
#[test]
fn the_recovery_window_admits_a_new_owner_who_proves_the_code() {
    use wellfed_client::setup::{Ask, Deframer, Session, Step, frame, panel_position};
    use wellfed_model::setup::Reply;
    use wellfed_session::setup_code::Code;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const STRANGER: [u8; 32] = [0x41; 32];

    local(async {
        let sim = Running::start(&Flash::memory(), false).await;
        sim.type_line("hold lock");
        sim.type_line("hold lock+dispense");

        let code = Code::from_bits(0x0bad);
        let ask = Ask {
            label: "stranger".into(),
            wifi: None,
        };
        let mut session = Session::new(&keys(STRANGER), [0x42; 32], ask, code, [0x43; 32]).unwrap();
        let mut stream = tokio::net::TcpStream::connect(sim.address).await.unwrap();
        // The clear opening frame names the recovery session.
        let mut hello = vec![1, 0, 2];
        hello.extend(frame(&session.start().unwrap()).unwrap());
        stream.write_all(&hello).await.unwrap();

        let mut deframer = Deframer::default();
        let mut buffer = [0u8; 1024];
        let reply = 'exchange: loop {
            let n = tokio::time::timeout(WAIT, stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_ne!(n, 0, "the feeder closed the recovery session");
            deframer.push(&buffer[..n]);
            while let Some(message) = deframer.next_message().unwrap() {
                match session.receive(&message).unwrap() {
                    Step::Send(bytes) => stream.write_all(&frame(&bytes).unwrap()).await.unwrap(),
                    Step::Reply(Reply::EnterCode { .. }) => {
                        let presses: Vec<String> = code
                            .presses()
                            .iter()
                            .map(|&b| panel_position(b).to_string())
                            .collect();
                        sim.type_line(&format!("press {}", presses.join(" ")));
                    }
                    Step::Reply(reply) => break 'exchange reply,
                }
            }
        };
        assert!(matches!(reply, Reply::Done { .. }), "{reply:?}");
        assert_eq!(session.feeder_key(), Some(sim.identity));

        let stranger = sim.open(STRANGER).await.unwrap();
        assert_eq!(stranger.role(), ClientRole::Owner);
        sim.stop().await;
    });
}

/// The lines the next batches carry, until one ends with a line `until`
/// accepts.
async fn log_until(connection: &mut Connection, until: impl Fn(&str) -> bool) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let message = connection.next(WAIT).await.unwrap();
        if let Pushed::Log(batch) = message {
            let done = batch.lines.iter().any(|line| until(line));
            lines.extend(batch.lines);
            if done {
                return lines;
            }
        }
    }
}

#[test]
fn an_owner_tails_the_record_lines_the_feeder_logs() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        let tail = owner
            .ask(sim.id(), &Op::TailLog { after: None })
            .await
            .unwrap();
        assert_eq!(tail, Answer::Done { generation: None });

        // From the boot on, as the feeder's UART printed it.
        let lines = log_until(&mut owner, |line| line.contains("op=tail-log")).await;
        assert!(lines[0].starts_with("boot bank="), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("PLAPI1 session slot=0 state=open role=owner")),
            "{lines:?}"
        );

        // A new line follows live.
        let lock = owner
            .ask(sim.id(), &Op::SetLock { locked: false })
            .await
            .unwrap();
        assert_eq!(lock, Answer::Done { generation: None });
        log_until(&mut owner, |line| line.contains("op=set-lock")).await;

        // A member may not.
        let invite = owner
            .ask(
                sim.id(),
                &Op::CreateInvite {
                    role: ClientRole::Member,
                },
            )
            .await
            .unwrap();
        let Answer::Invite(invite) = invite else {
            panic!("{invite:?}");
        };
        let invite = Invite {
            feeder: sim.identity,
            id: invite.id,
            secret: invite.secret.0,
        };
        join(&keys(MEMBER), invite, "member", sim.address, [0x92; 32])
            .await
            .unwrap();
        let mut member = sim.open(MEMBER).await.unwrap();
        let refused = member
            .ask(sim.id(), &Op::TailLog { after: None })
            .await
            .unwrap();
        assert_eq!(refused, Answer::Refused(Refusal::Forbidden));
        sim.stop().await;
    });
}

/// The statistics end to end: each section over the session, and the
/// statistics' metrics to a subscriber every 30 s beside the signal.
#[test]
fn an_owner_reads_the_statistics_and_their_live_metrics() {
    paused(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let mut owner = sim.open(OWNER).await.unwrap();
        assert!(owner.capabilities().stats);

        for section in StatsSection::ALL {
            let answer = owner.ask(sim.id(), &Op::Stats { section }).await.unwrap();
            let Answer::Stats(stats) = answer else {
                panic!("{section:?}: {answer:?}");
            };
            match stats {
                Stats::Counters(counters) => {
                    assert_eq!(counters.boot.sessions, 1, "this session");
                    assert_eq!(counters.lifetime, None);
                }
                Stats::Flash(flash) => {
                    assert_eq!(flash.boot.storage.len(), 32);
                    assert!(flash.events_kept >= 1, "the boot event");
                }
                Stats::Memory(_) | Stats::Power(_) | Stats::Food(_) => {}
            }
        }

        owner.subscribe(None, true).await.unwrap();
        let mut kinds = Vec::new();
        while kinds.len() < 6 {
            match owner.next(Duration::from_secs(60)).await.unwrap() {
                Pushed::Sample(sample) => {
                    if let SampleValue::Wifi { channel, .. } = sample.value {
                        assert_eq!(channel, Some(6));
                    }
                    if let SampleValue::Gateway { round_trip_ms } = sample.value {
                        assert_eq!(round_trip_ms, None, "the simulator probes no gateway");
                    }
                    let kind = sample.value.metric();
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
                Pushed::Event(_) | Pushed::Gap(_) | Pushed::Log(_) => {}
            }
        }
        for kind in MetricKind::ALL {
            assert!(kinds.contains(&kind), "{kind:?} in {kinds:?}");
        }
        sim.stop().await;
    });
}
