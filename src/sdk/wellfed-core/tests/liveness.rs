//! Link liveness from association, lease and gateway: no internet needed.

use wellfed_core::liveness::{
    Cause, GATEWAY_MISSES, LEAVE, Liveness, PROBE_PERIOD_MS, Probe, Verdict,
};

/// A link sampled every watch tick, probing whenever the machine asks.
struct Link {
    machine: Liveness,
    now: u32,
    associated: bool,
    leased: bool,
    /// What the next probe sees.
    gateway: Probe,
    /// Every verdict other than `Hold` and `Probe`, with its time.
    seen: Vec<(u32, Verdict)>,
    probes: u32,
}

impl Link {
    fn joined(gateway: Probe) -> Self {
        Self {
            machine: Liveness::new(),
            now: 10_000,
            associated: true,
            leased: true,
            gateway,
            seen: Vec::new(),
            probes: 0,
        }
    }

    /// One watch tick as the shim runs it: step, and on `Probe` probe and
    /// step again with the result, then wait 500 ms.
    fn tick(&mut self) -> Verdict {
        let mut verdict = self
            .machine
            .step(self.now, self.associated, self.leased, Probe::None);
        while verdict == Verdict::Probe {
            self.probes += 1;
            verdict = self
                .machine
                .step(self.now, self.associated, self.leased, self.gateway);
        }

        if verdict != Verdict::Hold {
            self.seen.push((self.now, verdict));
        }
        self.now = self.now.wrapping_add(500);
        verdict
    }

    /// Ticks for `ms`, stopping at a leave.
    fn run(&mut self, ms: u32) -> Option<Cause> {
        for _ in 0..ms / 500 {
            if let Verdict::Leave(cause) = self.tick() {
                return Some(cause);
            }
        }
        None
    }
}

#[test]
fn codes_match_the_shim() {
    assert_eq!(Verdict::Hold.code(), 0);
    assert_eq!(Verdict::Probe.code(), 1);
    assert_eq!(Verdict::Settled.code(), 2);
    assert_eq!(Verdict::Leave(Cause::Link).code(), LEAVE);
    assert_eq!(Verdict::Leave(Cause::Lease).code(), LEAVE + 2);
    assert_eq!(Verdict::Leave(Cause::Gateway).code(), LEAVE + 3);

    for verdict in [
        Verdict::Hold,
        Verdict::Probe,
        Verdict::Settled,
        Verdict::Leave(Cause::Link),
        Verdict::Leave(Cause::Lease),
        Verdict::Leave(Cause::Gateway),
    ] {
        assert_eq!(Verdict::from_code(verdict.code()), Some(verdict));
    }
    for code in [-1, 3, LEAVE - 1, LEAVE + 1, LEAVE + 4, i32::MIN] {
        assert_eq!(Verdict::from_code(code), None, "{code}");
    }

    for (code, probe) in [
        (0, Probe::None),
        (1, Probe::Answered),
        (2, Probe::Silent),
        (3, Probe::NoGateway),
        (-1, Probe::None),
        (99, Probe::None),
    ] {
        assert_eq!(Probe::from_code(code), probe, "{code}");
    }
}

#[test]
fn probes_at_once_and_then_every_period() {
    let mut link = Link::joined(Probe::Answered);

    assert_eq!(
        link.machine.step(link.now, true, true, Probe::None),
        Verdict::Probe
    );
    assert_eq!(
        link.machine.step(link.now, true, true, Probe::Answered),
        Verdict::Settled
    );
    assert!(link.machine.live());
    assert_eq!(
        link.machine.step(link.now + 1, true, true, Probe::None),
        Verdict::Hold
    );
    assert_eq!(
        link.machine
            .step(link.now + PROBE_PERIOD_MS - 1, true, true, Probe::None),
        Verdict::Hold
    );
    assert_eq!(
        link.machine
            .step(link.now + PROBE_PERIOD_MS, true, true, Probe::None),
        Verdict::Probe
    );
}

#[test]
fn a_network_without_internet_stays_joined() {
    // The bench hotspot: the gateway answers on the LAN and nothing beyond
    // it does, which the machine never asks about.
    let mut link = Link::joined(Probe::Answered);

    assert_eq!(link.run(3_600_000), None);
    assert_eq!(link.seen, [(10_000, Verdict::Settled)]);
    assert_eq!(link.probes, 3_600_000 / PROBE_PERIOD_MS);
    assert!(link.machine.live());
}

#[test]
fn a_gateway_that_never_answers_echo_settles_on_association_and_lease() {
    let mut link = Link::joined(Probe::Silent);

    assert_eq!(link.run(600_000), None);
    let settled = 10_000 + u32::from(GATEWAY_MISSES - 1) * PROBE_PERIOD_MS;
    assert_eq!(link.seen, [(settled, Verdict::Settled)]);
    assert!(link.machine.live());

    // It may start answering later, and is then watched like any other.
    link.gateway = Probe::Answered;
    assert_eq!(link.run(60_000), None);
    link.gateway = Probe::Silent;
    assert_eq!(link.run(60_000), Some(Cause::Gateway));
}

#[test]
fn a_lease_without_a_gateway_rests_on_association_and_lease() {
    let mut link = Link::joined(Probe::NoGateway);

    assert_eq!(link.run(600_000), None);
    assert_eq!(link.seen.len(), 1);
    assert!(link.machine.live());

    link.leased = false;
    assert_eq!(link.run(1_000), Some(Cause::Lease));
}

#[test]
fn a_silent_gateway_is_a_loss_after_the_misses() {
    let mut link = Link::joined(Probe::Answered);
    assert_eq!(link.run(60_000), None);

    link.gateway = Probe::Silent;
    let silent_from = link.now;
    let mut was_live = Vec::new();
    let cause = loop {
        let verdict = link.tick();
        was_live.push(link.machine.live());
        if let Verdict::Leave(cause) = verdict {
            break cause;
        }
    };

    assert_eq!(cause, Cause::Gateway);
    let took = link.now - silent_from;
    let window = u32::from(GATEWAY_MISSES) * PROBE_PERIOD_MS;
    assert!(
        took > window - PROBE_PERIOD_MS && took <= window,
        "{took} ms"
    );
    // Not live from the first miss on, so confirmation never counts silence.
    let first_miss = was_live.iter().position(|&live| !live).unwrap();
    assert!(was_live[first_miss..].iter().all(|&live| !live));
    assert!(!link.machine.live());
}

#[test]
fn flapping_answers_keep_the_link() {
    // Every other echo lost: never GATEWAY_MISSES in a row.
    let mut link = Link::joined(Probe::Answered);
    assert_eq!(link.run(10_000), None);

    for round in 0..200 {
        link.gateway = if round % 2 == 0 {
            Probe::Silent
        } else {
            Probe::Answered
        };
        assert_eq!(link.run(PROBE_PERIOD_MS), None, "round {round}");
    }

    // Two misses, then an answer, resets the count.
    for _ in 0..50 {
        link.gateway = Probe::Silent;
        assert_eq!(
            link.run(u32::from(GATEWAY_MISSES - 1) * PROBE_PERIOD_MS),
            None
        );
        link.gateway = Probe::Answered;
        assert_eq!(link.run(PROBE_PERIOD_MS), None);
    }
    assert!(link.machine.live());
}

#[test]
fn liveness_follows_the_latest_probe_once_answered() {
    let mut machine = Liveness::new();
    assert!(!machine.live(), "not live before it settles");

    assert_eq!(machine.step(0, true, true, Probe::None), Verdict::Probe);
    assert_eq!(
        machine.step(0, true, true, Probe::Answered),
        Verdict::Settled
    );
    assert!(machine.live());

    assert_eq!(
        machine.step(PROBE_PERIOD_MS, true, true, Probe::None),
        Verdict::Probe
    );
    assert_eq!(
        machine.step(PROBE_PERIOD_MS, true, true, Probe::Silent),
        Verdict::Hold
    );
    assert!(!machine.live(), "a missed echo is not known good");

    assert_eq!(
        machine.step(2 * PROBE_PERIOD_MS, true, true, Probe::None),
        Verdict::Probe
    );
    assert_eq!(
        machine.step(2 * PROBE_PERIOD_MS, true, true, Probe::Answered),
        Verdict::Hold
    );
    assert!(machine.live());
}

#[test]
fn an_association_drop_leaves_at_once() {
    let mut link = Link::joined(Probe::Answered);
    assert_eq!(link.run(20_000), None);

    link.associated = false;
    assert_eq!(link.tick(), Verdict::Leave(Cause::Link));
    assert!(!link.machine.live());

    // Before it settled, too.
    let mut machine = Liveness::new();
    assert_eq!(
        machine.step(0, false, true, Probe::None),
        Verdict::Leave(Cause::Link)
    );
}

#[test]
fn a_lost_lease_leaves_at_once_whatever_the_gateway_says() {
    let mut link = Link::joined(Probe::Answered);
    assert_eq!(link.run(20_000), None);

    link.leased = false;
    assert_eq!(link.tick(), Verdict::Leave(Cause::Lease));
    assert!(!link.machine.live());

    // A probe answered in the same step does not save it: the address is gone.
    let mut machine = Liveness::new();
    assert_eq!(
        machine.step(0, true, false, Probe::Answered),
        Verdict::Leave(Cause::Lease)
    );
    // The association is checked first.
    assert_eq!(
        Liveness::new().step(0, false, false, Probe::Answered),
        Verdict::Leave(Cause::Link)
    );
}

#[test]
fn the_clock_wrapping_does_not_stall_the_probes() {
    let mut link = Link::joined(Probe::Answered);
    link.now = u32::MAX - 20_000;

    assert_eq!(link.run(60_000), None);
    assert_eq!(link.probes, 60_000 / PROBE_PERIOD_MS);
}
