//! The simulator over real sockets: a TCP listener for the device API and,
//! optionally, the mDNS responder on UDP 5353, driven like the firmware's
//! loop: every socket call non-blocking, one [`Sim::pass`] per turn.
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

use wellfed_core::buttons::Gesture;
use wellfed_core::mdns::{self, Destination, Responder};
use wellfed_feeder::api::SESSIONS;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

use crate::banks::Banks;
use crate::device::{DeviceError, Random};
use crate::flash::Flash;
use crate::sim::{Record, Sim};

/// The pause between passes while nothing waits, as the loop's poll.
const POLL: Duration = Duration::from_millis(20);
/// The pause while the server has work waiting.
const BUSY: Duration = Duration::from_millis(1);
/// mDNS datagrams read per pass, so a storm of queries costs a bounded turn.
const DATAGRAMS: usize = 4;
/// How long a failed accept pauses the listener.
const RETRY: Duration = Duration::from_secs(2);

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
    let port = listener.local_addr().map_err(ServeError::Listen)?.port();
    let mut mdns = match advertise {
        Some(advertise) => Some(Mdns::open(advertise)?),
        None => None,
    };

    let mut boots = 1;
    let mut booted = Booted::start(&feeder, boots, port, &mut log)?;
    let mut sockets: [Option<TcpStream>; SESSIONS] = Default::default();
    let mut control = Some(control);
    // A failed accept (out of descriptors, say) pauses the listener for
    // a while, as the firmware reopens its own after a delay.
    let mut listen_at = Instant::now();
    let mut delay = Duration::ZERO;

    loop {
        let mut reboot = false;
        tokio::select! {
            biased;
            command = next_control(&mut control) => match command {
                Some(Control::Gesture(gesture)) => {
                    let now = booted.now();
                    booted.sim.gesture(now, gesture, |r| log(Event::Sim(r)));
                }
                Some(Control::Reboot) => {
                    feeder.banks.borrow_mut().power_cut();
                    wellfed_amebad_sys::fake::reset_hardware();
                    reboot = true;
                }
                Some(Control::Quit) => return Ok(()),
                None => control = None,
            },
            accepted = listener.accept(), if Instant::now() >= listen_at => match accepted {
                Ok((stream, _)) => {
                    // Small frames each way; the feeder's stack sends them at once.
                    let _ = stream.set_nodelay(true);
                    let now = booted.now();
                    let server = booted.sim.server();
                    if let Some(slot) = server.accept(now, |r| log(Event::Sim(Record::Server(r)))) {
                        sockets[slot] = Some(stream);
                    }
                }
                Err(error) => {
                    log(Event::Accept(error.kind()));
                    listen_at = Instant::now() + RETRY;
                }
            },
            () = sleep(delay) => {}
        }

        if !reboot {
            let now = booted.now();
            receive(&mut booted.sim, &mut sockets, &mut log);
            let busy = booted.sim.pass(now, |r| log(Event::Sim(r)));
            send(&mut booted.sim, &mut sockets, &mut log);
            booted.identity_changed(port, &mut log);
            if let Some(socket) = &mut mdns {
                socket.step(&mut booted.responder, now);
            }
            delay = if busy { BUSY } else { POLL };
            reboot = booted.sim.take_reboot();
        }
        if reboot {
            sockets = Default::default();
            boots += 1;
            booted = Booted::start(&feeder, boots, port, &mut log)?;
            delay = Duration::ZERO;
        }
    }
}

/// One boot of the feeder: its state, the start its uptime counts from,
/// and its mDNS responder, which starts over with it.
struct Booted {
    sim: Sim,
    at: Instant,
    responder: Responder,
    identity: [u8; 32],
}

impl Booted {
    /// Boot number `count`. The operator's preparation failing (a full
    /// client table, say) is logged; the feeder boots without it.
    fn start(
        feeder: &Feeder,
        count: u32,
        port: u16,
        log: &mut impl FnMut(Event),
    ) -> Result<Self, DeviceError> {
        let mut sim = Sim::open(
            &feeder.flash,
            &feeder.banks,
            feeder.update_key,
            (feeder.random)(),
            0,
            count,
        )?;
        if let Err(error) = (feeder.prepare)(&mut sim) {
            log(Event::Prepare(error));
        }
        let identity = sim.identity();
        log(Event::Started { identity });
        Ok(Self {
            sim,
            at: Instant::now(),
            responder: Responder::new(&identity, port),
            identity,
        })
    }

    /// Milliseconds since this boot, wrapping as the feeder's uptime does.
    fn now(&self) -> u32 {
        let elapsed = self.at.elapsed().as_millis() & u128::from(u32::MAX);
        u32::try_from(elapsed).unwrap_or(u32::MAX)
    }

    /// A Reset hold took a new identity: advertise and report it.
    fn identity_changed(&mut self, port: u16, log: &mut impl FnMut(Event)) {
        let identity = self.sim.identity();
        if identity != self.identity {
            self.identity = identity;
            self.responder = Responder::new(&identity, port);
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

fn receive(
    sim: &mut Sim,
    sockets: &mut [Option<TcpStream>; SESSIONS],
    log: &mut impl FnMut(Event),
) {
    for (slot, socket) in sockets.iter_mut().enumerate() {
        let Some(stream) = socket else {
            continue;
        };
        let room = sim.server().rx(slot);
        if room.is_empty() {
            continue;
        }
        match stream.try_read(room) {
            Ok(0) => close(sim, socket, slot, log),
            Ok(n) => sim.server().received(slot, n),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => close(sim, socket, slot, log),
        }
    }
}

fn send(sim: &mut Sim, sockets: &mut [Option<TcpStream>; SESSIONS], log: &mut impl FnMut(Event)) {
    for (slot, socket) in sockets.iter_mut().enumerate() {
        let Some(stream) = socket else {
            continue;
        };
        let pending = sim.server().tx(slot);
        if !pending.is_empty() {
            match stream.try_write(pending) {
                Ok(n) => sim.server().sent(slot, n),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    close(sim, socket, slot, log);
                    continue;
                }
            }
        }
        if sim.server().closing(slot) {
            close(sim, socket, slot, log);
        }
    }
}

fn close(sim: &mut Sim, socket: &mut Option<TcpStream>, slot: usize, log: &mut impl FnMut(Event)) {
    *socket = None;
    sim.server()
        .closed(slot, |r| log(Event::Sim(Record::Server(r))));
}

/// The mDNS socket, shared with any other responder on the host.
struct Mdns {
    socket: UdpSocket,
    ip: Ipv4Addr,
    packet: [u8; mdns::PACKET],
    reply: [u8; mdns::PACKET],
}

impl Mdns {
    fn open(advertise: Advertise) -> Result<Self, ServeError> {
        use socket2::{Domain, Protocol, Socket, Type};

        let open = || -> io::Result<UdpSocket> {
            let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
            // Avahi or another responder may hold the port already.
            socket.set_reuse_address(true)?;
            socket.set_reuse_port(true)?;
            socket.set_nonblocking(true)?;
            let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, mdns::PORT);
            socket.bind(&SocketAddr::V4(any).into())?;
            socket.join_multicast_v4(&group(), &advertise.ip)?;
            socket.set_multicast_if_v4(&advertise.ip)?;
            UdpSocket::from_std(socket.into())
        };
        Ok(Self {
            socket: open().map_err(ServeError::Mdns)?,
            ip: advertise.ip,
            packet: [0; mdns::PACKET],
            reply: [0; mdns::PACKET],
        })
    }

    /// Announce when due and answer what arrived. A failed send is only a
    /// lost answer: the asker asks again.
    fn step(&mut self, responder: &mut Responder, now: u32) {
        responder.set_ip(Some(u32::from(self.ip)), now);
        let to_group = SocketAddr::V4(SocketAddrV4::new(group(), mdns::PORT));
        if let Some(len) = responder.announce(now, &mut self.reply) {
            let _ = self.socket.try_send_to(&self.reply[..len], to_group);
        }
        for _ in 0..DATAGRAMS {
            let Ok((len, from)) = self.socket.try_recv_from(&mut self.packet) else {
                return;
            };
            let Some(reply) = responder.query(&self.packet[..len], from.port(), &mut self.reply)
            else {
                continue;
            };
            let to = match reply.to {
                Destination::Multicast => to_group,
                Destination::Unicast => from,
            };
            let _ = self.socket.try_send_to(&self.reply[..reply.len], to);
        }
    }
}

fn group() -> Ipv4Addr {
    Ipv4Addr::from(mdns::GROUP)
}
