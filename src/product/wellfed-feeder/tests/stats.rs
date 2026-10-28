//! The statistics keeper: the loop metric's percentile, the supply's
//! outages and battery time, the food beam, the gateway metric and the
//! session counts, from what a loop pass hands it.

use wellfed_feeder::feeder::platform::Link;
use wellfed_feeder::stats::{HISTORY_MS, Keeper, TOTAL_SECTORS, Totals, reset_cause};
use wellfed_proto::api::{
    Counts, Erases, FLASH_SECTORS, FoodLevel, Metric, POWER_HISTORY, Power, ResetCause, Resets,
};

fn power(adapter: bool, battery_mv: Option<u32>) -> Power {
    Power {
        adapter,
        battery_mv,
    }
}

#[test]
fn the_loop_metric_reports_the_window_and_starts_a_new_one() {
    let mut keeper = Keeper::new();
    for _ in 0..990 {
        keeper.pass_took(200);
    }
    for _ in 0..10 {
        keeper.pass_took(5_000);
    }
    keeper.pass_took(27_400);

    let Metric::Loop {
        passes,
        max_us,
        p99_us,
    } = keeper.loop_metric()
    else {
        panic!("a loop metric");
    };
    assert_eq!((passes, max_us), (1_001, 27_400));
    // 1% of passes is 10: the 99th percentile is in the 5 ms bucket, an
    // upper bound within an eighth of an octave.
    assert!((5_000..5_000 + 5_000 / 8 + 1).contains(&p99_us), "{p99_us}");

    assert_eq!(
        keeper.loop_metric(),
        Metric::Loop {
            passes: 0,
            max_us: 0,
            p99_us: 0
        },
        "each window starts empty"
    );
}

#[test]
fn a_steady_loop_reports_its_own_time_as_the_percentile() {
    let mut keeper = Keeper::new();
    for _ in 0..100 {
        keeper.pass_took(3);
    }
    assert_eq!(
        keeper.loop_metric(),
        Metric::Loop {
            passes: 100,
            max_us: 3,
            p99_us: 3
        }
    );
}

#[test]
fn outages_and_battery_time_follow_the_adapter() {
    let mut keeper = Keeper::new();
    keeper.power(0, power(true, Some(4_800)));
    keeper.power(5_000, power(false, Some(4_700)));
    keeper.power(10_000, power(false, Some(4_650)));
    keeper.power(15_000, power(true, Some(4_790)));
    keeper.power(20_000, power(true, Some(4_800)));

    let stats = keeper.power_stats();
    assert_eq!(stats.adapter, Some(true));
    assert_eq!((stats.adapter_changes, stats.outages), (2, 1));
    assert_eq!(stats.battery_s, 10, "the two sampled intervals on battery");
    assert_eq!(
        (stats.battery_min_mv, stats.battery_max_mv),
        (Some(4_650), Some(4_800))
    );
    assert_eq!(keeper.counts(20_000).outages, 1);
    assert_eq!(keeper.counts(20_000).battery_s, 10);
}

#[test]
fn the_battery_history_takes_one_point_an_hour_newest_last() {
    let mut keeper = Keeper::new();
    keeper.power(0, power(true, Some(4_800)));
    keeper.tick(HISTORY_MS - 1);
    assert!(keeper.power_stats().battery_mv.iter().all(Option::is_none));

    keeper.tick(HISTORY_MS);
    keeper.power(HISTORY_MS + 1, power(true, Some(4_700)));
    keeper.tick(2 * HISTORY_MS);
    let history = keeper.power_stats().battery_mv;
    assert_eq!(history[POWER_HISTORY - 2..], [Some(4_800), Some(4_700)]);
    assert_eq!(keeper.power_stats().period_s, 3_600);
}

#[test]
fn the_food_beam_counts_its_transitions_and_feeds_since_a_refill() {
    let mut keeper = Keeper::new();
    let fed = wellfed_proto::api::EventBody::Fed {
        feed: 1,
        requested: 1,
        completed: 1,
        outcome: wellfed_proto::api::FeedOutcome::Done,
        reason: wellfed_proto::api::FeedReason::Done,
        hopper: FoodLevel::Present,
        by: wellfed_proto::api::Source::Panel,
    };
    keeper.food(true);
    keeper.fed(&fed);
    assert_eq!(
        keeper.food_stats().feeds_since_refill,
        None,
        "no refill seen"
    );

    keeper.food(false);
    keeper.food(false);
    keeper.food(true);
    keeper.fed(&fed);
    keeper.fed(&fed);
    let food = keeper.food_stats();
    assert_eq!(food.present, Some(true));
    assert_eq!((food.emptied, food.refilled), (1, 1));
    assert_eq!(food.feeds_since_refill, Some(2));
    assert_eq!((keeper.counts(0).feeds, keeper.counts(0).portions), (3, 3));
}

#[test]
fn the_gateway_metric_names_only_an_answer_since_the_last() {
    let mut keeper = Keeper::new();
    let link = |answers, ms| Link {
        gateway_answers: answers,
        gateway_ms: ms,
        channel: Some(11),
        ap_changes: 1,
        link_losses: 2,
        check_losses: 1,
        dhcp_renewals: 4,
        ..Link::default()
    };
    keeper.link(link(3, 7));
    assert_eq!(
        keeper.gateway_metric(),
        Metric::Gateway {
            round_trip_ms: Some(7)
        }
    );
    assert_eq!(
        keeper.gateway_metric(),
        Metric::Gateway {
            round_trip_ms: None
        },
        "no probe answered since"
    );
    assert_eq!(
        keeper.wifi_metric(),
        Metric::Wifi {
            channel: Some(11),
            ap_changes: 1
        }
    );
    let counts = keeper.counts(0);
    assert_eq!((counts.reconnects, counts.dhcp_renewals), (3, 4));
}

#[test]
fn the_reset_cause_comes_from_the_rom_flags() {
    for (flags, cause) in [
        (0, ResetCause::PowerOn),
        (0x0010_0000, ResetCause::Watchdog),
        (0x0002_0000, ResetCause::Watchdog),
        (0x0008_0000, ResetCause::Software),
        (0x0004_0000 | 0x0010_0000, ResetCause::Brownout),
        (0x0020_0000, ResetCause::Other),
    ] {
        assert_eq!(reset_cause(flags), cause, "{flags:#x}");
    }
    let mut keeper = Keeper::new();
    keeper.boot(0x0010_0000);
    assert_eq!(keeper.counters(0, None).reset, ResetCause::Watchdog);
}

#[test]
fn the_uptime_counts_past_the_millisecond_clock_s_wrap() {
    let mut keeper = Keeper::new();
    keeper.tick(u32::MAX - 500);
    keeper.tick(1_500);
    assert_eq!(keeper.counts(1_500).uptime_s, 4_294_968);
}

#[test]
fn sessions_count_with_their_handshake_times() {
    let mut keeper = Keeper::new();
    keeper.opened(40);
    keeper.opened(140);
    keeper.presses(5);
    let counters = keeper.counters(1_000, None);
    assert_eq!((counters.boot.sessions, counters.boot.presses), (2, 5));
    assert_eq!(
        (counters.handshake_max_ms, counters.handshake_mean_ms),
        (140, 90)
    );
}

/// The lifetime figures are the stored totals plus this boot's: the
/// counts, the resets as stored (this boot's already counted), and the
/// erases of the sectors the wire lists; none without a store.
#[test]
fn lifetime_figures_add_this_boot_to_the_stored_totals() {
    let mut keeper = Keeper::new();
    keeper.opened(40);
    keeper.presses(2);
    let mut stored = Totals::ZERO.plus(
        &Counts {
            uptime_s: 7_200,
            sessions: 10,
            presses: u32::MAX,
            ..Counts::default()
        },
        &[3; TOTAL_SECTORS],
        5,
    );
    stored.count_boot(ResetCause::Brownout);

    let counters = keeper.counters(61_000, Some(&stored));
    let lifetime = counters.lifetime.unwrap();
    assert_eq!(
        (lifetime.uptime_s, lifetime.sessions, lifetime.presses),
        (7_261, 11, u32::MAX),
        "summed, held at the top"
    );
    assert_eq!(
        counters.resets,
        Some(Resets {
            brownout: 1,
            ..Resets::default()
        })
    );
    assert_eq!(keeper.counters(61_000, None).lifetime, None);

    let mut boot = [0; FLASH_SECTORS];
    boot[13] = 4;
    let boot = Erases {
        storage: boot,
        update: 2,
    };
    let flash = Keeper::flash(boot, Some(&stored), 1, 2);
    let lifetime = flash.lifetime.unwrap();
    assert_eq!((lifetime.storage[13], lifetime.storage[0]), (7, 3));
    assert_eq!(lifetime.update, 7);
    assert_eq!(flash.boot, boot);
    assert_eq!(Keeper::flash(boot, None, 1, 2).lifetime, None);
}
