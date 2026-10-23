//! `wifi --ble`: give an offline feeder running the `ble` firmware its Wi-Fi
//! credentials over Bluetooth LE, through BlueZ (FW34).
//!
//! The feeder advertises its setup service (`wellfed_core::ble_setup`) once
//! it has been offline for two minutes. This client finds it by the service
//! UUID, reads the hello's nonce, and sends the same signed FW25 `wifi`
//! request the server sends over MQTT, signed under that nonce. The feeder
//! answers with the transport's own reply and joins the new network at its
//! next attempt. Neither the request nor the reply is printed with the
//! passphrase in it.

use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use bluer::gatt::WriteOp;
use bluer::gatt::remote::{Characteristic, CharacteristicWriteRequest};
use bluer::{Address, Uuid};
use futures_util::{Stream, StreamExt};
use wellfed_core::ble_setup::{CONTROL, PREFIX, SERVICE, parse_prefix, prefix};
use serde::Deserialize;
use tokio::time::timeout;

use crate::config::Config;
use crate::replacement::{Input, Op};

const FIND_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Control writes fit the smallest ATT payload, so they never depend on
/// the MTU exchange having happened.
const CONTROL_CHUNK: usize = 20;

/// What to ask of the feeder.
pub enum Change {
    /// Store these credentials; the feeder joins with them next.
    Store { ssid: String, psk: String },
    /// Return to the build's credentials.
    Clear,
    /// Read which credentials are in use.
    Read,
}

#[derive(Deserialize)]
struct Hello {
    op: String,
    nonce: String,
}

/// Notifications reassembled into length-prefixed messages.
struct Messages {
    notifications: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
    pending: VecDeque<u8>,
}

impl Messages {
    async fn fill(&mut self, n: usize, what: &str) -> Result<()> {
        while self.pending.len() < n {
            let notification = self
                .notifications
                .next()
                .await
                .with_context(|| format!("the feeder dropped the link before its {what}"))?;
            self.pending.extend(notification);
        }
        Ok(())
    }

    /// The next message, waiting at most [`ANSWER_TIMEOUT`].
    async fn next(&mut self, what: &str) -> Result<Vec<u8>> {
        timeout(ANSWER_TIMEOUT, async {
            self.fill(PREFIX, what).await?;
            let prefix = [self.pending[0], self.pending[1]];
            let len = parse_prefix(prefix).context("the feeder sent an oversized message")?;
            self.fill(PREFIX + len, what).await?;
            Ok(self.pending.drain(..PREFIX + len).skip(PREFIX).collect())
        })
        .await
        .with_context(|| format!("no {what} from the feeder"))?
    }
}

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

/// The feeder: `address` if given, otherwise the first device advertising
/// the setup service (already known devices included).
async fn find(adapter: &bluer::Adapter, address: Option<Address>) -> Result<bluer::Device> {
    if let Some(address) = address {
        return Ok(adapter.device(address)?);
    }

    // No discovery filter: BlueZ 5.87 crashed (SIGSEGV) twice on the bench
    // around a filtered discovery's client going away, so the service UUID
    // is checked here instead. Held for the search: discovery runs while
    // the stream lives.
    let _discovery = adapter.discover_devices().await?;
    timeout(FIND_TIMEOUT, async {
        loop {
            // BlueZ reports a cached device once, when discovery starts and
            // before it has been heard, so poll the list: only a device
            // advertising now has an RSSI.
            for address in adapter.device_addresses().await? {
                let device = adapter.device(address)?;
                let advertising = device.rssi().await?.is_some();
                let setup = device
                    .uuids()
                    .await?
                    .is_some_and(|uuids| uuids.contains(&uuid(SERVICE)));
                if advertising && setup {
                    return Ok::<_, anyhow::Error>(device);
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .context("no feeder advertising its setup service; it advertises after two minutes offline")?
}

async fn control(device: &bluer::Device) -> Result<Characteristic> {
    for service in device.services().await? {
        if service.uuid().await? != uuid(SERVICE) {
            continue;
        }
        for characteristic in service.characteristics().await? {
            if characteristic.uuid().await?.as_u128() == CONTROL {
                return Ok(characteristic);
            }
        }
    }
    bail!("the device has no setup service")
}

/// Send `change` to the feeder; returns its reply.
#[allow(clippy::print_stdout)]
pub async fn send(config: &Config, address: Option<Address>, change: Change) -> Result<String> {
    let key = config
        .firmware
        .as_ref()
        .and_then(|firmware| firmware.request_key())
        .context("no firmware request key in the config (wellfed-local firmware-request-key)")?;
    let mut input = Input::op(Op::Wifi);
    match change {
        Change::Store { ssid, psk } => (input.ssid, input.psk) = (Some(ssid), Some(psk)),
        Change::Clear => input.clear = Some(true),
        Change::Read => {}
    }
    ensure!(
        input.valid(),
        "SSID must be 1-32 bytes and the passphrase 8-63 printable ASCII characters"
    );

    let session = bluer::Session::new().await.context("connect to BlueZ")?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    let device = find(&adapter, address).await?;
    println!("Found the feeder at {}; connecting…", device.address());
    if !device.is_connected().await? {
        timeout(CONNECT_TIMEOUT, device.connect())
            .await
            .context("connecting timed out")?
            .context("connect to the feeder")?;
    }
    let result = exchange(&device, &key, &input).await;
    let _ = device.disconnect().await;
    result
}

async fn exchange(
    device: &bluer::Device,
    key: &crate::request_auth::RequestKey,
    input: &Input,
) -> Result<String> {
    let control = timeout(CONNECT_TIMEOUT, control(device))
        .await
        .context("the feeder's services did not resolve")??;
    // The feeder sends its hello as soon as notifications are on. `notify()`
    // subscribes to value changes only after enabling them and can miss it;
    // `notify_io()` holds the socket from the moment they are enabled.
    let reader = timeout(ANSWER_TIMEOUT, control.notify_io())
        .await
        .context("enabling notifications timed out")??;
    let notifications = futures_util::stream::unfold(reader, |reader| async move {
        let notification = reader.recv().await.ok()?;
        Some((notification, reader))
    });
    let mut messages = Messages {
        notifications: Box::pin(notifications),
        pending: VecDeque::new(),
    };

    let hello: Hello = serde_json::from_slice(&messages.next("hello").await?)
        .context("the feeder's hello is not JSON")?;
    ensure!(hello.op == "hello", "the feeder did not say hello");
    let request = input
        .wire(uuid::Uuid::new_v4())
        .signed(key, &hello.nonce, 1);
    let prefix = prefix(request.len()).context("the request is too long")?;

    // `write` defaults to a write command; control takes write requests, so
    // each piece is acknowledged and arrives in order.
    let write = CharacteristicWriteRequest {
        op_type: WriteOp::Request,
        ..Default::default()
    };
    let framed: Vec<u8> = prefix.into_iter().chain(request.into_bytes()).collect();
    for piece in framed.chunks(CONTROL_CHUNK) {
        timeout(ANSWER_TIMEOUT, control.write_ext(piece, &write))
            .await
            .context("writing the request timed out")?
            .context("write the request")?;
    }

    let reply = messages.next("reply").await?;
    String::from_utf8(reply).context("the feeder's reply is not text")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_reassemble_across_notifications() {
        let hello = br#"{"op":"hello","nonce":"0123456789abcdef"}"#;
        let mut bytes = prefix(hello.len()).unwrap().to_vec();
        bytes.extend_from_slice(hello);
        bytes.extend_from_slice(&prefix(2).unwrap());
        bytes.extend_from_slice(b"{}");
        let pieces: Vec<Vec<u8>> = bytes.chunks(7).map(<[u8]>::to_vec).collect();
        let mut messages = Messages {
            notifications: Box::pin(futures_util::stream::iter(pieces)),
            pending: VecDeque::new(),
        };

        let first: Hello = serde_json::from_slice(&messages.next("hello").await.unwrap()).unwrap();
        assert_eq!(
            (first.op.as_str(), first.nonce.as_str()),
            ("hello", "0123456789abcdef")
        );
        assert_eq!(messages.next("reply").await.unwrap(), b"{}");
        assert!(messages.next("reply").await.is_err(), "the link ended");
    }
}
