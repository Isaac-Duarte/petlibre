//! The feeder's own values as text for a person: its status, settings,
//! plans, events and samples. Both paths print them, the device API's
//! commands and `wellfed server`'s (whose bodies are the same model
//! values), so a value reads alike on either; `--json` prints them as the
//! wire spells them instead.
//!
//! Words that name a value (`jammed`, `restarting`) are its JSON spelling,
//! so the text and the JSON can be matched by eye. A feed's portions read
//! through [`Delivered`](wellfed_client::history::Delivered), so an uncertain feed is a range here too, never a
//! count.

use std::fmt::Write as _;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_client::history::Feed;
use wellfed_model::{
    Battery, Blocked, EnrollPath, Event, EventBody, FeedFault, FoodLevel, MealReminder,
    MissedReason, Plans, Power, Pushed, Readiness, Sample, SampleValue, Settings, Source, Status,
    Supply, TimeOfDay,
};

/// The feeder's time in `zone`, with its offset so a line reads the same
/// on any computer.
pub(crate) fn time(at: Option<Timestamp>, zone: &TimeZone) -> String {
    at.map_or_else(
        || "(clock not set)".to_owned(),
        |at| {
            at.to_zoned(zone.clone())
                .strftime("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        },
    )
}

/// Who asked; nobody known for a feed a reset interrupted that no plan
/// had claimed.
fn source(by: Option<Source>) -> String {
    match by {
        Some(Source::Panel) => "panel".to_owned(),
        Some(Source::Client { client }) => format!("client {client}"),
        Some(Source::Feeder) => "feeder".to_owned(),
        Some(Source::Schedule { plan }) => format!("plan {plan}"),
        None => "unknown".to_owned(),
    }
}

/// One feed's number, source, portions and ending, as `wellfed feeds`, the
/// events and the feed's own result line all show it.
pub(crate) fn fed(feed: &Feed) -> String {
    let low = match feed.hopper {
        FoodLevel::Low => "  food read low",
        FoodLevel::Unread | FoodLevel::Present => "",
    };
    format!(
        "feed {}  by {}  {} ({}){low}",
        feed.feed,
        source(feed.by),
        feed.delivered.describe(feed.requested, feed.outcome),
        feed.reason.code()
    )
}

/// The status, a line a topic. A field the feeder did not report reads
/// `not reported`.
pub(crate) fn status(status: &Status, zone: &TimeZone) -> String {
    let clock = match status.clock {
        Some(at) => time(Some(at), zone),
        None => "not set: scheduled meals hold until a sync".to_owned(),
    };
    let panel = match status.locked {
        Some(true) => "locked",
        Some(false) => "unlocked",
        None => "not reported",
    };
    let power = match (status.supply, status.power) {
        (Some(supply), _) => self::supply(supply),
        (None, Some(power)) => self::power(power),
        (None, None) => "not reported".to_owned(),
    };

    let mut text = format!(
        "Build {} on bank {}, boot {}, up {}\n\
         Clock: {clock}\n\
         Feeding: {}\n\
         Food: {}\n",
        status.build,
        status.bank,
        status.boot_count,
        uptime(status.uptime_ms),
        readiness(status.readiness),
        food(status.food),
    );
    if let Some(fault) = status.feed_fault {
        // Writing to a String cannot fail.
        let _ = writeln!(text, "Last fault: {}", feed_fault(fault));
    }
    let _ = write!(
        text,
        "Power: {power}\n\
         Desiccant: {}\n\
         Panel: {panel}\n\
         Wi-Fi: {} dBm ({} credentials), {} link losses, {} gateway losses\n\
         Sessions {}, settings generation {}, Wi-Fi generation {}, last event {}\n\
         Faults {}, reset flags {:#x}",
        status.desiccant,
        status.rssi,
        word(&status.wifi),
        status.link_losses,
        status.dns_losses,
        status.sessions,
        status.settings_generation,
        status.wifi_generation,
        status
            .last_event
            .map_or_else(|| "none".to_owned(), |seq| seq.to_string()),
        status.faults,
        status.reset_flags,
    );
    text
}

/// Whether a feed can start now, and if not why.
fn readiness(readiness: Option<Readiness>) -> String {
    match readiness {
        Some(Readiness::Ready) => "ready".to_owned(),
        Some(Readiness::Blocked(reason)) => {
            format!("blocked, {} ({})", reason.code(), blocked(reason))
        }
        None => "not reported".to_owned(),
    }
}

/// Why a feed cannot start, in words.
const fn blocked(reason: Blocked) -> &'static str {
    match reason {
        Blocked::Starting => "the feeding owner has not opened yet",
        Blocked::Journal => "the feed journal could not be opened: no feed this boot",
        Blocked::Fault => "stopped on a fault: no feed until a restart",
        Blocked::Resolving => "an interrupted feed is being resolved",
        Blocked::Feeding => "a feed is queued or running",
        Blocked::Joining => "a Wi-Fi join runs",
        Blocked::Updating => "an install holds the flash",
        Blocked::Setup => "BLE setup mode runs",
        Blocked::Unserved => "this firmware does not feed",
        Blocked::Battery => {
            "on batteries below the floor a feed needs or not yet read, or after a brownout"
        }
        Blocked::Restarting => "a restart is coming within seconds",
    }
}

/// The hopper as the status reports it: the food beam's latest reading, from
/// a feed or a short window at rest.
fn food(level: FoodLevel) -> &'static str {
    match level {
        FoodLevel::Unread => "not read yet (the food beam reads soon after a boot, on the adapter)",
        FoodLevel::Present => "present at the last reading",
        FoodLevel::Low => "low at the last reading",
    }
}

/// The last feed that ended on a fault. The status carries no portion
/// count, so none is shown: the feed's history line has it.
fn feed_fault(fault: FeedFault) -> String {
    format!(
        "feed {} {} ({}); `wellfed feeds` has its portions",
        fault.feed,
        fault.outcome.code(),
        fault.reason.code()
    )
}

/// The supply as the feeder judged it.
fn supply(supply: Supply) -> String {
    let adapter = match supply.adapter {
        Some(true) => "adapter",
        Some(false) => "on batteries (Wi-Fi off 5 s after the adapter goes)",
        None => "adapter not sampled",
    };
    let floor = match supply.below_floor {
        Some(true) => ", pack below the floor a feed on batteries needs",
        Some(false) | None => "",
    };
    format!("{adapter}; {}{floor}", battery(supply.battery))
}

fn battery(battery: Battery) -> String {
    match battery {
        Battery::Absent => "no batteries".to_owned(),
        Battery::Reading { mv, percent } => format!("batteries {percent}% ({mv} mV)"),
        Battery::Implausible { mv } => format!("battery reading implausible ({mv} mV)"),
        Battery::Unread => "batteries not read".to_owned(),
        Battery::Stale { age_ms } => format!("battery reading stale ({} s old)", age_ms / 1000),
    }
}

/// The supply from a feeder that reports only the raw power reading.
fn power(power: Power) -> String {
    let adapter = if power.adapter {
        "adapter"
    } else {
        "on batteries"
    };
    match power.battery_mv {
        Some(mv) => format!("{adapter}; batteries {mv} mV"),
        None => adapter.to_owned(),
    }
}

/// Milliseconds of uptime as days, hours, minutes and seconds.
fn uptime(ms: u32) -> String {
    let seconds = ms / 1000;
    let (days, hours, minutes, seconds) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    match (days, hours) {
        (0, 0) => format!("{minutes} min {seconds} s"),
        (0, _) => format!("{hours} h {minutes} min"),
        _ => format!("{days} d {hours} h"),
    }
}

pub(crate) fn settings(settings: &Settings) -> String {
    format!(
        "Generation {}: lights {} ({}), sound {} ({}), volume {}, meal reminder {}, desiccant \
         every {} days (replaced {}){}",
        settings.generation,
        on_off(settings.lights),
        settings.lights_window,
        on_off(settings.sound),
        settings.sound_window,
        settings.volume,
        reminder(settings.meal_reminder),
        settings.desiccant_days,
        settings
            .desiccant_replaced
            .map_or_else(|| "-".to_owned(), |date| date.to_string()),
        if settings.writable {
            ""
        } else {
            " (not writable now)"
        },
    )
}

fn reminder(reminder: MealReminder) -> String {
    match reminder {
        MealReminder::Off => "off".to_owned(),
        MealReminder::Before { minutes } => format!("{minutes} min ahead"),
    }
}

const fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Each plan, a line a filled slot.
pub(crate) fn plans(plans: &Plans) -> String {
    let mut text = format!("Plans, generation {}", plans.generation);
    if !plans.writable {
        text.push_str(" (not writable now)");
    }
    let mut any = false;
    for (slot, plan) in plans.plans.iter().enumerate() {
        let Some(plan) = plan else {
            continue;
        };
        any = true;
        let call = match plan.call {
            0 => "no call".to_owned(),
            1 => "call once".to_owned(),
            plays => format!("call {plays} times"),
        };
        let _ = write!(
            text,
            "\n  {slot}  {}  {} portions  {}  {call}  {}",
            TimeOfDay(plan.minute),
            plan.portions,
            plan.days,
            if plan.enabled { "enabled" } else { "disabled" },
        );
    }
    if !any {
        text.push_str("\n  no plans");
    }
    text
}

/// One pushed message as a line: an event with its number and time, a
/// gap, a sample or the log's lines.
pub(crate) fn pushed(pushed: &Pushed, zone: &TimeZone) -> String {
    match pushed {
        Pushed::Event(event) => format!(
            "{:>6}  {}  {}",
            event.seq,
            time(event.at, zone),
            self::event(event, zone)
        ),
        Pushed::Gap(gap) => format!(
            "events {}-{} left the feeder's history before they were read",
            gap.from, gap.to
        ),
        Pushed::Sample(sample) => self::sample(sample),
        Pushed::Log(batch) => batch.lines.join("\n"),
    }
}

/// What an event says, without its number or time.
pub(crate) fn event(event: &Event, zone: &TimeZone) -> String {
    match event.body {
        EventBody::Boot { bank, reset_flags } => {
            format!("booted from bank {bank} (reset flags {reset_flags:#x})")
        }
        EventBody::Lock { locked, by } => format!(
            "panel {} by {}",
            if locked { "locked" } else { "unlocked" },
            source(Some(by))
        ),
        EventBody::Settings { generation, by } => {
            format!(
                "settings changed to generation {generation} by {}",
                source(Some(by))
            )
        }
        EventBody::Wifi { generation, by } => format!(
            "Wi-Fi credentials changed to generation {generation} by {}",
            source(Some(by))
        ),
        EventBody::Enrolled { client, role, path } => {
            let path = match path {
                EnrollPath::Setup => "setup",
                EnrollPath::Invite => "an invite",
                EnrollPath::Recovery => "recovery",
            };
            format!("client {client} enrolled as {} through {path}", role.code())
        }
        EventBody::Revoked { client, by } => {
            format!("client {client} revoked by {}", source(Some(by)))
        }
        EventBody::ClockSync { by, jump_ms } => {
            let jump = jump_ms.map_or_else(
                || "it was not set".to_owned(),
                |ms| format!("moved {ms} ms"),
            );
            format!("clock set by {} ({jump})", source(Some(by)))
        }
        EventBody::Lost { count } => {
            format!("the feeder could not record {count} events; feeds among them are gone")
        }
        EventBody::Fed { .. } => {
            Feed::from_event(event).map_or_else(String::new, |feed| fed(&feed))
        }
        EventBody::FeedingBlocked {
            reason: Some(reason),
        } => {
            format!("feeding blocked, {} ({})", reason.code(), blocked(reason))
        }
        EventBody::FeedingBlocked { reason: None } => "feeding ready again".to_owned(),
        EventBody::Plans { generation, by } => {
            format!(
                "plans changed to generation {generation} by {}",
                source(Some(by))
            )
        }
        EventBody::Missed { plan, due, reason } => {
            let why = match reason {
                MissedReason::Late => "more than 15 minutes late".to_owned(),
                MissedReason::Blocked { reason } => {
                    format!("blocked for 15 minutes, {}", reason.code())
                }
                MissedReason::Interrupted => "interrupted before its feed was reserved".to_owned(),
            };
            format!(
                "plan {plan}'s meal due {} missed: {why}",
                time(Some(due), zone)
            )
        }
        EventBody::MealSoon {
            plan,
            due,
            minute,
            portions,
        } => format!(
            "meal soon: plan {plan}, {portions} portions at {} ({})",
            TimeOfDay(minute),
            time(Some(due), zone)
        ),
        EventBody::Supply {
            adapter,
            battery,
            below_floor,
        } => format!(
            "supply: {}",
            supply(Supply {
                adapter,
                battery,
                below_floor,
            })
        ),
        EventBody::DispenseRefused { reason } => format!(
            "Dispense pressed while feeding was blocked, {}: nothing started",
            reason.code()
        ),
        EventBody::Food { level } => format!("hopper {}", word(&level)),
    }
}

/// A live sample, with the feeder's uptime when it measured.
fn sample(sample: &Sample) -> String {
    let value = match sample.value {
        SampleValue::Power {
            adapter,
            battery_mv,
        } => format!(
            "power: {}",
            power(Power {
                adapter,
                battery_mv
            })
        ),
        SampleValue::Rssi { dbm } => format!("Wi-Fi signal {dbm} dBm"),
        SampleValue::Heap { free, min } => format!("heap {free} B free (fewest {min} B)"),
        SampleValue::Loop {
            passes,
            max_us,
            p99_us,
        } => format!("loop {passes} passes, longest {max_us} us, 99th percentile {p99_us} us"),
        SampleValue::Gateway { round_trip_ms } => match round_trip_ms {
            Some(ms) => format!("gateway round trip {ms} ms"),
            None => "gateway did not answer".to_owned(),
        },
        SampleValue::Wifi {
            channel,
            ap_changes,
        } => format!(
            "Wi-Fi channel {}, {ap_changes} access point changes",
            channel.map_or_else(|| "-".to_owned(), |channel| channel.to_string())
        ),
        SampleValue::Supply {
            adapter,
            battery,
            below_floor,
        } => format!(
            "supply: {}",
            supply(Supply {
                adapter,
                battery,
                below_floor,
            })
        ),
    };
    format!("sample at {}: {value}", uptime(sample.uptime_ms))
}

/// A value as the wire spells it: a bare word for an enum's name, its JSON
/// otherwise.
pub(crate) fn word(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        Ok(other) => other.to_string(),
        Err(_) => "?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use wellfed_model::{
        Desiccant, FeedOutcome, FeedReason, Gap, Generation, Hex, PLANS, Plan, Weekdays,
        WifiSource, Window,
    };

    use super::*;

    fn status() -> Status {
        Status {
            build: "feeder 345".to_owned(),
            bank: 1,
            uptime_ms: 3_725_000,
            boot_count: 42,
            reset_flags: 0,
            faults: 0,
            rssi: -54,
            wifi: WifiSource::Stored,
            link_losses: 1,
            dns_losses: 0,
            clock: None,
            power: None,
            locked: Some(false),
            settings_generation: Generation(7),
            wifi_generation: Generation(3),
            last_event: Some(1175),
            sessions: 1,
            food: FoodLevel::Low,
            feed_fault: Some(FeedFault {
                feed: 128,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Interrupted,
            }),
            readiness: Some(Readiness::Blocked(Blocked::Restarting)),
            desiccant: Desiccant::Remaining {
                days: -3,
                due: true,
            },
            supply: Some(Supply {
                adapter: Some(false),
                battery: Battery::Reading {
                    mv: 4_310,
                    percent: 10,
                },
                below_floor: Some(true),
            }),
        }
    }

    #[test]
    fn the_status_reads_its_newer_fields_in_words() {
        let text = super::status(&status(), &TimeZone::UTC);

        for line in [
            "Build feeder 345 on bank 1, boot 42, up 1 h 2 min",
            "Clock: not set: scheduled meals hold until a sync",
            "Feeding: blocked, restarting (a restart is coming within seconds)",
            "Food: low at the last reading",
            "Last fault: feed 128 uncertain (interrupted); `wellfed feeds` has its portions",
            "Power: on batteries (Wi-Fi off 5 s after the adapter goes); batteries 10% (4310 mV), \
             pack below the floor a feed on batteries needs",
            "Desiccant: due (3 days over)",
            "Panel: unlocked",
            "Wi-Fi: -54 dBm (stored credentials), 1 link losses, 0 gateway losses",
        ] {
            assert!(text.lines().any(|l| l == line), "{line:?} not in\n{text}");
        }
    }

    #[test]
    fn a_status_without_a_fault_or_supply_says_so() {
        let mut quiet = status();
        quiet.feed_fault = None;
        quiet.supply = None;
        quiet.readiness = None;
        quiet.clock = Some(Timestamp::from_second(1_793_100_000).unwrap());

        let text = super::status(&quiet, &TimeZone::UTC);

        assert!(!text.contains("Last fault"), "{text}");
        assert!(text.contains("Power: not reported"), "{text}");
        assert!(text.contains("Feeding: not reported"), "{text}");
        assert!(text.contains("Clock: 2026-10-27 11:20:00 +00:00"), "{text}");
    }

    fn event(body: EventBody) -> String {
        let event = Event {
            epoch: 1,
            seq: 9,
            at: None,
            body,
        };
        super::event(&event, &TimeZone::UTC)
    }

    #[test]
    fn events_read_as_words() {
        let due = Timestamp::from_second(1_793_100_000).unwrap();

        assert_eq!(
            event(EventBody::MealSoon {
                plan: 3,
                due,
                minute: 11 * 60 + 20,
                portions: 2
            }),
            "meal soon: plan 3, 2 portions at 11:20 (2026-10-27 11:20:00 +00:00)"
        );
        assert_eq!(
            event(EventBody::Missed {
                plan: 0,
                due,
                reason: MissedReason::Blocked {
                    reason: Blocked::Battery
                }
            }),
            "plan 0's meal due 2026-10-27 11:20:00 +00:00 missed: blocked for 15 minutes, battery"
        );
        assert_eq!(
            event(EventBody::FeedingBlocked {
                reason: Some(Blocked::Restarting)
            }),
            "feeding blocked, restarting (a restart is coming within seconds)"
        );
        assert_eq!(
            event(EventBody::FeedingBlocked { reason: None }),
            "feeding ready again"
        );
        assert_eq!(
            event(EventBody::Food {
                level: FoodLevel::Low
            }),
            "hopper low"
        );
        assert_eq!(
            event(EventBody::Supply {
                adapter: Some(true),
                battery: Battery::Absent,
                below_floor: Some(false)
            }),
            "supply: adapter; no batteries"
        );
        assert_eq!(
            event(EventBody::DispenseRefused {
                reason: Blocked::Feeding
            }),
            "Dispense pressed while feeding was blocked, feeding: nothing started"
        );
        assert_eq!(
            event(EventBody::Lock {
                locked: true,
                by: Source::Client {
                    client: Hex([0xab, 0xcd, 1, 2])
                }
            }),
            "panel locked by client abcd0102"
        );
    }

    #[test]
    fn an_uncertain_feed_event_reads_as_a_range() {
        let text = event(EventBody::Fed {
            feed: 130,
            requested: 3,
            completed: 0,
            outcome: FeedOutcome::Uncertain,
            reason: FeedReason::Interrupted,
            hopper: FoodLevel::Unread,
            by: Source::Feeder,
        });

        assert_eq!(
            text,
            "feed 130  by unknown  0 to 3 of 3 portions, uncertain (interrupted)"
        );
    }

    #[test]
    fn a_pushed_event_carries_its_number_and_time() {
        let food = Pushed::Event(Event {
            epoch: 1,
            seq: 1175,
            at: None,
            body: EventBody::Food {
                level: FoodLevel::Present,
            },
        });
        let gap = Pushed::Gap(Gap {
            epoch: 1,
            from: 1,
            to: 40,
        });

        assert_eq!(
            pushed(&food, &TimeZone::UTC),
            "  1175  (clock not set)  hopper present"
        );
        assert_eq!(
            pushed(&gap, &TimeZone::UTC),
            "events 1-40 left the feeder's history before they were read"
        );
    }

    #[test]
    fn settings_and_plans_read_as_lines() {
        let settings = Settings {
            generation: Generation(7),
            lights: true,
            sound: false,
            volume: 40,
            lights_window: Window::Always,
            sound_window: Window::Between {
                start: 8 * 60,
                end: 21 * 60,
            },
            desiccant_days: 14,
            desiccant_replaced: None,
            meal_reminder: MealReminder::Before { minutes: 5 },
            writable: true,
        };
        assert_eq!(
            super::settings(&settings),
            "Generation 7: lights on (always), sound off (08:00-21:00), volume 40, meal reminder \
             5 min ahead, desiccant every 14 days (replaced -)"
        );

        let mut table = [None; PLANS];
        table[2] = Some(Plan {
            minute: 7 * 60 + 30,
            portions: 2,
            days: Weekdays::DAILY,
            enabled: false,
            call: 3,
        });
        table[5] = Some(Plan {
            minute: 18 * 60,
            portions: 1,
            days: Weekdays::ONCE,
            enabled: true,
            call: 0,
        });
        let plans = Plans {
            generation: Generation(4),
            writable: true,
            plans: table,
        };
        assert_eq!(
            super::plans(&plans),
            "Plans, generation 4\n  2  07:30  2 portions  daily  call 3 times  disabled\n  5  \
             18:00  1 portions  once  no call  enabled"
        );
        let empty = Plans {
            plans: [None; PLANS],
            ..plans
        };
        assert_eq!(super::plans(&empty), "Plans, generation 4\n  no plans");
    }
}
