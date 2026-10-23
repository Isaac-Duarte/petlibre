use wellfed_hal::supervise::{Inputs, LIVENESS_MS, Startup, TRIAL_DEADLINE_MS, feed};

fn running(now_ms: u32, beat: u32) -> Inputs {
    Inputs {
        now_ms,
        startup: Startup::Running,
        startup_beat_ms: beat,
        ..Inputs::default()
    }
}

#[test]
fn startup_feeds_only_while_the_startup_task_steps() {
    assert!(feed(&running(5_000, 4_000)));
    assert!(!feed(&running(4_000 + LIVENESS_MS, 4_000)));
    // Outputs and other tasks do not matter before the role starts.
    assert!(feed(&Inputs {
        outputs_ok: false,
        ..running(100, 0)
    }));
}

#[test]
fn a_determined_startup_failure_resets_only_a_trial_image() {
    let failed = Inputs {
        startup: Startup::Failed,
        ..Inputs::default()
    };
    assert!(
        feed(&failed),
        "a confirmed image holds STOP without a reset loop"
    );
    assert!(
        !feed(&Inputs {
            trial: true,
            ..failed
        }),
        "a trial counts it"
    );
}

#[test]
fn running_feeds_while_tasks_step_and_outputs_stay_safe() {
    let ok = Inputs {
        now_ms: 60_000,
        startup: Startup::Started,
        tasks_alive: true,
        outputs_ok: true,
        ..Inputs::default()
    };
    assert!(feed(&ok));
    assert!(!feed(&Inputs {
        tasks_alive: false,
        ..ok
    }));
    assert!(!feed(&Inputs {
        outputs_ok: false,
        ..ok
    }));
}

#[test]
fn a_trial_image_must_confirm_before_the_deadline() {
    let trial = Inputs {
        now_ms: TRIAL_DEADLINE_MS - 1,
        startup: Startup::Started,
        trial: true,
        tasks_alive: true,
        outputs_ok: true,
        ..Inputs::default()
    };
    assert!(feed(&trial));
    assert!(!feed(&Inputs {
        now_ms: TRIAL_DEADLINE_MS,
        ..trial
    }));
    assert!(!feed(&Inputs {
        now_ms: TRIAL_DEADLINE_MS,
        startup: Startup::Running,
        startup_beat_ms: TRIAL_DEADLINE_MS,
        ..trial
    }));

    // Confirmed during this boot, or never on trial: no deadline.
    assert!(feed(&Inputs {
        now_ms: TRIAL_DEADLINE_MS * 3,
        confirmed: true,
        ..trial
    }));
    assert!(feed(&Inputs {
        now_ms: TRIAL_DEADLINE_MS * 3,
        trial: false,
        ..trial
    }));
}

/// The app stores the state in an `AtomicU8` and prints the raw code.
#[test]
fn startup_codes_round_trip() {
    for (state, code) in [
        (Startup::Running, 0),
        (Startup::Started, 1),
        (Startup::Failed, 2),
    ] {
        assert_eq!(state.code(), code);
        assert_eq!(Startup::from_code(code), state);
    }
    assert_eq!(Startup::from_code(3), Startup::Failed);
    assert_eq!(Startup::from_code(u8::MAX), Startup::Failed);
}
