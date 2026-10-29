//! `wellfed-bench note`: the operator's notes, posted to the server, which
//! keeps them in `private/bench-status/notes.jsonl` and shows them on the
//! page and the console. A timer that expects a UART or device log line,
//! or a device event, is settled by the server as it arrives, or marked
//! missed once its window passes.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::Context;
use clap::{ArgGroup, Args, Subcommand};
use jiff::{SignedDuration, Timestamp, Zoned, civil};
use wellfed_bench_server::Client;
use wellfed_formats::bench::notes::{Body, Expect, Note, Verdict, WINDOW};

#[derive(Debug, Args)]
pub(crate) struct NoteArgs {
    #[command(subcommand)]
    what: What,
}

#[derive(Debug, Subcommand)]
enum What {
    /// The step under way, replacing the last.
    Step { text: String },
    /// The steps after it, replacing the list.
    Next { steps: Vec<String> },
    /// Start a timer, or clear one; with --expect or --expect-event the
    /// server marks it seen when that arrives, or missed once its window
    /// passes.
    Timer(TimerArgs),
    /// Record evidence for a card, with its verdict.
    Evidence(EvidenceArgs),
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("when").args(["at", "after", "clear"]).required(true)))]
pub(crate) struct TimerArgs {
    name: String,
    /// Due at this local time, the next time it comes.
    #[arg(long, value_name = "HH:MM")]
    at: Option<String>,
    /// Due this long from now, such as `90s`, `5m` or `1h30m`.
    #[arg(long = "in", value_name = "DURATION")]
    after: Option<String>,
    /// Drop the timer.
    #[arg(long)]
    clear: bool,
    /// A UART or device log line containing this text, such as `PLSCH1 fire`.
    #[arg(long, conflicts_with_all = ["expect_event", "clear"])]
    expect: Option<String>,
    /// A device event of this kind, such as `boot` or `fed`.
    #[arg(long, value_name = "KIND", conflicts_with = "clear")]
    expect_event: Option<String>,
    /// How long after the due time the expectation may still arrive
    /// (default 2m); it counts from the timer's start.
    #[arg(long, value_name = "DURATION", conflicts_with = "clear")]
    window: Option<String>,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("verdict").args(["pass", "fail"]).required(true)))]
pub(crate) struct EvidenceArgs {
    /// The work the evidence is for, in the operator's own words.
    #[arg(long)]
    card: String,
    /// A file or directory inside private/.
    path: PathBuf,
    #[arg(long)]
    pass: bool,
    #[arg(long)]
    fail: bool,
    /// What it shows.
    #[arg(long, default_value = "")]
    text: String,
}

/// `note`: post it, and say what the server kept.
pub(crate) async fn note(client: &Client, root: &Path, args: NoteArgs) -> anyhow::Result<u8> {
    let cwd = std::env::current_dir()?;
    let body = body(args, root, &cwd, &Zoned::now())?;

    let note = client.note(&body).await?;
    wellfed_rig::say!("Noted: {}", describe(&note));
    Ok(0)
}

/// The note `args` asks for, as of `now`, with a path taken from `cwd`.
fn body(args: NoteArgs, root: &Path, cwd: &Path, now: &Zoned) -> anyhow::Result<Body> {
    Ok(match args.what {
        What::Step { text } => Body::Step { text },
        What::Next { steps } => Body::Next { steps },
        What::Timer(timer) => timer_body(timer, now)?,
        What::Evidence(evidence) => Body::Evidence {
            card: evidence.card,
            verdict: if evidence.pass {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
            text: evidence.text,
            path: Some(inside_private(root, &cwd.join(&evidence.path))?),
        },
    })
}

fn timer_body(args: TimerArgs, now: &Zoned) -> anyhow::Result<Body> {
    if args.clear {
        return Ok(Body::Clear { label: args.name });
    }

    let due = match (&args.at, &args.after) {
        (Some(at), _) => next_at(at, now)?,
        (None, Some(after)) => now.timestamp() + duration("--in", after)?,
        (None, None) => anyhow::bail!("--at, --in or --clear says when"),
    };
    let window = match &args.window {
        Some(window) => duration("--window", window)?,
        None => SignedDuration::from_secs_f64(WINDOW),
    };
    let expect = match (args.expect, args.expect_event) {
        (Some(text), _) => Some(Expect::Line { text }),
        (None, Some(kind)) => Some(Expect::Event { kind }),
        (None, None) => None,
    };
    let due = seconds(due);
    Ok(Body::Timer {
        label: args.name,
        due,
        until: due + window.as_secs_f64(),
        expect,
    })
}

/// The next time the local clock reads `at`: today, or tomorrow once it
/// has passed.
fn next_at(at: &str, now: &Zoned) -> anyhow::Result<Timestamp> {
    let time = civil::Time::from_str(at)
        .with_context(|| format!("--at {at:?}: not a time such as 14:30"))?;
    let mut day = now.date();
    loop {
        let due = day
            .to_datetime(time)
            .to_zoned(now.time_zone().clone())
            .with_context(|| format!("--at {at}"))?;
        if due.timestamp() > now.timestamp() {
            return Ok(due.timestamp());
        }
        day = day.tomorrow().with_context(|| format!("--at {at}"))?;
    }
}

/// A non-negative length such as `90s`, `5m` or `1h30m`.
fn duration(flag: &str, text: &str) -> anyhow::Result<SignedDuration> {
    let length = SignedDuration::from_str(text)
        .with_context(|| format!("{flag} {text:?}: not a length such as 90s, 5m or 1h30m"))?;
    anyhow::ensure!(!length.is_negative(), "{flag} {text}: must not be negative");
    Ok(length)
}

fn seconds(at: Timestamp) -> f64 {
    at.as_duration().as_secs_f64()
}

/// `path` as the server keeps it: root-relative, inside `private/`.
fn inside_private(root: &Path, path: &Path) -> anyhow::Result<String> {
    let path = path
        .canonicalize()
        .with_context(|| format!("{}", path.display()))?;
    let private = root.join("private");
    let inside = path
        .strip_prefix(root)
        .ok()
        .filter(|_| path.starts_with(&private) && path != private);
    let Some(relative) = inside else {
        anyhow::bail!(
            "{} is not inside {}: evidence stays in private/",
            path.display(),
            private.display()
        );
    };
    Ok(relative.display().to_string())
}

/// One line for what was noted.
pub(crate) fn describe(note: &Note) -> String {
    match &note.body {
        Body::Step { text } => format!("step {text:?}"),
        Body::Next { steps } => format!("next steps {}", steps.join(", ")),
        Body::Timer {
            label,
            due,
            until,
            expect,
        } => {
            let due = clock(*due);
            match expect {
                Some(expect) => format!(
                    "timer {label} due {due}, expecting {expect} until {}",
                    clock(*until)
                ),
                None => format!("timer {label} due {due}"),
            }
        }
        Body::Clear { label } => format!("timer {label} cleared"),
        Body::Seen { label, line } => format!("timer {label} saw {line:?}"),
        Body::Missed { label } => format!("timer {label} missed"),
        Body::Evidence {
            card,
            verdict,
            path,
            ..
        } => {
            let verdict = match verdict {
                Verdict::Pass => "pass",
                Verdict::Fail => "fail",
            };
            let path = path.as_deref().unwrap_or("no file");
            format!("{card} {verdict} ({path})")
        }
    }
}

/// `at`, Unix seconds, as the bench host's local wall clock.
pub(crate) fn clock(at: f64) -> String {
    let Ok(at) = Timestamp::from_duration(SignedDuration::from_secs_f64(at)) else {
        return format!("{at:.0}");
    };
    at.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%H:%M:%S")
        .to_string()
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        note: NoteArgs,
    }

    fn parse(words: &[&str]) -> NoteArgs {
        Cli::try_parse_from(std::iter::once("note").chain(words.iter().copied()))
            .unwrap()
            .note
    }

    fn noon() -> Zoned {
        "2026-10-29T12:00:00+00:00[UTC]".parse().unwrap()
    }

    fn checkout() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("private/captures/boot-01")).unwrap();
        (dir, root)
    }

    #[test]
    fn a_timer_is_due_in_a_length_or_at_the_next_clock_time() {
        let (_dir, root) = checkout();
        let now = noon();
        let at = seconds(now.timestamp());

        let soon = body(
            parse(&["timer", "fire", "--in", "5m", "--expect", "PLSCH1 fire"]),
            &root,
            &root,
            &now,
        )
        .unwrap();
        assert_eq!(
            soon,
            Body::Timer {
                label: "fire".into(),
                due: at + 300.0,
                until: at + 300.0 + WINDOW,
                expect: Some(Expect::Line {
                    text: "PLSCH1 fire".into()
                }),
            }
        );

        // 11:30 has passed today, so it is tomorrow's.
        let Body::Timer { due, until, .. } = body(
            parse(&[
                "timer",
                "meal",
                "--at",
                "11:30",
                "--expect-event",
                "fed",
                "--window",
                "10m",
            ]),
            &root,
            &root,
            &now,
        )
        .unwrap() else {
            panic!("not a timer");
        };
        assert_eq!(due, at + 23.5 * 3600.0);
        assert_eq!(until, due + 600.0);
    }

    #[test]
    fn a_timer_needs_a_time_and_takes_one_expectation() {
        let cli = |words: &[&str]| {
            Cli::try_parse_from(std::iter::once("note").chain(words.iter().copied())).is_ok()
        };

        assert!(!cli(&["timer", "t"]));
        assert!(!cli(&[
            "timer",
            "t",
            "--in",
            "1m",
            "--expect",
            "a",
            "--expect-event",
            "boot"
        ]));
        assert!(!cli(&["timer", "t", "--clear", "--expect", "a"]));
        assert!(cli(&["timer", "t", "--clear"]));
        assert!(!cli(&["evidence", "--card", "schedule", "x"]));
    }

    #[test]
    fn evidence_is_kept_root_relative_and_only_inside_private() {
        let (_dir, root) = checkout();

        let kept = body(
            parse(&[
                "evidence",
                "--card",
                "schedule",
                "captures/boot-01",
                "--pass",
            ]),
            &root,
            &root.join("private"),
            &noon(),
        )
        .unwrap();
        assert!(matches!(
            kept,
            Body::Evidence { path: Some(path), verdict: Verdict::Pass, .. } if path == "private/captures/boot-01"
        ));

        for outside in [".", "private", "private/../src"] {
            std::fs::create_dir_all(root.join("src")).unwrap();
            let refused = body(
                parse(&["evidence", "--card", "schedule", outside, "--fail"]),
                &root,
                &root,
                &noon(),
            );
            assert!(refused.is_err(), "{outside}");
        }
    }
}
