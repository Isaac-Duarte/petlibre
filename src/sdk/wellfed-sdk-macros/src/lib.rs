//! EXPERIMENTAL attributes a Wellfed firmware is written with: its start
//! hook, its tasks and its log records. `wellfed-sdk` re-exports them; a
//! firmware never depends on this crate directly.
//!
//! Each expands to the shape the build's audits read and the runtime calls:
//! the `wellfed_firmware_start` hook, a non-generic `extern "C"` task entry
//! named after the function, and one `write!` per record variant, as a
//! hand-written formatter would be.

mod record;
mod start;
mod task;

use proc_macro::TokenStream;

/// The firmware's start hook:
///
/// ```ignore
/// #[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
/// fn start() -> Startup {
///     wellfed_sdk::spawn(blink_task);
///     Startup::Done
/// }
/// ```
///
/// `runtime` names the product runtime crate whose record stores the SDK's
/// settings writer and Wi-Fi join call; naming it here links it.
#[proc_macro_attribute]
pub fn start(args: TokenStream, item: TokenStream) -> TokenStream {
    start::expand(args.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// One of the firmware's tasks: a step that returns the milliseconds to
/// sleep before the next step, or zero to end the task.
///
/// ```ignore
/// #[wellfed_sdk::task(name = "wellfed-blink", stack = BLINK_STACK_WORDS, priority = 2)]
/// fn blink_task() -> u32 { 0 }
/// ```
///
/// The function becomes a `wellfed_sdk::Task` constant of the same name,
/// for `wellfed_sdk::spawn`. `stack` is in words, a literal or a
/// `const NAME: u16` in the firmware's sources, where the stack audit reads
/// it.
#[proc_macro_attribute]
pub fn task(args: TokenStream, item: TokenStream) -> TokenStream {
    task::expand(args.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `Display` for a record as the body of its `PL…1` line, and with
/// `#[record(tag = "PLBLK1")]` the `LogRecord` trait that logs the whole line.
/// See `record.rs` for the attributes.
#[proc_macro_derive(Record, attributes(record))]
pub fn derive_record(item: TokenStream) -> TokenStream {
    syn::parse(item)
        .and_then(|input| record::expand(&input))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
