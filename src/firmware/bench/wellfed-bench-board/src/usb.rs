//! USB composite device: two CDC-ACM functions.
//!
//! Function order fixes the Linux by-id names: the feeder UART is interface 0
//! (`...-if00`) and the relay port interface 2 (`...-if02`). The serial string
//! is the flash chip's unique ID, so each board keeps stable paths.

use portable_atomic::{AtomicBool, Ordering};

use embassy_rp::Peri;
use embassy_rp::interrupt::typelevel::Binding;
use embassy_rp::peripherals::{FLASH, USB};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config, Handler, UsbDevice};
use static_cell::StaticCell;

pub(crate) type UsbDriver = Driver<'static, USB>;
pub(crate) type Cdc = CdcAcmClass<'static, UsbDriver>;

/// pid.codes test VID:PID. Fine for prototypes; a real PID (pid.codes or a
/// Raspberry Pi 0x2E8A sub-PID) is needed before boards ship.
const VID: u16 = 0x1209;
const PID: u16 = 0x0001;

/// Full-speed bulk packet size.
pub(crate) const PACKET: u16 = 64;

static CONFIGURED: AtomicBool = AtomicBool::new(false);

/// True while the host has the device configured (enumerated and not reset).
pub(crate) fn configured() -> bool {
    CONFIGURED.load(Ordering::Relaxed)
}

struct UsbState;

impl Handler for UsbState {
    fn configured(&mut self, configured: bool) {
        CONFIGURED.store(configured, Ordering::Relaxed);
    }

    fn reset(&mut self) {
        CONFIGURED.store(false, Ordering::Relaxed);
    }

    fn suspended(&mut self, suspended: bool) {
        if suspended {
            CONFIGURED.store(false, Ordering::Relaxed);
        }
    }
}

/// The W25Q128's 64-bit unique ID as 16 uppercase hex digits.
pub(crate) fn serial_number(flash: Peri<'static, FLASH>) -> &'static str {
    const FLASH_SIZE: usize = 16 * 1024 * 1024;
    let mut flash =
        embassy_rp::flash::Flash::<_, embassy_rp::flash::Blocking, FLASH_SIZE>::new_blocking(flash);
    let mut id = [0u8; 8];
    // On failure every board would share the all-zero serial, which still
    // enumerates; by-id paths would just collide between boards.
    let _ = flash.blocking_unique_id(&mut id);

    static SERIAL: StaticCell<[u8; 16]> = StaticCell::new();
    let serial = SERIAL.init([0; 16]);
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for (i, byte) in id.iter().enumerate() {
        serial[2 * i] = HEX[usize::from(byte >> 4)];
        serial[2 * i + 1] = HEX[usize::from(byte & 0xF)];
    }
    // Only ASCII hex digits were written.
    core::str::from_utf8(serial).unwrap_or("0000000000000000")
}

pub(crate) fn build(
    usb: Peri<'static, USB>,
    irq: impl Binding<embassy_rp::interrupt::typelevel::USBCTRL_IRQ, InterruptHandler<USB>>,
    serial: &'static str,
) -> (UsbDevice<'static, UsbDriver>, Cdc, Cdc) {
    let driver = Driver::new(usb, irq);

    let mut config = Config::new(VID, PID);
    config.manufacturer = Some("Wellfed");
    config.product = Some("Wellfed Bench");
    config.serial_number = Some(serial);
    config.max_power = 500;
    config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 0]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 0]),
        CONTROL_BUF.init([0; 64]),
    );

    static HANDLER: StaticCell<UsbState> = StaticCell::new();
    builder.handler(HANDLER.init(UsbState));

    static UART_STATE: StaticCell<State> = StaticCell::new();
    static RELAY_STATE: StaticCell<State> = StaticCell::new();
    let uart = CdcAcmClass::new(&mut builder, UART_STATE.init(State::new()), PACKET);
    let relay = CdcAcmClass::new(&mut builder, RELAY_STATE.init(State::new()), PACKET);

    (builder.build(), uart, relay)
}

#[embassy_executor::task]
pub(crate) async fn run(mut device: UsbDevice<'static, UsbDriver>) -> ! {
    device.run().await
}
