//! A simulator running on the current [`tokio::task::LocalSet`], for the
//! tests of every crate that talks to a feeder: this one's, and the
//! server's service and end-to-end tests. Its randomness is a counter, so
//! a test's identities and invite secrets repeat from run to run.

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

use wellfed_core::store::clients::{Label, Role};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::banks::Banks;
use crate::console::{self, ConsoleError};
use crate::device::Random;
use crate::flash::Flash;
use crate::serve::{self, Control, Event, Feeder};

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
                    sim.device().enroll(key, label, Role::Owner)
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

/// A relay in front of a simulator that can stall: while paused it stops
/// moving bytes either way but keeps both sockets open, as a feeder that
/// lost power does (no reset reaches the client). Resuming delivers what
/// waited, in order.
#[derive(Debug)]
pub struct Relay {
    pub address: SocketAddr,
    paused: watch::Sender<bool>,
}

impl Relay {
    pub async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind the relay's port");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        let (paused, watching) = watch::channel(false);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(feeder) = TcpStream::connect(target).await else {
                    continue;
                };
                let (client_in, client_out) = client.into_split();
                let (feeder_in, feeder_out) = feeder.into_split();
                tokio::spawn(relay(client_in, feeder_out, watching.clone()));
                tokio::spawn(relay(feeder_in, client_out, watching.clone()));
            }
        });
        Self { address, paused }
    }

    pub fn pause(&self) {
        self.paused.send_replace(true);
    }

    pub fn resume(&self) {
        self.paused.send_replace(false);
    }
}

/// Copy `from` to `to`, holding off while paused.
async fn relay(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, mut paused: watch::Receiver<bool>) {
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
        if paused.wait_for(|paused| !paused).await.is_err()
            || to.write_all(&buffer[..n]).await.is_err()
        {
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
