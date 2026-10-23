//! The firmware's start hook and, in the `fw16-pulse` variant, its one task.

petlibre_sdk::firmware!(start);

/// The normal image: no task, so nothing holds the KM0 flash handshake.
#[cfg(not(feature = "fw16-pulse"))]
fn start() {
    petlibre_sdk::startup_done(true);
}

#[cfg(feature = "fw16-pulse")]
use pulse::start;

#[cfg(feature = "fw16-pulse")]
mod pulse {
    use core::ffi::c_void;

    use petlibre_probes::pulse;
    use petlibre_sdk::Slot;
    use petlibre_sdk::hal::flash::park::Hardware;
    use petlibre_sdk::hal::sdk;

    pub(super) fn start() {
        // SAFETY: the step ignores its context and keeps its state in statics.
        let created =
            unsafe { petlibre_sdk::spawn(c"petlibre-pulse", PROBE_STACK_WORDS, 3, pulse_task) };
        sdk::log(if created {
            c"PLMOT1 start created=1\r\n"
        } else {
            c"PLMOT1 start created=0\r\n"
        });
    }

    /// Building the probe in place copies it through two constructor frames
    /// (about 3.6 KiB), so 4 KiB overflowed once LTO inlined differently.
    const PROBE_STACK_WORDS: u16 = 2048;

    /// Touched only by the pulse task; see `pulse_task`.
    static PULSE: Slot<pulse::Probe> = Slot::new();

    unsafe extern "C" fn pulse_task(_: *mut c_void) -> u32 {
        // SAFETY: only this task touches the slot; the probe stays in place.
        let probe = unsafe { PULSE.get_or_init(Default::default) };
        let step = probe.step::<Hardware>();
        if let Some(record) = step.record {
            petlibre_probes::log(format_args!("PLMOT1 {record}\r\n"));
        }
        // The probe holds the KM0 flash handshake until it ends.
        petlibre_sdk::startup_done(step.delay_ms == 0);
        step.delay_ms
    }
}
