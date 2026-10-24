//! FW45's raw samplers on the fake: the jitter sampler measures SysTick
//! cycles between 32 kHz edges across a wrap, and refuses a stopped timer or
//! a SysTick that is not running; the ADC sampler keeps a conversion's low
//! byte.

use petlibre_hal::adc;
use petlibre_hal::entropy::{Jitter, adc_sample};
use petlibre_hal::sdk::fake;

fn ticking(reload: u32, step: u32, value: u32) {
    fake::reset_hardware();
    fake::with(|h| {
        h.ticks_per_read = 1;
        h.systick_reload = reload;
        h.systick_step = step;
        h.systick_value = value;
    });
}

#[test]
fn jitter_counts_cycles_between_edges_across_the_reload() {
    // One SysTick read per sample, each 300 cycles on from the last; the
    // count starts 500 above zero, so the third sample wraps.
    ticking(999, 300, 500);
    let mut jitter = Jitter::default();
    assert_eq!(jitter.sample(), None, "the first call only reads");
    for _ in 0..4 {
        assert_eq!(jitter.sample(), Some(300u32.to_le_bytes()[0]));
    }
}

#[test]
fn jitter_needs_a_moving_timer_and_a_running_systick() {
    ticking(999, 1, 0);
    fake::with(|h| h.ticks_per_read = 0);
    assert_eq!(Jitter::default().sample(), None, "no 32 kHz edge");

    ticking(0, 1, 0);
    assert_eq!(Jitter::default().sample(), None, "SysTick stopped");
}

#[test]
fn the_adc_sample_is_a_conversions_low_byte() {
    fake::reset_hardware();
    fake::with(|h| h.supply_adc = 0x0abc);
    assert!(adc::open().calibration.is_some());
    assert_eq!(adc_sample(10, 10), Some(0xbc));
}
