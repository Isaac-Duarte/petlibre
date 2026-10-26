//! The operator's interrupt, as one handle a bench process hands to
//! everything that waits.
//!
//! It lives beside the bench's files rather than in the rig because the
//! read-only page (`wellfed-dashboard`) stops on the same interrupt and
//! links neither the rig nor the relay.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once and never cleared (by Ctrl-C, or by an owner stopping what it
/// started, as the capture chain does): every clone sees it, so a wait
/// anywhere in the process ends and its sequence stops through its cleanup.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_clone_sees_the_cancel() {
        let cancel = Cancel::new();
        let clone = cancel.clone();
        assert!(!clone.is_cancelled());

        cancel.cancel();

        assert!(clone.is_cancelled());
    }
}
