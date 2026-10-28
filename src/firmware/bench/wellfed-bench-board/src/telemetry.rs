//! Feeder measurements, refreshed every 10 ms for the AT queries and RECOVER.

use core::cell::Cell;

use embassy_rp::adc::{Adc, Async, Channel};
use embassy_rp::gpio::Input;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Ticker};
use wellfed_bench_at::{Source, Telemetry};

pub(crate) struct Sensors {
    pub(crate) adc: Adc<'static, Async>,
    /// GPIO26/ADC0 VBUS_FDR_SENSE.
    pub(crate) vbus: Channel<'static>,
    /// GPIO27/ADC1 FDR_ISENSE.
    pub(crate) current: Channel<'static>,
    /// GPIO28/ADC2 FDR_TX_SENSE.
    pub(crate) tx: Channel<'static>,
    /// GPIO3 FDR_PWR_FLT_N, AP2171W FLG with R11 pull-up.
    pub(crate) fault_n: Input<'static>,
    /// GPIO7 AUX_ACTIVE, TPS2116 ST with R29 pull-up.
    pub(crate) aux: Input<'static>,
}

static LATEST: Mutex<CriticalSectionRawMutex, Cell<Telemetry>> =
    Mutex::new(Cell::new(Telemetry::new()));

pub(crate) fn latest() -> Telemetry {
    LATEST.lock(Cell::get)
}

const SAMPLES: u32 = 16;

#[embassy_executor::task]
pub(crate) async fn run(mut s: Sensors) -> ! {
    let mut ticker = Ticker::every(Duration::from_millis(10));
    loop {
        let vbus = average(&mut s.adc, &mut s.vbus).await;
        let current = average(&mut s.adc, &mut s.current).await;
        let tx = average(&mut s.adc, &mut s.tx).await;
        let t = Telemetry {
            vbus_mv: wellfed_bench_at::vbus_mv(vbus),
            current_ma: wellfed_bench_at::current_ma(current),
            // ST is open-drain and released (pulled high) while VIN1, the aux
            // USB-C, feeds the output. Confirm on the first board.
            source: if s.aux.is_high() {
                Source::Aux
            } else {
                Source::Host
            },
            fault: s.fault_n.is_low(),
            tx_mv: wellfed_bench_at::adc_mv(tx),
        };
        LATEST.lock(|cell| cell.set(t));
        ticker.next().await;
    }
}

async fn average(adc: &mut Adc<'static, Async>, channel: &mut Channel<'static>) -> u16 {
    let mut sum = 0u32;
    for _ in 0..SAMPLES {
        // A failed conversion counts as zero; the next tick retries.
        sum += u32::from(adc.read(channel).await.unwrap_or(0));
    }
    // At most 16 × 4095, so the average fits in u16.
    u16::try_from(sum / SAMPLES).unwrap_or(u16::MAX)
}
