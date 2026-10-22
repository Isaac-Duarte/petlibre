//! FW07 stage D bench probe on KM4: join the configured network and keep it.
//!
//! Starts the KM4 Wi-Fi shim (`example_entry.c`), which owns the connection:
//! radio on, then scan, connect and DHCP, then periodic DNS lookups while it
//! watches the link, rejoining with bounded backoff when the link drops or
//! DNS keeps failing. This probe drains the shim's numbered event
//! ring without end and logs each event once, with stopped-output readback.
//! The SSID and password never pass through here. Outputs stay as startup
//! left them (STOP, emitters OFF).

use petlibre_amebad::sdk::{self, WifiEvent, WifiPoll};
use petlibre_amebad::stopped;

pub const POLL_MS: u32 = 250;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Start {
        created: bool,
    },
    Event {
        event: WifiEvent,
        stopped: bool,
    },
    /// The ring overwrote event `seq` before it was read; reading resumes past it.
    Overrun {
        seq: u32,
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
    /// The next event to read.
    next: u32,
}

fn step(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

impl Probe {
    /// One task iteration in task context.
    pub fn step(&mut self) -> Step {
        if self.done {
            return step(0, None);
        }
        if !self.started {
            self.started = true;
            self.next = 1;
            let created = sdk::wifi_start();
            self.done = !created;
            return step(
                if created { POLL_MS } else { 0 },
                Some(Record::Start { created }),
            );
        }
        match sdk::wifi_event(self.next) {
            WifiPoll::Pending => step(POLL_MS, None),
            WifiPoll::Event(event) => {
                self.next += 1;
                petlibre_amebad::net_status::observe(&event);
                let stopped = stopped::observe().verified();
                // Drain quickly; the ring holds 16.
                step(1, Some(Record::Event { event, stopped }))
            }
            WifiPoll::Lost => {
                let seq = self.next;
                self.next += 1;
                step(1, Some(Record::Overrun { seq }))
            }
        }
    }
}
