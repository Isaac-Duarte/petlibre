//! A simulator running on the current [`tokio::task::LocalSet`], for the
//! tests of every crate that talks to a feeder: this one's, and the
//! server's service and end-to-end tests. Its randomness is a counter, so
//! a test's identities and invite secrets repeat from run to run.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use wellfed_device_api::clients::{Label, Role};

use wellfed_board_plaf103::storage;
use wellfed_feeder::store::traces::{TraceError, TraceRing};
use wellfed_proto::api::TraceSummary;

use crate::banks::Banks;
use crate::console::{self, ConsoleError};
use crate::flash::{Flash, Region};
use crate::platform::Random;
use crate::serve::{self, Control, Event, Feeder};

/// Append a feed's sealed trace to `flash`'s trace ring, as the feeder's
/// settings writer does once the feed is over: the simulator has no motor
/// to trace, but serves what its ring holds.
pub fn store_trace(
    flash: &Rc<Flash>,
    summary: &TraceSummary,
    body: &[u8],
) -> Result<u32, TraceError> {
    let mut ring = TraceRing::default();
    ring.open(
        Region::new(flash, storage::TRACES),
        storage::TRACES.count as usize,
    );
    ring.append(summary, body)
}

/// A running simulator.
#[derive(Debug)]
pub struct Running {
    pub address: SocketAddr,
    /// The feeder's identity key, as its first boot made it.
    pub identity: [u8; 32],
    /// Everything the simulator logs, from its start.
    pub events: mpsc::UnboundedReceiver<Event>,
    control: mpsc::UnboundedSender<Control>,
    done: JoinHandle<()>,
    _alone: Alone,
}

/// The update stage's mailbox and the interlock are process-wide statics,
/// as on the feeder, so simulators on two threads of one process (`cargo
/// test` runs tests on threads) would take each other's requests. A thread
/// starting a simulator holds the process until its last one stops; the
/// simulators of one thread share the hold, so a test may run two.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

thread_local! {
    /// This thread's hold on the process, and how many of its simulators
    /// share it.
    static HOLD: RefCell<Option<(MutexGuard<'static, ()>, usize)>> = const { RefCell::new(None) };
}

/// One simulator's share of its thread's hold on the process.
#[derive(Debug)]
struct Alone;

impl Alone {
    fn take() -> Self {
        HOLD.with_borrow_mut(|hold| match hold {
            Some((_, simulators)) => *simulators += 1,
            None => {
                // A test that failed while holding it stopped its simulator.
                let guard = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
                *hold = Some((guard, 1));
            }
        });
        Self
    }
}

impl Drop for Alone {
    fn drop(&mut self) {
        // At thread exit the hold is already gone with the thread's locals.
        let _ = HOLD.try_with(|hold| {
            let mut hold = hold.borrow_mut();
            if let Some((_, simulators)) = hold.as_mut() {
                *simulators -= 1;
                if *simulators == 0 {
                    *hold = None;
                }
            }
        });
    }
}

impl Running {
    /// The simulator on `flash` at a free localhost port, with `owner`'s
    /// public key enrolled as an owner at every boot when given.
    pub async fn start(flash: &Rc<Flash>, owner: Option<[u8; 32]>) -> Self {
        Self::start_at(flash, owner, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await
    }

    /// As [`Self::start`], at `address`: a restart on the port a client
    /// already knows.
    pub async fn start_at(flash: &Rc<Flash>, owner: Option<[u8; 32]>, address: SocketAddr) -> Self {
        Self::start_with(flash, &Banks::new(), None, owner, address).await
    }

    /// As [`Self::start_at`], on `banks` (kept across starts) and trusting
    /// `update_key` for updates.
    pub async fn start_with(
        flash: &Rc<Flash>,
        banks: &Rc<RefCell<Banks>>,
        update_key: Option<[u8; 32]>,
        owner: Option<[u8; 32]>,
        address: SocketAddr,
    ) -> Self {
        let alone = Alone::take();
        let listener = TcpListener::bind(address)
            .await
            .expect("bind the simulator's port");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        // Each start its own seed, so two simulators in one test have two
        // identities; a restart on the same flash keeps the stored one.
        static SEED: AtomicU8 = AtomicU8::new(0x5a);
        let seed = SEED.fetch_add(1, Ordering::Relaxed);
        let feeder = Feeder {
            flash: Rc::clone(flash),
            banks: Rc::clone(banks),
            update_key,
            random: Box::new(counter(seed)),
            prepare: Box::new(move |sim| match owner {
                Some(key) => {
                    let label = Label::new(b"test-owner").expect("a 10-byte label");
                    sim.platform().enroll(key, label, Role::Owner)
                }
                None => Ok(()),
            }),
        };
        let (control, commands) = mpsc::unbounded_channel();
        let (sink, mut events) = mpsc::unbounded_channel();
        let done = tokio::task::spawn_local(async move {
            serve::serve(feeder, listener, None, commands, |event| {
                let _ = sink.send(event);
            })
            .await
            .expect("the simulator serves until told to quit");
        });

        let identity = loop {
            match events.recv().await {
                Some(Event::Started { identity }) => break identity,
                Some(_) => {}
                None => panic!("the simulator stopped before it started"),
            }
        };
        Self {
            address,
            identity,
            events,
            control,
            done,
            _alone: alone,
        }
    }

    pub fn control(&self, control: Control) {
        // The simulator only stops on `Quit`, which takes `self`.
        let _ = self.control.send(control);
    }

    /// A console line, as the operator would type it.
    pub fn type_line(&self, line: &str) -> Result<(), ConsoleError> {
        for control in console::parse(line)? {
            self.control(control);
        }
        Ok(())
    }

    /// Quit, and wait for the sockets to close.
    pub async fn stop(self) {
        self.control(Control::Quit);
        self.done.await.expect("the simulator task ends cleanly");
    }
}

/// A simulator on a thread of its own, in real time, for a test whose
/// client runs on a multi-threaded runtime or a blocking thread. Its
/// thread holds the process, as any simulator's does, until it is dropped,
/// which stops it.
#[derive(Debug)]
pub struct Threaded {
    pub address: SocketAddr,
    pub identity: [u8; 32],
    control: std::sync::mpsc::Sender<Control>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Threaded {
    /// A fresh simulator with `owner`'s public key enrolled as an owner.
    pub fn start(owner: [u8; 32]) -> Self {
        let (started, starting) = std::sync::mpsc::channel();
        let (control, controls) = std::sync::mpsc::channel::<Control>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime builds");
            runtime.block_on(tokio::task::LocalSet::new().run_until(async move {
                let sim = Running::start(&Flash::memory(), Some(owner)).await;
                let _ = started.send((sim.address, sim.identity));
                loop {
                    match controls.try_recv() {
                        Ok(Control::Quit) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            break;
                        }
                        Ok(other) => sim.control(other),
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    }
                }
                sim.stop().await;
            }));
        });
        let (address, identity) = starting
            .recv()
            .expect("the simulator thread reports its address");

        Self {
            address,
            identity,
            control,
            thread: Some(thread),
        }
    }

    pub fn control(&self, control: Control) {
        let _ = self.control.send(control);
    }
}

impl Drop for Threaded {
    fn drop(&mut self) {
        // Stopped and joined, so its thread's hold is gone before the next
        // test's simulator starts.
        let _ = self.control.send(Control::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A relay in front of a simulator that can stall: while paused it stops
/// moving bytes either way but keeps both sockets open, as a feeder that
/// lost power does (no reset reaches the client). Resuming delivers what
/// waited, in order. It can also hold back only what the feeder sends (an
/// answer lost on its way back), and cut every connection it carries.
#[derive(Debug)]
pub struct Relay {
    pub address: SocketAddr,
    to_feeder: watch::Sender<bool>,
    to_client: Arc<watch::Sender<bool>>,
    cut: watch::Sender<u64>,
    hold: Hold,
}

/// How much more of what clients send passes before the feeder's answers
/// are held back, once [`Relay::hold_answers_after`] set it.
#[derive(Clone, Debug)]
struct Hold {
    left: Arc<Mutex<Option<usize>>>,
    answers: Arc<watch::Sender<bool>>,
}

impl Hold {
    /// Count `n` bytes about to go to the feeder; when they reach the
    /// budget, hold the answers back before they are sent. A client that
    /// waits for each answer before its next request (an install's steps)
    /// had every earlier one answered, so none slips through after.
    fn sending(&self, n: usize) {
        let mut left = self.left.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(budget) = left.as_mut() {
            if n >= *budget {
                *left = None;
                self.answers.send_replace(true);
            } else {
                *budget -= n;
            }
        }
    }
}

impl Relay {
    pub async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind the relay's port");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        let (to_feeder, feeder_paused) = watch::channel(false);
        let (to_client, client_paused) = watch::channel(false);
        let to_client = Arc::new(to_client);
        let (cut, cuts) = watch::channel(0);
        let hold = Hold {
            left: Arc::new(Mutex::new(None)),
            answers: Arc::clone(&to_client),
        };
        let holding = hold.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(feeder) = TcpStream::connect(target).await else {
                    continue;
                };
                let (client_in, client_out) = client.into_split();
                let (feeder_in, feeder_out) = feeder.into_split();
                let forward = relay(
                    client_in,
                    feeder_out,
                    feeder_paused.clone(),
                    Some(holding.clone()),
                );
                let back = relay(feeder_in, client_out, client_paused.clone(), None);
                // Cuts from now on end this connection.
                let mut this = cuts.clone();
                this.mark_unchanged();
                tokio::spawn(until_cut(forward, this.clone()));
                tokio::spawn(until_cut(back, this));
            }
        });
        Self {
            address,
            to_feeder,
            to_client,
            cut,
            hold,
        }
    }

    pub fn pause(&self) {
        self.to_feeder.send_replace(true);
        self.to_client.send_replace(true);
    }

    pub fn resume(&self) {
        self.to_feeder.send_replace(false);
        self.to_client.send_replace(false);
    }

    /// Deliver what the client sends, but hold back what the feeder sends
    /// until [`Self::resume`].
    pub fn hold_answers(&self) {
        self.to_client.send_replace(true);
    }

    /// Deliver `bytes` more of what clients send, then hold back what the
    /// feeder sends until [`Self::resume`]: the request that crosses the
    /// budget reaches the feeder, its answer does not. The relay decides as
    /// the bytes pass, so a test holds a transfer part-way through without
    /// racing it.
    pub fn hold_answers_after(&self, bytes: usize) {
        let mut left = self
            .hold
            .left
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *left = Some(bytes);
    }

    /// Whether the feeder's answers are held back now.
    pub fn holding_answers(&self) -> bool {
        *self.to_client.borrow()
    }

    /// Close every connection the relay carries now, dropping what it held
    /// back; later connections pass as before.
    pub fn cut(&self) {
        self.cut.send_modify(|cuts| *cuts += 1);
    }
}

/// Run `copy` until the relay's connections are cut. A cut wins over
/// bytes waiting to be copied, so what it held back is dropped.
async fn until_cut(copy: impl Future<Output = ()>, mut cuts: watch::Receiver<u64>) {
    tokio::select! {
        biased;
        _ = cuts.changed() => {}
        () = copy => {}
    }
}

/// Copy `from` to `to`, holding off while paused, and counting what goes
/// to the feeder against `hold`'s budget.
async fn relay(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    mut paused: watch::Receiver<bool>,
    hold: Option<Hold>,
) {
    let mut buffer = [0u8; 2048];
    loop {
        if paused.wait_for(|paused| !paused).await.is_err() {
            return;
        }
        let n = tokio::select! {
            read = from.read(&mut buffer) => match read {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            },
            _ = paused.wait_for(|paused| *paused) => continue,
        };
        // A pause that lands now still holds this chunk back.
        if paused.wait_for(|paused| !paused).await.is_err() {
            return;
        }
        if let Some(hold) = &hold {
            hold.sending(n);
        }
        if to.write_all(&buffer[..n]).await.is_err() {
            return;
        }
    }
}

/// A generator that never repeats within a run: identities, ephemerals and
/// invite secrets all come from it. The count sits clear of the bits X25519
/// clamps (byte 0's low three, byte 31's top two), so every output is a
/// distinct key.
pub fn counter(seed: u8) -> impl Fn() -> Random {
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
