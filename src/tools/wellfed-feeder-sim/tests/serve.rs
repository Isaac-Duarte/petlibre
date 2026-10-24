//! The simulated feeder over real TCP on localhost, driven by
//! `wellfed-connect` as the server and the CLI drive a feeder.

use std::cell::Cell;
use std::future::Future;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::Duration;

use wellfed_client::api::Invite;
use wellfed_connect::{Connection, Error as ConnectError, join};
use wellfed_core::noise::Keypair;
use wellfed_core::store::clients::{Label, Role as StoreRole};
use wellfed_feeder::panel::{Action, Event as PanelEvent, Lock, Source as PanelSource};
use wellfed_feeder_sim::console;
use wellfed_feeder_sim::device::Random;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::serve::{self, Control, Event, Feeder};
use wellfed_feeder_sim::sim::{Record, Reset};
use wellfed_proto::api::{
    EventBody, FeederMessage, Joined, Op, Outcome, Refusal, RequestId, Role, SettingsChanges,
    Source, Status, TimeSync, Wifi,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const OWNER: [u8; 32] = [0x21; 32];
const MEMBER: [u8; 32] = [0x31; 32];
const WAIT: Duration = Duration::from_secs(5);

/// A September 2026 instant, inside the clock's accepted range.
const NOW_MS: u64 = 1_790_000_000_000;

fn keys(secret: [u8; 32]) -> Keypair {
    Keypair::from_secret(secret)
}

/// A generator that never repeats within a test: identities, ephemerals
/// and invite secrets all come from it. The count sits clear of the bits
/// X25519 clamps (byte 0's low three, byte 31's top two), so every output
/// is a distinct key.
fn counter(seed: u8) -> impl Fn() -> Random {
    let next = Rc::new(Cell::new(0u32));
    move || {
        let next = Rc::clone(&next);
        Box::new(move || {
            let n = next.get().wrapping_add(1);
            next.set(n);
            let mut out = [seed; 32];
            out[8..12].copy_from_slice(&n.to_le_bytes());
            Some(out)
        })
    }
}

/// A running simulator.
struct Running {
    address: SocketAddr,
    identity: [u8; 32],
    control: mpsc::UnboundedSender<Control>,
    events: mpsc::UnboundedReceiver<Event>,
    done: tokio::task::JoinHandle<()>,
    ids: u8,
}

impl Running {
    /// The simulator on `flash`, enrolling the test's owner unless
    /// `enroll` is false.
    async fn start(flash: &Rc<Flash>, enroll: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let feeder = Feeder {
            flash: Rc::clone(flash),
            random: Box::new(counter(0x5a)),
            prepare: Box::new(move |sim| {
                if enroll {
                    let label = Label::new(b"test-owner").unwrap();
                    sim.device()
                        .enroll(*keys(OWNER).public(), label, StoreRole::Owner)?;
                }
                Ok(())
            }),
        };
        let (control, commands) = mpsc::unbounded_channel();
        let (sink, mut events) = mpsc::unbounded_channel();
        let done = tokio::task::spawn_local(async move {
            serve::serve(feeder, listener, None, commands, |event| {
                let _ = sink.send(event);
            })
            .await
            .unwrap();
        });
        let identity = loop {
            if let Some(Event::Started { identity }) = events.recv().await {
                break identity;
            }
        };
        Self {
            address,
            identity,
            control,
            events,
            done,
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
        for control in console::parse(line).unwrap() {
            self.control.send(control).unwrap();
        }
    }

    async fn stop(self) {
        self.control.send(Control::Quit).unwrap();
        self.done.await.unwrap();
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

async fn status(sim: &mut Running, connection: &mut Connection) -> Status<'static> {
    let answer = connection.ask(sim.id(), Op::Status).await.unwrap();
    let Outcome::Status(status) = answer.outcome() else {
        panic!("{answer:?}");
    };
    Status {
        build: "",
        ..status
    }
}

/// Every event the subscription replays, until it is quiet.
async fn backlog(connection: &mut Connection) -> Vec<EventBody> {
    connection.subscribe(None, false).await.unwrap();
    let mut events = Vec::new();
    while let Ok(message) = connection.next(Duration::from_millis(500)).await {
        if let FeederMessage::Event(event) = message.get() {
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
        assert_eq!(owner.role(), Role::Owner);

        let status = owner.ask(sim.id(), Op::Status).await.unwrap();
        let Outcome::Status(status) = status.outcome() else {
            panic!("{status:?}");
        };
        assert_eq!(status.locked, Some(true), "the panel boots locked");
        assert_eq!(status.clock_ms, None, "no clock before a sync");

        let changes = SettingsChanges {
            volume: Some(20),
            ..SettingsChanges::default()
        };
        let put = Op::PutSettings { base: 0, changes };
        let done = owner.ask(sim.id(), put).await.unwrap();
        assert_eq!(
            done.outcome(),
            Outcome::Done {
                generation: Some(1)
            }
        );
        let stale = owner.ask(sim.id(), put).await.unwrap();
        assert_eq!(
            stale.outcome(),
            Outcome::Refused(Refusal::Stale { current: 1 })
        );
        let settings = owner.ask(sim.id(), Op::GetSettings).await.unwrap();
        let Outcome::Settings(settings) = settings.outcome() else {
            panic!("{settings:?}");
        };
        assert_eq!((settings.volume, settings.generation), (20, 1));

        let unlock = owner
            .ask(sim.id(), Op::SetLock { locked: false })
            .await
            .unwrap();
        assert_eq!(unlock.outcome(), Outcome::Done { generation: None });
        let sync = TimeSync {
            unix_ms: NOW_MS,
            offset_s: -6 * 3600,
            next_change_s: 0,
            next_offset_s: -6 * 3600,
        };
        let synced = owner.ask(sim.id(), Op::TimeSync(sync)).await.unwrap();
        assert_eq!(synced.outcome(), Outcome::Done { generation: None });
        let status = owner.ask(sim.id(), Op::Status).await.unwrap();
        let Outcome::Status(status) = status.outcome() else {
            panic!("{status:?}");
        };
        assert_eq!(status.locked, Some(false));
        assert!(status.clock_ms.is_some_and(|ms| ms >= NOW_MS));

        let feed = owner.ask(sim.id(), Op::Feed { portions: 1 }).await.unwrap();
        assert_eq!(
            feed.outcome(),
            Outcome::Refused(Refusal::Unavailable),
            "no feeding owner yet"
        );
        sim.stop().await;
    });
}

#[test]
fn a_retried_write_gets_its_first_answer_even_after_a_reconnect() {
    local(async {
        let mut sim = Running::start(&Flash::memory(), true).await;
        let changes = SettingsChanges {
            sound: Some(false),
            ..SettingsChanges::default()
        };
        let put = Op::PutSettings { base: 0, changes };
        let id = sim.id();

        let mut first = sim.open(OWNER).await.unwrap();
        let answer = first.ask(id, put).await.unwrap();
        assert_eq!(
            answer.outcome(),
            Outcome::Done {
                generation: Some(1)
            }
        );
        drop(first);

        let mut second = sim.open(OWNER).await.unwrap();
        let retried = second.ask(id, put).await.unwrap();
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
            .ask(sim.id(), Op::CreateInvite { role: Role::Member })
            .await
            .unwrap();
        let Outcome::Invite(invite) = invite.outcome() else {
            panic!("{invite:?}");
        };
        let invite = Invite {
            feeder: sim.identity,
            id: invite.id,
            secret: invite.secret,
        };
        let joined = join(&keys(MEMBER), invite, "member", sim.address, [0x91; 32])
            .await
            .unwrap();
        assert_eq!(joined, Joined::Enrolled { role: Role::Member });

        let mut member = sim.open(MEMBER).await.unwrap();
        assert_eq!(member.role(), Role::Member);
        let reboot = member.ask(sim.id(), Op::Reboot).await.unwrap();
        assert_eq!(reboot.outcome(), Outcome::Refused(Refusal::Forbidden));

        let revoke = Op::RevokeClient {
            key: *keys(MEMBER).public(),
        };
        let revoked = owner.ask(sim.id(), revoke).await.unwrap();
        assert_eq!(revoked.outcome(), Outcome::Done { generation: None });
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
        owner.ask(sim.id(), lock).await.unwrap();
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
        let answer = owner.ask(sim.id(), Op::Reboot).await.unwrap();
        assert_eq!(answer.outcome(), Outcome::Done { generation: None });
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
            Some(Event::Sim(Record::Panel(PanelEvent::Changed { lock, source }))) => {
                return (lock, source);
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
                Some(Event::Sim(Record::Panel(PanelEvent::Asked {
                    action: Action::Reset,
                    allowed,
                }))) => asked.push(allowed),
                Some(Event::Sim(Record::Reset(reset))) => resets.push(reset),
                Some(_) => {}
                None => panic!("the simulator stopped"),
            }
        };
        // The new identity came only after the second, unlocked hold.
        assert_eq!(asked, [false, true]);
        assert_eq!(resets, [Reset::Done]);
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
            ssid: b"home",
            psk: "correct horse",
        };
        let stored = owner
            .ask(
                sim.id(),
                Op::PutWifi {
                    base: 0,
                    wifi: Some(wifi),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            stored.outcome(),
            Outcome::Done {
                generation: Some(1)
            }
        );
        let stale = owner
            .ask(
                sim.id(),
                Op::PutWifi {
                    base: 0,
                    wifi: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            stale.outcome(),
            Outcome::Refused(Refusal::Stale { current: 1 })
        );
        let cleared = owner
            .ask(
                sim.id(),
                Op::PutWifi {
                    base: 1,
                    wifi: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            cleared.outcome(),
            Outcome::Done {
                generation: Some(2)
            }
        );
        let status = status(&mut sim, &mut owner).await;
        assert_eq!(status.wifi_generation, 2);

        let invite = owner
            .ask(sim.id(), Op::CreateInvite { role: Role::Member })
            .await
            .unwrap();
        let Outcome::Invite(invite) = invite.outcome() else {
            panic!("{invite:?}");
        };
        let invite = Invite {
            feeder: sim.identity,
            id: invite.id,
            secret: invite.secret,
        };
        join(&keys(MEMBER), invite, "member", sim.address, [0x92; 32])
            .await
            .unwrap();
        let mut member = sim.open(MEMBER).await.unwrap();
        let forbidden = member
            .ask(
                sim.id(),
                Op::PutWifi {
                    base: 2,
                    wifi: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(forbidden.outcome(), Outcome::Refused(Refusal::Forbidden));

        let history = backlog(&mut owner).await;
        let wifi: Vec<u32> = history
            .iter()
            .filter_map(|body| match body {
                EventBody::Wifi { generation, .. } => Some(*generation),
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
        let answer = owner.ask(sim.id(), Op::Status).await.unwrap();
        assert!(matches!(answer.outcome(), Outcome::Status(_)));
        assert_eq!(owner.queued(), 1);

        let message = owner.next(WAIT).await.unwrap();
        assert!(
            matches!(
                message.get(),
                FeederMessage::Event(event) if matches!(event.body, EventBody::Boot { .. })
            ),
            "{message:?}"
        );
        assert!(matches!(
            owner
                .request(wellfed_connect::KEEPALIVE_ID, Op::Status)
                .await,
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
        let limit = Duration::from_millis(wellfed_feeder::api::IDLE_MS.into());
        let wait = limit + Duration::from_secs(30);
        loop {
            match follower.next(wait).await {
                Ok(_) => {}
                Err(ConnectError::Timeout) => break,
                Err(error) => panic!("the follower was dropped: {error}"),
            }
        }
        let status = follower.ask(sim.id(), Op::Status).await.unwrap();
        assert!(matches!(status.outcome(), Outcome::Status(_)));
        // Its keepalive goes out to a closed socket, so the read finds the
        // close or the reset.
        assert!(matches!(
            quiet.next(Duration::from_secs(1)).await,
            Err(ConnectError::Closed | ConnectError::Io(_))
        ));
        sim.stop().await;
    });
}

/// LAN recovery, end to end: the operator unlocks the panel and opens the
/// recovery window at the console, a stranger's key runs the setup
/// exchange over the device API, and the operator keys in its code.
#[test]
fn the_recovery_window_admits_a_new_owner_who_proves_the_code() {
    use wellfed_client::setup::{Ask, Deframer, Session, Step, frame, panel_position};
    use wellfed_core::setup_code::Code;
    use wellfed_proto::setup::Reply;
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
        assert_eq!(stranger.role(), Role::Owner);
        sim.stop().await;
    });
}
