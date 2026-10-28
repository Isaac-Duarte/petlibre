//! The simulator over real sockets: a TCP listener for the device API and,
//! optionally, an mDNS socket on UDP 5353, driven like the firmware's loop:
//! the loop's own driver makes every socket call, non-blocking, once per
//! [`Sim::pass`]. This task awaits only what a pass cannot poll: the
//! operator's controls and the listener's connections, which it hands to
//! the pass.
//!
//! One task owns everything, as the loop task does, so the sans-IO server
//! is never shared. Time comes from tokio's clock, which a test can pause
//! and advance to run the idle and handshake timeouts in virtual time.
//!
//! A `Reboot` request drops every connection and opens the feeder again
//! from its flash: sessions, the panel, pairing and the clock start over,
//! and the stores carry on, as after a reset.

use std::cell::RefCell;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;
use std::time::Duration;

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};
use wellfed_core::buttons::Gesture;
use wellfed_core::mdns;

use crate::banks::Banks;
use crate::flash::Flash;
use crate::platform::{DeviceError, Random, SimSupply};
use crate::sim::{Record, Sim};

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error("read the listener's address: {0}")]
    Listen(io::Error),
    #[error("open the mDNS socket: {0}")]
    Mdns(io::Error),
}

/// What the operator does to the simulated feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A gesture from the buttons.
    Gesture(Gesture),
    /// Restart, as a power cycle: KM0's trial count and the update-attempt
    /// bit are lost with the backup registers.
    Reboot,
    /// Hold a feed in flight where it is (`true`), as a motor that stopped
    /// moving, or let it go on. A restart lets go.
    StallFeeds(bool),
    /// The adapter fitted (`true`) or pulled, from now on and across
    /// restarts.
    Adapter(bool),
    /// The battery channel's pin millivolts from now on and across
    /// restarts; `None` takes the cells out.
    Pack(Option<u32>),
    /// Fill the hopper (`true`) or empty it, as the next feed's food beam
    /// reads it. A restart fills it again.
    Hopper(bool),
    /// Stop serving.
    Quit,
}

/// Where the feeder advertises itself, and as which address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Advertise {
    /// The address the A record gives: the interface clients reach.
    pub ip: Ipv4Addr,
}

/// Something that happened, for the operator's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Sim(Record),
    /// The feeder booted, or took a new identity, with this identity.
    Started {
        identity: [u8; 32],
    },
    /// The boot's preparation failed; the feeder runs without it.
    Prepare(DeviceError),
    /// Accepting a connection failed; the listener pauses.
    Accept(io::ErrorKind),
}

/// Run on each boot's feeder before its first pass.
pub type Prepare = Box<dyn Fn(&mut Sim) -> Result<(), DeviceError>>;

/// How to build the feeder each time it boots.
pub struct Feeder {
    pub flash: Rc<Flash>,
    /// The two banks, kept across restarts.
    pub banks: Rc<RefCell<Banks>>,
    /// The update-signing key built into the simulated firmware.
    pub update_key: Option<[u8; 32]>,
    /// A fresh generator for each boot.
    pub random: Box<dyn Fn() -> Random>,
    /// Applied to each boot's device before the first pass: the operator's
    /// enrolled owner, for one.
    pub prepare: Prepare,
}

/// Serve the feeder on `listener` until a [`Control::Quit`]. Once
/// `control` closes, the feeder runs on with no one at its buttons.
pub async fn serve(
    feeder: Feeder,
    listener: TcpListener,
    advertise: Option<Advertise>,
    control: mpsc::UnboundedReceiver<Control>,
    mut log: impl FnMut(Event),
) -> Result<(), ServeError> {
    // Checked up front, so a listener that cannot say where it is fails
    // the start rather than the first client.
    listener.local_addr().map_err(ServeError::Listen)?;

    let mut boots = 1;
    let mut supply = SimSupply::MAINS;
    let mut booted = Booted::start(&feeder, boots, advertise, &mut log)?;
    let mut control = Some(control);
    let mut delay = Duration::ZERO;

    loop {
        let mut reboot = false;
        tokio::select! {
            biased;
            command = next_control(&mut control) => match command {
                Some(Control::Gesture(gesture)) => booted.sim.gesture(gesture),
                Some(Control::Reboot) => {
                    feeder.banks.borrow_mut().power_cut();
                    wellfed_amebad_sys::fake::reset_hardware();
                    reboot = true;
                }
                Some(Control::StallFeeds(stalled)) => booted.sim.stall_feeds(stalled),
                Some(Control::Adapter(adapter)) => {
                    supply.adapter = adapter;
                    booted.sim.platform().set_supply(supply);
                }
                Some(Control::Pack(pin_mv)) => {
                    supply.pin_mv = pin_mv;
                    booted.sim.platform().set_supply(supply);
                }
                Some(Control::Hopper(food)) => booted.sim.fill_hopper(food),
                Some(Control::Quit) => return Ok(()),
                None => control = None,
            },
            // The loop's driver opens the listener, and closes it for a
            // while after a failed accept, as the firmware reopens its own.
            accepted = listener.accept(), if booted.sim.listening() => match accepted {
                Ok((stream, _)) => {
                    // Small frames each way; the feeder's stack sends them at once.
                    let _ = stream.set_nodelay(true);
                    booted.sim.accepted(stream);
                }
                Err(error) => {
                    log(Event::Accept(error.kind()));
                    booted.sim.accept_failed();
                }
            },
            () = sleep(delay) => {}
        }

        if !reboot {
            let now = booted.now();
            let next = booted.sim.pass(now, |r| log(Event::Sim(r)));
            booted.identity_changed(&mut log);
            delay = Duration::from_millis(u64::from(next));
            reboot = booted.sim.take_reboot();
        }
        if reboot {
            // The connections close with the boot's platform.
            boots += 1;
            booted = Booted::start(&feeder, boots, advertise, &mut log)?;
            booted.sim.platform().set_supply(supply);
            delay = Duration::ZERO;
        }
    }
}

/// One boot of the feeder: its state and the start its uptime counts from.
struct Booted {
    sim: Sim,
    at: Instant,
    identity: [u8; 32],
}

impl Booted {
    /// Boot number `count`. The operator's preparation failing (a full
    /// client table, say) is logged; the feeder boots without it.
    fn start(
        feeder: &Feeder,
        count: u32,
        advertise: Option<Advertise>,
        log: &mut impl FnMut(Event),
    ) -> Result<Self, ServeError> {
        let mdns = match advertise {
            Some(advertise) => Some((open_mdns(advertise)?, advertise.ip)),
            None => None,
        };
        let mut sim = Sim::open(
            &feeder.flash,
            &feeder.banks,
            feeder.update_key,
            (feeder.random)(),
            0,
            count,
            mdns,
        )?;
        if let Err(error) = (feeder.prepare)(&mut sim) {
            log(Event::Prepare(error));
        }
        let identity = sim.identity()?;
        log(Event::Started { identity });
        Ok(Self {
            sim,
            at: Instant::now(),
            identity,
        })
    }

    /// Milliseconds since this boot, wrapping as the feeder's uptime does.
    fn now(&self) -> u32 {
        let elapsed = self.at.elapsed().as_millis() & u128::from(u32::MAX);
        u32::try_from(elapsed).unwrap_or(u32::MAX)
    }

    /// A Reset hold took a new identity: report it.
    fn identity_changed(&mut self, log: &mut impl FnMut(Event)) {
        let Ok(identity) = self.sim.identity() else {
            return;
        };
        if identity != self.identity {
            self.identity = identity;
            log(Event::Started { identity });
        }
    }
}

/// The next control, or never once the channel has closed.
async fn next_control(control: &mut Option<mpsc::UnboundedReceiver<Control>>) -> Option<Control> {
    match control {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// The mDNS socket, shared with any other responder on the host.
fn open_mdns(advertise: Advertise) -> Result<UdpSocket, ServeError> {
    use socket2::{Domain, Protocol, Socket, Type};

    let open = || -> io::Result<UdpSocket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        // Avahi or another responder may hold the port already.
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, mdns::PORT);
        socket.bind(&SocketAddr::V4(any).into())?;
        socket.join_multicast_v4(&Ipv4Addr::from(mdns::GROUP), &advertise.ip)?;
        socket.set_multicast_if_v4(&advertise.ip)?;
        UdpSocket::from_std(socket.into())
    };
    open().map_err(ServeError::Mdns)
}
