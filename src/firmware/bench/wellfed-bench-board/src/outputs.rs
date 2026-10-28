//! The four emulated relays. This is the only module that drives the feeder
//! power, strap and UART buffer-enable pins.

use portable_atomic::{AtomicU8, Ordering};

use embassy_rp::gpio::{Level, Output};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};
use wellfed_bench_at::{Channel, Channels};

pub(crate) struct Outputs {
    /// GPIO2 FDR_PWR_EN, AP2171W EN, active high.
    pub(crate) power: Output<'static>,
    /// GPIO4 STRAP_EN, 2N7002 gate, active high.
    pub(crate) strap: Output<'static>,
    /// GPIO5 UART_TX_OE_N, 74LVC2G125 channel A, active low.
    pub(crate) tx_oe_n: Output<'static>,
    /// GPIO6 UART_RX_OE_N, 74LVC2G125 channel B, active low.
    pub(crate) rx_oe_n: Output<'static>,
}

pub(crate) struct Relays {
    pins: Outputs,
    state: Channels,
}

impl Relays {
    pub(crate) fn set(&mut self, channel: Channel, on: bool) {
        let (pin, active) = match channel {
            Channel::Power => (&mut self.pins.power, Level::High),
            Channel::Strap => (&mut self.pins.strap, Level::High),
            Channel::UartTx => (&mut self.pins.tx_oe_n, Level::Low),
            Channel::UartRx => (&mut self.pins.rx_oe_n, Level::Low),
        };
        let inactive = match active {
            Level::High => Level::Low,
            Level::Low => Level::High,
        };
        pin.set_level(if on { active } else { inactive });
        self.state.set(channel, on);
        SNAPSHOT.store(pack(self.state), Ordering::Relaxed);
    }

    pub(crate) fn state(&self) -> Channels {
        self.state
    }

    /// The bench's `cut_power`: strap first so the feeder never powers up
    /// strapped by accident, then power, then the UART lines so the bridge
    /// cannot back-power an unpowered feeder.
    pub(crate) fn cut_power(&mut self) {
        self.set(Channel::Strap, false);
        self.set(Channel::Power, false);
        self.set(Channel::UartTx, false);
        self.set(Channel::UartRx, false);
    }

    /// The bench's `power_on`: UART first so the ROM's first bytes are seen.
    pub(crate) fn power_on(&mut self) {
        self.set(Channel::UartTx, true);
        self.set(Channel::UartRx, true);
        self.set(Channel::Power, true);
    }
}

/// Held for a whole command, or for a whole RECOVER sequence so host commands
/// wait instead of interleaving with it.
static RELAYS: Mutex<CriticalSectionRawMutex, Option<Relays>> = Mutex::new(None);

/// Lock-free copy of the state for the LEDs.
static SNAPSHOT: AtomicU8 = AtomicU8::new(0);

pub(crate) async fn init(pins: Outputs) {
    *RELAYS.lock().await = Some(Relays {
        pins,
        state: Channels::default(),
    });
}

/// `main` awaits `init` before it spawns any task that locks the relays.
const INITIALIZED: &str = "outputs::init runs before any task is spawned";

pub(crate) struct Guard(MutexGuard<'static, CriticalSectionRawMutex, Option<Relays>>);

impl core::ops::Deref for Guard {
    type Target = Relays;
    fn deref(&self) -> &Relays {
        self.0.as_ref().expect(INITIALIZED)
    }
}

impl core::ops::DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut Relays {
        self.0.as_mut().expect(INITIALIZED)
    }
}

pub(crate) async fn lock() -> Guard {
    Guard(RELAYS.lock().await)
}

pub(crate) fn snapshot() -> Channels {
    let bits = SNAPSHOT.load(Ordering::Relaxed);
    let mut state = Channels::default();
    for channel in Channel::ALL {
        state.set(channel, bits & (1 << channel.index()) != 0);
    }
    state
}

fn pack(state: Channels) -> u8 {
    Channel::ALL
        .iter()
        .filter(|&&c| state.get(c))
        .fold(0, |bits, &c| bits | 1 << c.index())
}
