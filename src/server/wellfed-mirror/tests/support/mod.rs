//! What the mirror's tests share: a mirror on a temporary store with one
//! simulated feeder enrolled and online.

use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{ClientRole, Connection as Link, FeederId};
use wellfed_mirror::{Config, Discovery, Feeder, Invite, Mirror, MirrorEvent, Timing};
use wellfed_server_testkit::WAIT;
use wellfed_server_testkit::feeder::{self, Running};
use wellfed_server_testkit::fixed::{FixedClock, SeededRandom};
use wellfed_store::{NewFeeder, Refresh, SealedKey, Store};
use tokio::sync::broadcast::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const ID: FeederId = FeederId(Uuid::from_u128(7));

fn config(answer: Duration) -> Config {
    Config {
        discovery: Discovery::Off,
        timing: Timing {
            retry_min: Duration::from_millis(50),
            retry_max: Duration::from_millis(400),
            time_sync: Duration::from_secs(3600),
            answer,
        },
    }
}

/// A mirror on `store`, with `sim` enrolled (reached at `address`) and
/// started; its updates from the first, once it is online.
pub async fn mirrored(
    store: &Store,
    sim: &Running,
    address: &str,
    answer: Duration,
) -> (Mirror, Receiver<MirrorEvent>) {
    let mirror = Mirror::new(
        store.clone(),
        config(answer),
        Arc::new(FixedClock),
        Arc::new(SeededRandom::fresh()),
        CancellationToken::new(),
    );
    let fresh = Refresh::Rebuilt {
        events: 0,
        undecoded: 0,
    };
    assert_eq!(mirror.refresh_projections().await.unwrap(), fresh);
    assert_eq!(
        mirror.refresh_projections().await.unwrap(),
        Refresh::Current
    );
    let invite = feeder::invite(sim, ClientRole::Server).await;
    let invite = Invite::parse(&invite).expect("the kit's invite parses");
    let identity = invite.feeder;
    let at = mirror.locate(identity, Some(address)).await.unwrap();
    let secret = [0x33; 32];
    let enrolled = mirror.enroll(invite, "test", at, secret).await.unwrap();
    assert_eq!(enrolled.role, ClientRole::Server);

    // The mirror reads the cursor from the feeder's row; the key it uses
    // is the one it is given, so the stored one is a stand-in.
    store
        .insert_feeder(NewFeeder {
            id: ID,
            identity,
            name: "kitchen",
            role: enrolled.role,
            address: Some(address),
            key: SealedKey {
                nonce: [0; 12],
                sealed: Vec::new(),
            },
            created: FixedClock.now(),
        })
        .await
        .unwrap();
    let mut updates = mirror
        .start(Feeder {
            id: ID,
            identity,
            secret,
            address: Some(address.into()),
        })
        .await;

    let mut links = Vec::new();
    while links.last() != Some(&Link::Online) {
        if let MirrorEvent::Link(link) = next(&mut updates).await {
            links.push(link);
        }
    }
    assert_eq!(links, [Link::Connecting, Link::Online]);
    (mirror, updates)
}

pub async fn next(updates: &mut Receiver<MirrorEvent>) -> MirrorEvent {
    tokio::time::timeout(WAIT, updates.recv())
        .await
        .expect("an update in time")
        .expect("the broadcast is open")
}
