//! Exports declared by `app/diagnostic.h`.

/// Synthetic observations and memory-only outputs; never accesses board I/O.
#[unsafe(no_mangle)]
pub extern "C" fn wellfed_diagnostic_run() -> bool {
    wellfed_core::feed::diagnostic::run()
}
