//! What the console shows, drawn from the server's live state alone: the
//! same panels as the page, so the operator at the bench reads the same
//! facts in the same words. Pure: the screen is a function of the state,
//! the log and the scroll, which the tests draw on ratatui's `TestBackend`.

use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Wrap};
use wellfed_formats::bench::banks::Bank;
use wellfed_formats::bench::live::{
    BenchState, CancelState, JournalSource, PendingPrompt, Phase, Session, Uart,
};
use wellfed_formats::bench::notes::{Expect, Timer, TimerState, Verdict};
use wellfed_formats::bench::progress::StepState;
use wellfed_formats::bench::prompt::Expects;

use crate::cli::{bank_number, entry_text};
use crate::note::clock;

/// What one frame draws.
pub(crate) struct Screen<'a> {
    pub(crate) state: &'a BenchState,
    /// The console's lines, oldest first.
    pub(crate) log: &'a VecDeque<String>,
    /// Lines up from the newest; 0 follows the log.
    pub(crate) scroll: usize,
    /// What the footer shows instead of the keys, such as the
    /// confirmation a first `q` during a write asks for.
    pub(crate) notice: Option<&'a str>,
}

const ROLES: [(&str, &str); 4] = [
    ("1", "Feeder power"),
    ("2", "Loader strap"),
    ("3", "UART into the feeder"),
    ("4", "UART out of the feeder"),
];
const UNKNOWN: &str = "not known yet";
/// Device log lines the feeder panel keeps on screen.
const DEVICE_TAIL: usize = 4;
/// The rows a job's question takes, with its borders: its words wrap.
const PROMPT_ROWS: u16 = 7;

pub(crate) fn draw(frame: &mut Frame<'_>, screen: &Screen<'_>) {
    let state = screen.state;
    let banner = match state.uart {
        Uart::Writing { .. } => 4,
        Uart::Loader => 1,
        Uart::Idle | Uart::Capturing { .. } => 0,
    };
    let asked = if state.prompt.is_some() {
        PROMPT_ROWS
    } else {
        0
    };
    let [header, banner_area, middle, prompt, log, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banner),
        Constraint::Fill(2),
        Constraint::Length(asked),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(Paragraph::new(header_line(state)), header);
    draw_banner(frame, banner_area, &state.uart);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(middle);
    let [job, activity, operator] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(4),
        Constraint::Min(4),
    ])
    .areas(left);
    let [relays, feeder] =
        Layout::vertical([Constraint::Length(8), Constraint::Min(8)]).areas(right);

    panel(frame, job, "Current job", job_lines(state));
    panel(frame, activity, "Activity", activity_lines(state));
    panel(frame, operator, "Operator", operator_lines(state));
    panel(frame, relays, "Relays and lease", relay_lines(state));
    panel(frame, feeder, "Feeder", feeder_lines(state));
    if let Some(pending) = &state.prompt {
        draw_prompt(frame, prompt, pending);
    }
    draw_log(frame, log, screen);

    let keys =
        "q or Ctrl+C stops the server (cancel, cleanup, release) · PgUp/PgDn scroll · End follows";
    let footer_line = match screen.notice {
        Some(notice) => Paragraph::new(notice).style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        None => Paragraph::new(keys).style(Style::new().fg(Color::DarkGray)),
    };
    frame.render_widget(footer_line, footer);
}

fn header_line(state: &BenchState) -> Line<'static> {
    let phase = match (state.server.phase, state.server.stopping) {
        (Phase::Serving, Some(deferred)) => Span::styled(
            match deferred.job {
                Some(job) => format!("stopping once job {job} ends: no new jobs"),
                None => "stopping once the write ends: no new jobs".to_string(),
            },
            Style::new().fg(Color::Yellow),
        ),
        (Phase::Serving, None) => Span::styled("serving", Style::new().fg(Color::Green)),
        (Phase::Stopping, _) => Span::styled(
            "stopping: cancelling the job, then releasing the bench",
            Style::new().fg(Color::Yellow),
        ),
    };
    Line::from(vec![
        Span::styled(
            "wellfed-bench serve ",
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "pid {} since {} · ",
            state.server.pid,
            clock(state.server.started)
        )),
        phase,
        Span::raw(format!(" · {}", clock(state.now))),
    ])
}

/// The write banner says not to touch the bench, with the sectors verified
/// so far; a loader session gets a line of its own.
fn draw_banner(frame: &mut Frame<'_>, area: Rect, uart: &Uart) {
    match *uart {
        Uart::Writing {
            done,
            total,
            counted,
        } => {
            let alarm = Style::new()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD);
            let [title, gauge] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).areas(area);
            frame.render_widget(
                Paragraph::new(
                    " UART WRITE IN PROGRESS: don't touch the bench, the feeder or its cables",
                )
                .style(alarm),
                title,
            );
            let (ratio, label) = if total > 0 {
                (
                    ratio(done, total),
                    format!("{done} of {total} sectors verified"),
                )
            } else if counted {
                (0.0, "Waiting for the first sector count".to_string())
            } else {
                (0.0, "This write reports no sector count".to_string())
            };
            frame.render_widget(
                Gauge::default()
                    .block(Block::default().borders(Borders::ALL))
                    .gauge_style(Style::new().fg(Color::Red))
                    .ratio(ratio)
                    .label(label),
                gauge,
            );
        }
        Uart::Loader => frame.render_widget(
            Paragraph::new(" The flash client holds the UART in a loader session")
                .style(Style::new().fg(Color::Black).bg(Color::Yellow)),
            area,
        ),
        Uart::Idle | Uart::Capturing { .. } => {}
    }
}

/// The question a job waits on, right above its output (where the
/// installer prints what goes with it, such as a recovery code), with the
/// keys that answer it.
fn draw_prompt(frame: &mut Frame<'_>, area: Rect, pending: &PendingPrompt) {
    let keys = match pending.expects {
        Expects::Done => "d: done",
        Expects::YesNo => "y: yes · n: no",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow))
        .title(format!(
            " Job {} asks (question {}) · {keys} ",
            pending.job, pending.seq
        ));
    frame.render_widget(
        Paragraph::new(pending.text.clone())
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn ratio(done: u64, total: u64) -> f64 {
    let done = u32::try_from(done.min(total)).unwrap_or(u32::MAX);
    let total = u32::try_from(total).unwrap_or(u32::MAX).max(1);
    (f64::from(done) / f64::from(total)).clamp(0.0, 1.0)
}

fn panel(frame: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn muted(text: impl Into<String>) -> Line<'static> {
    Line::styled(text.into(), Style::new().fg(Color::DarkGray))
}

fn job_lines(state: &BenchState) -> Vec<Line<'static>> {
    let Some(job) = &state.job else {
        return vec![muted("No job yet")];
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(job.label.clone(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(format!(" (job {}, from {})", job.id, clock(job.accepted))),
    ])];
    let running = job.record.as_ref().and_then(|record| {
        record
            .steps
            .iter()
            .find(|step| step.state == StepState::Running)
    });
    if let Some(step) = running {
        lines.push(Line::from(format!("Step: {}", step.label)));
    }
    if let Some(progress) = job
        .record
        .as_ref()
        .and_then(|record| record.progress.as_ref())
    {
        let unit = format!("{:?}", progress.unit).to_lowercase();
        lines.push(Line::from(format!(
            "Progress: {} of {} {unit}",
            progress.done, progress.total
        )));
    }
    if !job.upcoming.is_empty() {
        lines.push(muted(format!("Next: {}", job.upcoming.join(" → "))));
    }
    if job.cancel == CancelState::Requested && job.end.is_none() {
        lines.push(Line::styled(
            "Cancel asked: running its cleanup",
            Style::new().fg(Color::Yellow),
        ));
    }
    if let Some(end) = &job.end {
        let (text, color) = match (end.code, &end.error) {
            (0, _) => ("complete".to_string(), Color::Green),
            (_, Some(error)) => (format!("stopped ({}): {error}", end.code), Color::Red),
            (code, None) => (format!("stopped ({code})"), Color::Red),
        };
        lines.push(Line::styled(
            format!("Ended {}: {text}", clock(end.finished)),
            Style::new().fg(color),
        ));
    }
    lines
}

fn activity_lines(state: &BenchState) -> Vec<Line<'static>> {
    if state.activity.is_empty() {
        return vec![muted("No child process running")];
    }
    state
        .activity
        .iter()
        .map(|child| {
            Line::from(format!(
                "{:?} pid {}: {} (since {})",
                child.kind,
                child.pid,
                child.label,
                clock(child.started)
            ))
        })
        .collect()
}

fn relay_lines(state: &BenchState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!(
        "Lease: this server, pid {}, since {}",
        state.server.pid,
        clock(state.server.started)
    ))];
    for (channel, role) in ROLES {
        let line = match state.relays.get(channel) {
            Some(relay) => {
                let (word, color) = if relay.on {
                    ("on ", Color::Green)
                } else {
                    ("off", Color::DarkGray)
                };
                Line::from(vec![
                    Span::raw(format!("{channel} {role:<24}")),
                    Span::styled(word, Style::new().fg(color)),
                    Span::raw(format!("  read {}", clock(relay.at))),
                ])
            }
            None => muted(format!("{channel} {role:<24}not read")),
        };
        lines.push(line);
    }
    lines.push(Line::from(format!(
        "UART adapter: {}",
        if state.server.uart_port {
            "present"
        } else {
            "missing"
        }
    )));
    lines
}

fn feeder_lines(state: &BenchState) -> Vec<Line<'static>> {
    let feeder = &state.feeder;
    let capture = &state.capture;
    let mut lines = Vec::new();

    for bank in [Bank::Zero, Bank::One] {
        let text = state
            .banks
            .banks
            .get(&bank)
            .map_or_else(|| UNKNOWN.to_string(), entry_text);
        lines.push(Line::from(format!("Bank {}: {text}", bank_number(bank))));
    }
    lines.push(Line::from(format!(
        "Session: {}",
        session_text(&feeder.session)
    )));
    if feeder.other_sessions > 0 {
        lines.push(Line::styled(
            format!(
                "{} other device API session(s) open on the feeder",
                feeder.other_sessions
            ),
            Style::new().fg(Color::Red),
        ));
    }
    let status = feeder.status.as_ref().map_or_else(
        || UNKNOWN.to_string(),
        |status| {
            format!(
                "build {}, bank {}, boot {}, {} dBm",
                status.build.get(..12).unwrap_or(&status.build),
                status.bank,
                status.boot_count,
                status.rssi
            )
        },
    );
    lines.push(Line::from(format!("Status: {status}")));
    let clock_text = match (feeder.clock.skew_ms, feeder.clock.synced_at) {
        _ if feeder
            .status
            .as_ref()
            .is_some_and(|status| status.clock_ms.is_none()) =>
        {
            "invalid".to_string()
        }
        (Some(skew), Some(synced)) => format!("{skew:+} ms, set {}", clock(synced)),
        (Some(skew), None) => format!("{skew:+} ms"),
        (None, _) => UNKNOWN.to_string(),
    };
    lines.push(Line::from(format!("Clock: {clock_text}")));
    let journal = feeder.journal.map_or_else(
        || UNKNOWN.to_string(),
        |journal| {
            let epoch = journal
                .epoch
                .map(|epoch| format!(" in epoch {epoch}"))
                .unwrap_or_default();
            let from = match journal.from {
                JournalSource::Status => "status",
                JournalSource::Event => "event stream",
            };
            format!("event {}{epoch} (from the {from})", journal.seq)
        },
    );
    lines.push(Line::from(format!("Journal high water: {journal}")));
    let hotspot = match feeder.hotspot {
        Some(true) => "up",
        Some(false) => "down",
        None => UNKNOWN,
    };
    lines.push(Line::from(format!("Hotspot: {hotspot}")));
    let capturing = match (&capture.segment, &capture.failure) {
        (Some(segment), _) => format!("capturing to {segment}"),
        (None, Some(failure)) => format!("stopped: {failure}"),
        (None, None) => "not capturing".to_string(),
    };
    lines.push(Line::from(format!("Capture: {capturing}")));
    if let Some(start) = &capture.start {
        lines.push(Line::from(format!(
            "Last start banner: build {}{}",
            start.build.get(..12).unwrap_or(&start.build),
            start
                .bank
                .map(|bank| format!(", bank {bank}"))
                .unwrap_or_default()
        )));
    }

    lines.push(muted("Device log:"));
    let tail = feeder.log.len().saturating_sub(DEVICE_TAIL);
    if feeder.log.is_empty() {
        lines.push(muted(format!("  {UNKNOWN}")));
    }
    for line in &feeder.log[tail..] {
        lines.push(muted(format!("  {line}")));
    }
    lines
}

fn session_text(session: &Session) -> String {
    match session {
        Session::Idle => "not tried yet".into(),
        Session::Unconfigured { error } => format!("none: {error}"),
        Session::Paused { pause } => format!("paused: {pause}"),
        Session::Connecting { attempt, error, .. } => match error {
            Some(error) => format!("connecting, {attempt} failed ({error})"),
            None => "connecting".into(),
        },
        Session::Open { address, since, .. } => {
            format!("open to {address} since {}", clock(*since))
        }
    }
}

fn operator_lines(state: &BenchState) -> Vec<Line<'static>> {
    let operator = &state.operator;
    let mut lines = Vec::new();

    match &operator.step {
        Some(step) => lines.push(Line::styled(
            format!("Now: {step}"),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        None => lines.push(muted("No step noted (`wellfed-bench note step`)")),
    }
    for (n, step) in operator.next.iter().enumerate() {
        lines.push(muted(format!("  {}. {step}", n + 1)));
    }
    for timer in &operator.timers {
        lines.push(timer_line(timer, state.now));
    }
    for (card, items) in &operator.evidence {
        let passed = items
            .iter()
            .filter(|item| item.verdict == Verdict::Pass)
            .count();
        let failed = items.len() - passed;
        let last = items
            .last()
            .map(|item| item.text.as_str())
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{card}: "),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{passed} pass"), Style::new().fg(Color::Green)),
            Span::raw(", "),
            Span::styled(
                format!("{failed} fail"),
                Style::new().fg(if failed > 0 {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw(if last.is_empty() {
                String::new()
            } else {
                format!("; last: {last}")
            }),
        ]));
    }
    lines
}

fn timer_line(timer: &Timer, now: f64) -> Line<'static> {
    let left = timer.due - now;
    let when = if left > 0.0 {
        format!("in {}", length(left))
    } else {
        format!("due {} ago", length(-left))
    };
    let expect = timer.expect.as_ref().map(|expect| match expect {
        Expect::Line { text } => format!(", expects {text:?} until {}", clock(timer.until)),
        Expect::Event { kind } => format!(", expects a {kind} event until {}", clock(timer.until)),
    });
    let (settled, color) = match &timer.state {
        TimerState::Waiting if timer.expect.is_some() => (" waiting".to_string(), Color::Yellow),
        TimerState::Waiting => (String::new(), Color::Reset),
        TimerState::Seen { at, line } => (format!(" SEEN {}: {line}", clock(*at)), Color::Green),
        TimerState::Missed { .. } => (" MISSED".to_string(), Color::Red),
    };
    Line::from(vec![
        Span::styled(
            format!("⏱ {} ", timer.label),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{when}{}", expect.unwrap_or_default())),
        Span::styled(settled, Style::new().fg(color)),
    ])
}

/// `seconds` as the page writes a length: `42s`, `4m 05s`, `1h 02m`.
fn length(seconds: f64) -> String {
    let total = std::time::Duration::try_from_secs_f64(seconds.max(0.0) + 0.5)
        .map_or(u64::MAX, |length| length.as_secs());
    let (hours, minutes, rest) = (total / 3600, total % 3600 / 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {rest:02}s")
    } else {
        format!("{rest}s")
    }
}

fn draw_log(frame: &mut Frame<'_>, area: Rect, screen: &Screen<'_>) {
    let rows = usize::from(area.height.saturating_sub(2));
    let end = screen.log.len().saturating_sub(screen.scroll);
    let start = end.saturating_sub(rows);
    let lines: Vec<Line<'_>> = screen
        .log
        .range(start..end)
        .map(|line| Line::from(line.as_str()))
        .collect();
    let title = if screen.scroll == 0 {
        " Log ".to_string()
    } else {
        format!(" Log ({} lines up; End follows) ", screen.scroll)
    };
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

#[cfg(test)]
mod tests;
