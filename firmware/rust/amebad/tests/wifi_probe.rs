use petlibre_amebad::probes::wifi::{ON_TIMEOUT_MS, POLL_MS, Probe, Record, SCANS};
use petlibre_amebad::sdk::{WifiAp, fake};
use petlibre_amebad::stopped;

fn start() -> Probe {
    fake::reset_hardware();
    fake::with(|h| h.interrupts = 1);
    assert!(stopped::initialize().verified());
    let mut probe = Probe::default();
    assert_eq!(probe.step().record, Some(Record::Start { created: true }));
    probe
}

fn ap(n: u32) -> WifiAp {
    WifiAp {
        ssid_crc: 0x1000 + n,
        security: 0x0040_0004,
        rssi: -40 - n as i32,
        channel: 1 + n,
    }
}

/// Run until the probe logs something.
fn record(probe: &mut Probe) -> (u32, Option<Record>) {
    for _ in 0..1000 {
        let step = probe.step();
        if step.record.is_some() || step.delay_ms == 0 {
            return (step.delay_ms, step.record);
        }
    }
    panic!("no record");
}

#[test]
fn reports_radio_on_then_each_scan_and_its_networks() {
    let mut probe = start();
    assert_eq!(
        probe.step(),
        petlibre_amebad::probes::wifi::Step {
            delay_ms: POLL_MS,
            record: None
        }
    );
    fake::with(|h| {
        h.wifi.on_done = 1;
        h.wifi.on_ms = 1800;
    });
    assert_eq!(
        record(&mut probe).1,
        Some(Record::On {
            result: 0,
            ms: 1800
        })
    );
    for n in 1..=SCANS {
        fake::with(|h| {
            h.wifi.scans_done = n;
            h.wifi.scan_ms = 3000 + n;
            h.wifi.count = 2;
            h.wifi.aps[0] = ap(n);
            h.wifi.aps[1] = ap(n + 10);
        });
        assert_eq!(
            record(&mut probe).1,
            Some(Record::Scan {
                n,
                result: 0,
                ms: 3000 + n,
                count: 2,
                dropped: 0,
                stopped: true
            })
        );
        assert_eq!(
            record(&mut probe).1,
            Some(Record::Ap {
                n,
                index: 0,
                ap: ap(n)
            })
        );
        assert_eq!(
            record(&mut probe).1,
            Some(Record::Ap {
                n,
                index: 1,
                ap: ap(n + 10)
            })
        );
    }
    assert_eq!(record(&mut probe), (0, Some(Record::Done { scans: SCANS })));
    assert_eq!(probe.step().record, None);
}

#[test]
fn a_failed_radio_or_a_stalled_shim_ends_the_probe() {
    let mut probe = start();
    fake::with(|h| {
        h.wifi.on_done = 1;
        h.wifi.on_result = -1;
    });
    assert_eq!(
        record(&mut probe),
        (0, Some(Record::On { result: -1, ms: 0 }))
    );

    let mut probe = start();
    let (delay, last) = record(&mut probe);
    assert_eq!(delay, 0);
    assert!(matches!(last, Some(Record::Timeout { stage: 0, ms }) if ms > ON_TIMEOUT_MS));

    let mut probe = start();
    fake::with(|h| h.wifi.on_done = 1);
    assert!(matches!(record(&mut probe).1, Some(Record::On { .. })));
    assert!(matches!(
        record(&mut probe).1,
        Some(Record::Timeout { stage: 1, .. })
    ));
}

#[test]
fn a_second_start_is_refused() {
    let _ = start();
    let mut again = Probe::default();
    assert_eq!(
        again.step(),
        petlibre_amebad::probes::wifi::Step {
            delay_ms: 0,
            record: Some(Record::Start { created: false })
        }
    );
}
