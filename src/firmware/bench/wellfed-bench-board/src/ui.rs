//! LEDs, the RECOVER button and the watchdog.
//!
//! STAT LED: fast blink until USB is configured, a short heartbeat once it is,
//! solid while the strap is held, and triple blinks after a refused RECOVER.
//! UART LED: flashes on bridge traffic.
//!
//! RECOVER: hold 1 s to power-cycle the feeder into its ROM loader (the
//! bench's `loader` sequence); a short press is a plain power cycle. Flashing
//! a stored image from here comes later.

use portable_atomic::{AtomicU32, Ordering};

use embassy_rp::gpio::{Input, Output};
use embassy_rp::watchdog::Watchdog;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker, Timer};
use wellfed_bench_at::Channel;

use crate::{bridge, outputs, telemetry, usb};

pub(crate) struct Panel {
    /// GPIO16 D3 "STAT", active high.
    pub(crate) status_led: Output<'static>,
    /// GPIO17 D4 "UART", active high.
    pub(crate) uart_led: Output<'static>,
    /// GPIO8 SW3 "RECOVER", active low.
    pub(crate) recover_n: Input<'static>,
    pub(crate) watchdog: Watchdog,
}

#[derive(Clone, Copy)]
enum Press {
    Short,
    Long,
}

static PRESS: Signal<CriticalSectionRawMutex, Press> = Signal::new();

/// Ticks left of the "refused" blink pattern.
static REFUSED_TICKS: AtomicU32 = AtomicU32::new(0);

const TICK: Duration = Duration::from_millis(10);
const LONG_PRESS_TICKS: u32 = 100;
const DEBOUNCE_TICKS: u32 = 3;
const UART_FLASH_TICKS: u32 = 3;
const REFUSED_BLINK_TICKS: u32 = 300;
/// Longer than any blocking stretch: the bridge and relay tasks only await.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(2);

#[embassy_executor::task]
pub(crate) async fn run(mut panel: Panel) -> ! {
    panel.watchdog.start(WATCHDOG_TIMEOUT);
    let mut ticker = Ticker::every(TICK);
    let mut held = 0u32;
    let mut long_sent = false;
    let mut uart_flash = 0u32;

    loop {
        panel.watchdog.feed(WATCHDOG_TIMEOUT);

        if panel.recover_n.is_low() {
            held = held.saturating_add(1);
            if held == LONG_PRESS_TICKS {
                long_sent = true;
                PRESS.signal(Press::Long);
            }
        } else {
            if held >= DEBOUNCE_TICKS && !long_sent {
                PRESS.signal(Press::Short);
            }
            held = 0;
            long_sent = false;
        }

        if bridge::TRAFFIC.swap(false, Ordering::Relaxed) {
            uart_flash = UART_FLASH_TICKS;
        }
        panel.uart_led.set_level((uart_flash > 0).into());
        uart_flash = uart_flash.saturating_sub(1);

        panel.status_led.set_level(status_on().into());
        ticker.next().await;
    }
}

fn status_on() -> bool {
    let ms = Instant::now().as_millis();
    let refused = REFUSED_TICKS.load(Ordering::Relaxed);
    if refused > 0 {
        REFUSED_TICKS.store(refused - 1, Ordering::Relaxed);
        // Three 100 ms blinks per second.
        return ms % 1000 < 600 && ms % 200 < 100;
    }
    if outputs::snapshot().get(Channel::Strap) {
        return true;
    }
    if usb::configured() {
        ms % 1000 < 80
    } else {
        ms % 250 < 125
    }
}

/// Seconds the feeder stays off before power returns, as the bench does.
const OFF: Duration = Duration::from_secs(2);
/// Strap hold after power-on, as the bench's `STRAP_SECONDS`.
const STRAP_HOLD: Duration = Duration::from_secs(2);

#[embassy_executor::task]
pub(crate) async fn recover() -> ! {
    loop {
        let press = PRESS.wait().await;
        // Holding the lock makes host relay commands wait for the sequence.
        let mut relays = outputs::lock().await;
        relays.cut_power();
        Timer::after(OFF).await;

        match press {
            Press::Short => relays.power_on(),
            // Feeder TX still high with its 5 V cut: the D-cells are powering
            // it, so a power cycle cannot reach the ROM loader. Leave it off.
            Press::Long if telemetry::latest().alive() => {
                REFUSED_TICKS.store(REFUSED_BLINK_TICKS, Ordering::Relaxed);
            }
            Press::Long => {
                relays.set(Channel::Strap, true);
                relays.power_on();
                Timer::after(STRAP_HOLD).await;
                relays.set(Channel::Strap, false);
            }
        }
        // A press made during the sequence does not queue another one.
        PRESS.reset();
    }
}
