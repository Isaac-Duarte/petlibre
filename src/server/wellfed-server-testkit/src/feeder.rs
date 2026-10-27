//! The simulated feeder, over real TCP, and what its owner does to it: the
//! feeder's own policy and stores answer, never a mock.

use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

use wellfed_client::api::Invite;
use wellfed_connect::Connection;
pub use wellfed_feeder_sim::flash::Flash;
pub use wellfed_feeder_sim::harness::{Relay, Running};
pub use wellfed_feeder_sim::serve::Control;
use wellfed_model::{Answer, ClientRole, Op};
use wellfed_session::noise::Keypair;

/// The owner's secret key, enrolled on every simulator the kit starts.
pub const OWNER: [u8; 32] = [0x21; 32];

/// The owner's keypair.
pub fn owner() -> Keypair {
    Keypair::from_secret(OWNER)
}

/// A simulator on fresh flash, at a free localhost port, with the owner
/// enrolled.
pub async fn start() -> Running {
    start_on(&Flash::memory()).await
}

/// A simulator on `flash`, with the owner enrolled: a second start on the
/// same flash is the same feeder after a restart.
pub async fn start_on(flash: &Rc<Flash>) -> Running {
    Running::start(flash, Some(*owner().public())).await
}

/// As [`start_on`], at `address`: the feeder back on the port the server
/// already knows.
pub async fn restart(flash: &Rc<Flash>, address: SocketAddr) -> Running {
    Running::start_at(flash, Some(*owner().public()), address).await
}

/// How many feeds the simulator on `flash` has ever reserved in its feed
/// journal: the count a test that retries a feed proves is one.
pub fn feeds(flash: &Rc<Flash>) -> u64 {
    wellfed_feeder_sim::feeding::reserved(flash).unwrap_or(0)
}

/// An owner's session with the simulator.
pub async fn open_owner(sim: &Running) -> Connection {
    Connection::open(&owner(), sim.identity, sim.address, [0x99; 32])
        .await
        .expect("the owner opens a session with the simulator")
}

/// An invite for `role`, as the owner's CLI makes it. The feeder answers a
/// repeated write id from memory, so each invite takes a new one.
pub async fn invite(sim: &Running, role: ClientRole) -> String {
    static IDS: AtomicU8 = AtomicU8::new(0x80);
    let id = [IDS.fetch_add(1, Ordering::Relaxed); 16];

    let mut connection = open_owner(sim).await;
    let answer = connection
        .ask(id, &Op::CreateInvite { role })
        .await
        .expect("the simulator answers an invite request");
    let Answer::Invite(invite) = answer else {
        panic!("an invite request answered {answer:?}");
    };

    Invite {
        feeder: sim.identity,
        id: invite.id,
        secret: invite.secret.0,
    }
    .text()
}
