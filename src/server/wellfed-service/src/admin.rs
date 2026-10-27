//! What an owner administers on a feeder through the server: its Wi-Fi, a
//! restart and removing it; and a tune, which a member may play too. Each
//! goes through the journal. The feeder lets only an owner's key change
//! Wi-Fi or restart it, so a server adopted with the `server` role is
//! refused `forbidden` there.

use wellfed_api::{
    CommandKind, Connection as Link, FeederId, Generation, KeyRemoval, PlayRequest, Removal,
    RevokeAttempt, TuneName, WifiChange,
};
use wellfed_mirror::{MirrorEvent, hmac_sha256};
use wellfed_model::{Op, Tune, Wifi};
use serde::Serialize;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use crate::commands::done;
use crate::journal::Write;
use crate::phones::own_revoke;
use crate::{Caller, IdempotencyKey, Invalid, Result, Service};

/// A Wi-Fi change as it is hashed into the journal. The passphrase goes
/// in only as an HMAC under a subkey of the server's key, so the journal
/// (which an export carries) cannot be searched for a weak one.
#[derive(Serialize)]
struct WifiBody {
    base: Generation,
    ssid: Option<String>,
    psk: Option<String>,
}

/// A request with nothing in it but its route, as hashed.
#[derive(Serialize)]
struct Empty {}

impl Service {
    /// Store Wi-Fi credentials for the feeder's next join, or clear them,
    /// if its Wi-Fi store is still at `base` (the status's
    /// `wifi_generation`). A stale base is refused `stale` by the feeder.
    ///
    /// A change whose answer was lost is resent under its write id, which
    /// the feeder remembers only until it restarts: a resend after a
    /// restart finds the generation already moved by the change itself, so
    /// it is recorded `stale` though the change ran. The status's
    /// `wifi_generation` says which.
    pub async fn put_wifi(
        &self,
        caller: &Caller,
        id: FeederId,
        base: Generation,
        change: WifiChange,
        key: Option<IdempotencyKey>,
    ) -> Result<()> {
        if let Some(wifi) = &change.wifi {
            if !(1..=32).contains(&wifi.ssid.len()) {
                return Err(Invalid::Ssid.into());
            }
            let printable = wifi.psk.bytes().all(|byte| (0x20..=0x7e).contains(&byte));
            if !(8..=63).contains(&wifi.psk.len()) || !printable {
                return Err(Invalid::Psk.into());
            }
        }
        self.live_row(id).await?;

        let body = WifiBody {
            base,
            ssid: change.wifi.as_ref().map(|wifi| wifi.ssid.clone()),
            psk: change.wifi.as_ref().map(|wifi| {
                // A subkey of its own, so this digest never shares a key
                // with the sealing of the feeder keys.
                let subkey = hmac_sha256(&self.shared.config.key, &[b"wellfed-server wifi v1"]);
                let digest = hmac_sha256(&subkey, &[wifi.psk.as_bytes()]);
                wellfed_formats::hex::encode(&digest)
            }),
        };
        let wifi = change.wifi.map(|wifi| Wifi {
            ssid: wifi.ssid.into_bytes(),
            psk: wifi.psk,
        });
        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Wifi,
                body: &body,
                key,
                op: Op::PutWifi { base, wifi },
            })
            .await?;
        done(&recorded.outcome)
    }

    /// Restart the feeder. The feeder refuses `busy` while a feed is
    /// queued or runs. A restart whose answer was lost is never resent: the
    /// boot count in its status says whether it ran.
    pub async fn reboot(
        &self,
        caller: &Caller,
        id: FeederId,
        key: Option<IdempotencyKey>,
    ) -> Result<()> {
        self.live_row(id).await?;
        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Reboot,
                body: &Empty {},
                key,
                op: Op::Reboot,
            })
            .await?;
        done(&recorded.outcome)
    }

    /// Play a tune now; refused `muted` while the sound setting is off.
    /// The firmware steps its speaker only in setup mode for now, so it
    /// refuses `unavailable` in the loop.
    pub async fn play(
        &self,
        caller: &Caller,
        id: FeederId,
        request: PlayRequest,
        key: Option<IdempotencyKey>,
    ) -> Result<()> {
        self.live_row(id).await?;
        let tune = match request.tune {
            TuneName::Chime => Tune::Chime,
            TuneName::Meal => Tune::Meal,
            TuneName::Coin => Tune::Coin,
            TuneName::Alert => Tune::Alert,
            TuneName::Test => Tune::Test,
            TuneName::Twinkle => Tune::Twinkle,
        };
        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Play,
                body: &request,
                key,
                op: Op::Play { tune },
            })
            .await?;
        done(&recorded.outcome)
    }

    /// Remove the feeder: revoke the phone keys still owed a revoke on it,
    /// then the server's own key, stop mirroring it and archive it, whether
    /// or not the revokes landed, which the answer says. The feeder ends
    /// the session of a key it revokes before answering, so the server's
    /// own revoke is confirmed by the feeder refusing the reconnect. Its
    /// history, samples and journal stay readable, and adopting it again
    /// un-archives it and resumes the owed revokes. Runs beside no
    /// adoption, so a re-adoption cannot interleave with it.
    pub async fn remove_feeder(&self, caller: &Caller, id: FeederId) -> Result<Removal> {
        let _one_at_a_time = self.adopting.lock().await;
        let row = self.live_row(id).await?;
        let own = self.own_key(&row)?;
        // The server's key is the last that can revoke them.
        self.revoke_owed(id).await;
        let phones_owed = self.shared.store.revoking(id).await?;
        // Subscribed before the revoke goes out, so the refused reconnect
        // that confirms it cannot be missed.
        let updates = self.mirror.subscribe(id);

        let sent = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Remove,
                body: &Empty {},
                key: None,
                op: Op::RevokeClient { key: own },
            })
            .await
            .map(|recorded| recorded.outcome);
        let key = match own_revoke(sent)? {
            None => KeyRemoval::Revoked,
            Some(RevokeAttempt::Unknown) if self.refused_after(updates).await => {
                KeyRemoval::Revoked
            }
            Some(attempt) => KeyRemoval::Failed { attempt },
        };

        self.mirror.stop(id).await;
        let now = self.shared.clock.now();
        self.shared.store.archive_feeder(id, now).await?;
        let feeder = self.view(self.row(id).await?);
        Ok(Removal {
            feeder,
            key,
            phones_owed,
        })
    }

    /// Whether the feeder refused the server's next handshake within an
    /// answer's time: how a revoke of the server's own key is confirmed.
    /// The feeder closes the revoked key's session before its answer goes
    /// out, so that answer never arrives.
    async fn refused_after(&self, updates: Option<Receiver<MirrorEvent>>) -> bool {
        let Some(mut updates) = updates else {
            return false;
        };
        let refused = async {
            loop {
                match updates.recv().await {
                    Ok(MirrorEvent::Link(Link::Refused)) => return true,
                    Ok(MirrorEvent::Link(Link::Online)) | Err(RecvError::Closed) => return false,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                }
            }
        };
        let wait = self.shared.config.timing.answer;
        tokio::time::timeout(wait, refused).await.unwrap_or(false)
    }
}
