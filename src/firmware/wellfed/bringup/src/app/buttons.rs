//! The `buttons` variant's start hook and its one task, which logs the raw
//! lines and reads the button service's gestures, as a firmware's owner task
//! would.

use wellfed_probes::buttons;
use wellfed_sdk::buttons::Config;
use wellfed_sdk::hal::sdk;
use wellfed_sdk::{LogRecord as _, Slot, Startup, log};

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    let created = wellfed_sdk::spawn(button_task);
    sdk::log(if created {
        c"PLBTN1 start created=1\r\n"
    } else {
        c"PLBTN1 start created=0\r\n"
    });
    wellfed_sdk::buttons::start(Config::BENCH);

    // Neither the probe nor the service binds the KM0 flash handshake, so the
    // runtime may confirm once it is otherwise healthy.
    Startup::Done
}

/// Touched only by the button task; see `button_task`.
static BUTTONS: Slot<buttons::Probe> = Slot::new();

#[wellfed_sdk::task(name = "wellfed-btn", stack = 1024, priority = 3)]
fn button_task() -> u32 {
    let Some(mut probe) = BUTTONS.enter_or_init(Default::default) else {
        return 0;
    };

    let step = probe.step();
    if let Some(record) = step.record {
        record.log();
    }

    // The service logs each gesture as it makes it; this line shows the
    // reader got the same gesture through the queue.
    while let Some(gesture) = wellfed_sdk::buttons::next() {
        log(format_args!("PLKEY1 read {gesture}\r\n"));
    }
    step.delay_ms
}
