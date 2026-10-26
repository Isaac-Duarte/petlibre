//! The firmware's start hook: the variant's, or the normal image's. Each
//! variant module defines its own, and only one links.
//!
//! The build's stack audit reads every module here whichever variant links,
//! so each task's entry function and stack constant keeps a name of its own.

#[cfg(feature = "adc")]
mod adc;
#[cfg(feature = "buttons")]
mod buttons;
#[cfg(feature = "optics")]
mod optics;
#[cfg(feature = "pulse")]
mod pulse;
#[cfg(feature = "speaker")]
mod speaker;
#[cfg(feature = "stall")]
mod stall;

/// Each variant is one image: two would start each other's tasks and carry
/// only one variant's policies.
const _: () = {
    let variants = [
        cfg!(feature = "optics"),
        cfg!(feature = "buttons"),
        cfg!(feature = "adc"),
        cfg!(feature = "speaker"),
        cfg!(feature = "pulse"),
        cfg!(feature = "stall"),
        cfg!(feature = "fail"),
    ];
    let (mut on, mut i) = (0, 0);
    while i < variants.len() {
        if variants[i] {
            on += 1;
        }
        i += 1;
    }
    assert!(on <= 1, "wellfed-fw-bringup builds one variant per image");
};

/// The normal image and `fail` (whose failure is the runtime's): no task, so
/// nothing holds the KM0 flash handshake.
#[cfg(not(any(
    feature = "optics",
    feature = "buttons",
    feature = "adc",
    feature = "speaker",
    feature = "pulse",
    feature = "stall"
)))]
#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> wellfed_sdk::Startup {
    wellfed_sdk::Startup::Done
}
