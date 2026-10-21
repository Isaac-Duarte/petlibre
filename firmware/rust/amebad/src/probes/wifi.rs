//! FW07 stage B bench probe on KM4: radio on and scan only.
//!
//! Starts the KM4 Wi-Fi shim (`example_entry.c`: LwIP, `wifi_on(STA)`, then
//! `SCANS` scans) and polls its state. Each scan is reported with its result,
//! duration and network count, then one record per network carrying only a
//! CRC-32 of the SSID with RSSI, channel and security. No credentials exist
//! in this build and nothing joins a network. Outputs stay as startup left
//! them (STOP, emitters OFF); every scan record re-verifies that readback.

use crate::sdk::{self, WifiAp, WifiState};
use crate::stopped;

pub const POLL_MS: u32 = 250;
/// The shim's scans per boot.
pub const SCANS: u32 = 3;
/// `wifi_on` waits up to 20 s for the driver; allow margin before giving up.
pub const ON_TIMEOUT_MS: u32 = 30_000;
/// Each scan waits at most 10 s, with 5 s between scans.
pub const SCAN_TIMEOUT_MS: u32 = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Start {
        created: bool,
    },
    On {
        result: i32,
        ms: u32,
    },
    Scan {
        n: u32,
        result: i32,
        ms: u32,
        count: u32,
        dropped: u32,
        stopped: bool,
    },
    Ap {
        n: u32,
        index: u32,
        ap: WifiAp,
    },
    Done {
        scans: u32,
    },
    /// The shim made no progress in time: `stage` 0 waiting for `wifi_on`,
    /// otherwise the scan number awaited.
    Timeout {
        stage: u32,
        ms: u32,
    },
}

/// `delay_ms == 0` ends the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

#[derive(Debug, Default)]
pub struct Probe {
    started: bool,
    done: bool,
    on_reported: bool,
    scans_reported: u32,
    /// Networks of the last reported scan still to log.
    pending: u32,
    state: WifiState,
    /// Milliseconds since the last progress.
    waited_ms: u32,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

impl Probe {
    fn end(&mut self, record: Record) -> Step {
        self.done = true;
        next(0, Some(record))
    }

    /// One task iteration in task context.
    pub fn step(&mut self) -> Step {
        if self.done {
            return next(0, None);
        }
        if !self.started {
            self.started = true;
            let created = sdk::wifi_start();
            if !created {
                return self.end(Record::Start { created });
            }
            return next(POLL_MS, Some(Record::Start { created }));
        }
        if self.pending > 0 {
            let index = self.state.count.min(sdk::WIFI_APS as u32) - self.pending;
            self.pending -= 1;
            let ap = self.state.aps[index as usize];
            return next(
                1,
                Some(Record::Ap {
                    n: self.scans_reported,
                    index,
                    ap,
                }),
            );
        }
        let state = sdk::wifi_snapshot();
        if !self.on_reported {
            if state.on_done == 0 {
                return self.wait(0, ON_TIMEOUT_MS);
            }
            self.on_reported = true;
            self.waited_ms = 0;
            let record = Record::On {
                result: state.on_result,
                ms: state.on_ms,
            };
            if state.on_result != 0 {
                return self.end(record);
            }
            return next(POLL_MS, Some(record));
        }
        if state.scans_done > self.scans_reported {
            self.scans_reported = state.scans_done;
            self.state = state;
            self.pending = state.count.min(sdk::WIFI_APS as u32);
            self.waited_ms = 0;
            return next(
                1,
                Some(Record::Scan {
                    n: state.scans_done,
                    result: state.scan_result,
                    ms: state.scan_ms,
                    count: state.count,
                    dropped: state.dropped,
                    stopped: stopped::observe().verified(),
                }),
            );
        }
        if self.scans_reported >= SCANS {
            return self.end(Record::Done {
                scans: self.scans_reported,
            });
        }
        self.wait(self.scans_reported + 1, SCAN_TIMEOUT_MS)
    }

    fn wait(&mut self, stage: u32, limit: u32) -> Step {
        self.waited_ms += POLL_MS;
        if self.waited_ms > limit {
            return self.end(Record::Timeout {
                stage,
                ms: self.waited_ms,
            });
        }
        next(POLL_MS, None)
    }
}
