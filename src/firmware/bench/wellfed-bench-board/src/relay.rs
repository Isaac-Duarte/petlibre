//! CDC#1: the SH-UR04A relay protocol plus telemetry queries.
//!
//! One reply per command line, nothing unprompted: `wellfed-relay` flushes
//! its input, sends a line and reads exactly one reply line within 1 s.

use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::Sender;
use wellfed_bench_at::{Command, Line, LineBuffer};

use crate::usb::{Cdc, PACKET, UsbDriver};
use crate::{outputs, telemetry};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[embassy_executor::task]
pub(crate) async fn run(class: Cdc) -> ! {
    let (mut sender, mut receiver) = class.split();
    let mut lines = LineBuffer::default();
    let mut buf = [0u8; PACKET as usize];
    loop {
        receiver.wait_connection().await;
        let Ok(n) = receiver.read_packet(&mut buf).await else {
            // Bus reset or unplug: drop any half line.
            lines = LineBuffer::default();
            continue;
        };
        for &byte in &buf[..n] {
            match lines.push(byte) {
                None => {}
                Some(Line::TooLong) => send(&mut sender, wellfed_bench_at::ERROR).await,
                Some(Line::Complete(line)) => execute(&mut sender, &line).await,
            }
        }
    }
}

async fn execute(sender: &mut Sender<'static, UsbDriver>, line: &[u8]) {
    let Some(command) = wellfed_bench_at::parse(line) else {
        send(sender, wellfed_bench_at::ERROR).await;
        return;
    };

    let channels = {
        let mut relays = outputs::lock().await;
        if let Command::Set(channel, on) = command {
            relays.set(channel, on);
        }
        relays.state()
    };
    let reply = wellfed_bench_at::reply(command, channels, &telemetry::latest(), VERSION);
    send(sender, &reply).await;

    if command == Command::Bootsel {
        // Let the host read the OK before the device drops off the bus.
        Timer::after(Duration::from_millis(100)).await;
        outputs::lock().await.cut_power();
        embassy_rp::rom_data::reset_to_usb_boot(0, 0);
    }
}

async fn send(sender: &mut Sender<'static, UsbDriver>, text: &str) {
    let mut out = heapless::Vec::<u8, { wellfed_bench_at::REPLY_MAX + 2 }>::new();
    // REPLY_MAX bounds every reply, so the CRLF always fits.
    let _ = out.extend_from_slice(text.as_bytes());
    let _ = out.extend_from_slice(b"\r\n");
    for chunk in out.chunks(usize::from(PACKET)) {
        if sender.write_packet(chunk).await.is_err() {
            return;
        }
    }
}
