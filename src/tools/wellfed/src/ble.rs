//! `wellfed setup`'s radio half, and `wellfed install`'s: find a feeder in
//! setup mode through BlueZ and carry `wellfed_client::setup`'s messages
//! over its control characteristic, as `wellfed_install`'s
//! [`Bluetooth`] and [`SetupLink`].
//!
//! BlueZ quirks met on the bench shape this: a filtered discovery
//! crashed bluetoothd 5.87, so discovery is unfiltered and the service UUID
//! is checked here; and `notify()` can miss a notification sent as soon as
//! they are enabled, so the socket from `notify_io()` is held instead.

use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bluer::gatt::WriteOp;
use bluer::gatt::remote::{Characteristic, CharacteristicWriteRequest};
use bluer::{Address, Uuid};
use futures_util::{Stream, StreamExt};
use tokio::time::timeout;
use wellfed_client::setup::{CONTROL, Deframer, SERVICE, frame};
use wellfed_install::{Bluetooth, PlatformError, SetupLink};

const FIND_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
/// Control writes fit the smallest ATT payload, so they never depend on the
/// MTU exchange having happened.
const CONTROL_CHUNK: usize = 20;

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

/// One connected feeder's control characteristic.
pub(crate) struct Link {
    device: bluer::Device,
    control: Characteristic,
    notifications: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
    deframer: Deframer,
}

impl Link {
    /// Find the feeder (`address`, or the first advertising setup), connect
    /// and subscribe.
    pub(crate) async fn open(address: Option<Address>) -> Result<Self> {
        let session = bluer::Session::new().await.context("connect to BlueZ")?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;
        let device = find(&adapter, address).await?;
        eprintln!("Found a feeder at {}; connecting…", device.address());
        if !device.is_connected().await? {
            timeout(CONNECT_TIMEOUT, device.connect())
                .await
                .context("connecting timed out")?
                .context("connect to the feeder")?;
        }

        let control = timeout(CONNECT_TIMEOUT, control(&device))
            .await
            .context("the feeder's services did not resolve")??;
        let reader = timeout(CONNECT_TIMEOUT, control.notify_io())
            .await
            .context("enabling notifications timed out")??;
        let notifications = futures_util::stream::unfold(reader, |reader| async move {
            let notification = reader.recv().await.ok()?;
            Some((notification, reader))
        });
        Ok(Self {
            device,
            control,
            notifications: Box::pin(notifications),
            deframer: Deframer::default(),
        })
    }

    /// Send one message, framed. Write requests, so each piece is
    /// acknowledged and arrives in order.
    pub(crate) async fn send(&mut self, message: &[u8]) -> Result<()> {
        let write = CharacteristicWriteRequest {
            op_type: WriteOp::Request,
            ..Default::default()
        };
        for piece in frame(message)?.chunks(CONTROL_CHUNK) {
            timeout(CONNECT_TIMEOUT, self.control.write_ext(piece, &write))
                .await
                .context("writing to the feeder timed out")?
                .context("write to the feeder")?;
        }
        Ok(())
    }

    /// The next message, waiting at most `wait`.
    pub(crate) async fn receive(&mut self, wait: Duration) -> Result<Vec<u8>> {
        timeout(wait, async {
            loop {
                if let Some(message) = self.deframer.next_message()? {
                    return Ok(message);
                }
                let notification = self
                    .notifications
                    .next()
                    .await
                    .context("the feeder dropped the link")?;
                self.deframer.push(&notification);
            }
        })
        .await
        .context("no answer from the feeder in time")?
    }

    /// Hang up, bounded: BlueZ can stall disconnecting a device the feeder
    /// already dropped.
    pub(crate) async fn close(self) {
        let _ = timeout(CLOSE_TIMEOUT, self.device.disconnect()).await;
    }
}

/// The computer's radio, finding the feeder at `address` or the first
/// one in setup mode.
pub(crate) struct Bluez {
    pub(crate) address: Option<Address>,
}

impl Bluetooth for Bluez {
    type Link = Link;

    async fn open(&mut self) -> Result<Link, PlatformError> {
        Ok(Link::open(self.address).await?)
    }
}

impl SetupLink for Link {
    async fn send(&mut self, message: &[u8]) -> Result<(), PlatformError> {
        Ok(Self::send(self, message).await?)
    }

    async fn receive(&mut self, wait: Duration) -> Result<Vec<u8>, PlatformError> {
        Ok(Self::receive(self, wait).await?)
    }

    async fn close(self) {
        Self::close(self).await;
    }
}

/// The feeder advertising the setup service now: `address` if given,
/// otherwise the first one heard. Discovery runs even for a known address:
/// BlueZ connects only to a device it has heard since it last forgot it,
/// and refuses one it has not ("not available").
async fn find(adapter: &bluer::Adapter, address: Option<Address>) -> Result<bluer::Device> {
    // Held for the search: discovery runs while the stream lives.
    let _discovery = adapter.discover_devices().await?;
    timeout(FIND_TIMEOUT, async {
        loop {
            // BlueZ reports a cached device once, before it has been heard,
            // so poll the list: only a device advertising now has an RSSI.
            for heard in adapter.device_addresses().await? {
                if address.is_some_and(|wanted| wanted != heard) {
                    continue;
                }
                let device = adapter.device(heard)?;
                // A feeder still linked from a run before stops
                // advertising; the address names it, so it counts.
                let advertising = device.rssi().await?.is_some()
                    || (address.is_some() && device.is_connected().await?);
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
    .context(
        "no feeder in setup mode nearby: hold Lock then Mic for 5 s on an unlocked feeder on mains power",
    )?
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
