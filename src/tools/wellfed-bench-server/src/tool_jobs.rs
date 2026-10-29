//! The jobs that run one tool to its end rather than a rig sequence:
//! `install` (`wellfed install --prompts lines`) and `stock-serve`
//! (`wellfed-stock serve`). Each is a child of the server, so the bench
//! lease it takes comes back inherited, and the server never stops for it.
//!
//! The installer asks the person at the feeder as it goes. Each question is
//! one [`Prompt`] line on its standard output, and it waits for one
//! [`Answer`] line on its standard input. The job answers the restart
//! itself with the power relay. Every other question goes into the live
//! state as the [`PendingPrompt`] the page, the console, the CLI and an
//! agent show, until the server takes an answer for it (`Shared::answer`)
//! or the job ends. The installer's answers carry no number and are read
//! in order, so an answer reaches it only for the question it is waiting
//! on, never ahead of one.
//!
//! Its standard error is the job's output. A standard output line that is
//! no prompt means the installer and the server disagree about what is
//! asked, and nothing it waits for could be answered safely: the job shows
//! the line and stops the installer.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use clap::Args;
use wellfed_formats::bench::live::{ActivityKind, AnswerError, JobId, PendingPrompt};
use wellfed_formats::bench::prompt::{Answer, Prompt};
use wellfed_rig::commands::{Bench, seconds_arg};
use wellfed_rig::tools::{own_group, own_group_with_input};

use crate::jobs::{JobError, ToolPaths};
use crate::state::Store;

/// How often the job looks at its cancel while its child is quiet.
const TICK: Duration = Duration::from_millis(100);
/// How long the install waits for the server's session to show it closed
/// for the job: a session still opening sees the hold once its attempt
/// ends.
const PAUSE_WAIT: Duration = Duration::from_secs(60);

/// `install`: the flags of `wellfed install` the bench uses. The job adds
/// `--prompts lines` and the server's own `wellfed-stock`; the frame route
/// (`--ssid`) reads a passphrase at a terminal and has no place here.
#[derive(Debug, Clone, Args)]
pub struct InstallArgs {
    /// The replacement's frozen review, root-relative.
    #[arg(long)]
    pub review: PathBuf,
    /// The signed update manifest for the same build.
    #[arg(long)]
    pub manifest: PathBuf,
    /// The installer's `wellfed-stock` config (`init-installer`).
    #[arg(long)]
    pub stock_config: PathBuf,
    /// Stock's broker name, which the owner's resolver points at this
    /// computer: the DNS route.
    #[arg(long, conflicts_with = "from_bank0")]
    pub broker_name: Option<String>,
    /// The replacement is already on bank 0: start at pairing.
    #[arg(long)]
    pub from_bank0: bool,
    /// A copy of stock 1.1.15's OTA image that matches the pinned digest.
    #[arg(long)]
    pub stock_backup: Option<PathBuf>,
    /// How long the broker trusts the first stock feeder that connects.
    #[arg(long)]
    pub enroll_minutes: Option<u64>,
    /// Have the stock server set stock's clock even when it is far off.
    #[arg(long)]
    pub calibrate_clock: bool,
    /// Promote even if stock's stored plans cannot be read.
    #[arg(long)]
    pub without_stock_plans: bool,
    /// This computer's client key; the installer's default otherwise.
    #[arg(long)]
    pub keys: Option<PathBuf>,
    /// The replacement's address; where stock connected from otherwise.
    #[arg(long)]
    pub address: Option<String>,
    /// How long relay 1 stays off when the installer asks for the feeder
    /// to restart: about 10 s unplugged, as stock's own instruction says.
    #[arg(long, default_value_t = 10.0)]
    pub restart_seconds: f64,
}

impl InstallArgs {
    /// The installer's arguments. Each value goes as `--flag=value`, so
    /// none can read as a flag of its own.
    fn arguments(&self, stock_bin: &Path) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec!["install".into(), "--prompts=lines".into()];
        let mut value = |flag: &str, value: &dyn AsRef<std::ffi::OsStr>| {
            let mut word = OsString::from(format!("--{flag}="));
            word.push(value);
            arguments.push(word);
        };
        value("stock-server", &stock_bin);
        value("review", &self.review);
        value("manifest", &self.manifest);
        value("stock-config", &self.stock_config);
        if let Some(name) = &self.broker_name {
            value("broker-name", name);
        }
        if let Some(backup) = &self.stock_backup {
            value("stock-backup", backup);
        }
        if let Some(minutes) = self.enroll_minutes {
            value("enroll-minutes", &minutes.to_string());
        }
        if let Some(keys) = &self.keys {
            value("keys", keys);
        }
        if let Some(address) = &self.address {
            value("address", address);
        }

        for (flag, on) in [
            ("--from-bank0", self.from_bank0),
            ("--calibrate-clock", self.calibrate_clock),
            ("--without-stock-plans", self.without_stock_plans),
        ] {
            if on {
                arguments.push(flag.into());
            }
        }
        arguments
    }
}

/// `stock-serve`: `wellfed-stock serve` until it exits or the job is
/// cancelled, as the enrollment or broker window an install or a test
/// needs beside the bench.
#[derive(Debug, Clone, Args)]
pub struct StockServeArgs {
    /// The `wellfed-stock` config, root-relative.
    #[arg(long, default_value = "private/server.json")]
    pub config: PathBuf,
    /// With an installer's config and no feeder enrolled yet: trust the
    /// first stock feeder that connects within this many minutes.
    #[arg(long)]
    pub enroll_minutes: Option<u64>,
    /// Set stock's clock in every reply, even when it is far off.
    #[arg(long)]
    pub calibrate_clock: bool,
    /// Allow the stock-to-replacement install trial, which a request to
    /// the server's loopback API then starts.
    #[arg(long)]
    pub enable_ota_trial: bool,
}

impl StockServeArgs {
    fn arguments(&self) -> Vec<OsString> {
        let mut config = OsString::from("--config=");
        config.push(&self.config);
        let mut arguments: Vec<OsString> = vec!["serve".into(), config];
        if let Some(minutes) = self.enroll_minutes {
            arguments.push(format!("--enroll-minutes={minutes}").into());
        }
        for (flag, on) in [
            ("--calibrate-clock", self.calibrate_clock),
            ("--enable-ota-trial", self.enable_ota_trial),
        ] {
            if on {
                arguments.push(flag.into());
            }
        }
        arguments
    }
}

/// Why a tool job stopped.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolJobError {
    #[error(
        "{} not found; build it with cargo build --release -p wellfed or pass serve --wellfed-bin",
        path.display()
    )]
    WellfedMissing { path: PathBuf },
    #[error(
        "{} not found; build it with cargo build --release -p wellfed-stock or pass serve --stock-bin",
        path.display()
    )]
    StockMissing { path: PathBuf },
    /// The installer wrote a line on its standard output that is no
    /// prompt; it was stopped.
    #[error("wellfed install wrote {line:?} where a prompt goes; it was stopped")]
    Garbled { line: String },
    /// The installer stopped: its own last words, as it printed them.
    #[error("{}", message.as_deref().unwrap_or("wellfed install stopped without saying why"))]
    InstallStopped { code: i32, message: Option<String> },
    #[error("wellfed-stock serve exited with code {code}")]
    ServeExited { code: i32 },
    /// The installer opens its own sessions only beside a server session
    /// shown paused for its job, and it did not show that in time.
    #[error(
        "The bench server's feeder session did not show itself paused for job {job} within {} s; nothing was started",
        PAUSE_WAIT.as_secs()
    )]
    NotPaused { job: JobId },
}

/// What the job's loop hears: its child's lines, and the answers the
/// server takes for it.
pub(crate) enum Event {
    /// A line for the job's output.
    Output(String),
    Prompt(Prompt),
    /// A standard output line of the installer's that is no prompt.
    Garbled(String),
    /// One of the child's output pipes reached its end.
    Closed,
    Answer(Answer),
}

/// The job's side of its questions: it asks through the live state and
/// hears the answers the server took.
pub(crate) struct Asking {
    job: JobId,
    store: Store,
    sender: mpsc::Sender<Event>,
    events: mpsc::Receiver<Event>,
}

/// The server's side: where an answer it took for the job goes.
#[derive(Clone)]
pub(crate) struct Answering {
    sender: mpsc::Sender<Event>,
}

/// The two ends for job `job`. Every job gets them; only `install` asks.
pub(crate) fn asking(job: JobId, store: Store) -> (Asking, Answering) {
    let (sender, events) = mpsc::channel();
    let answering = Answering {
        sender: sender.clone(),
    };
    let asking = Asking {
        job,
        store,
        sender,
        events,
    };
    (asking, answering)
}

impl Answering {
    /// Hand the job an answer the server has checked against its question
    /// and taken out of the state. A job that has ended no longer hears.
    pub(crate) fn send(&self, answer: Answer) {
        let _ = self.sender.send(Event::Answer(answer));
    }
}

impl Asking {
    fn ask(&self, pending: PendingPrompt) {
        self.store.update(|state| state.prompt = Some(pending));
    }

    /// Wait until the state shows the feeder session paused for this job,
    /// which `wellfed install` checks before it opens sessions of its own.
    fn until_paused(&self, cancel: &wellfed_rig::Cancel) -> Result<(), JobError> {
        let deadline = std::time::Instant::now() + PAUSE_WAIT;
        loop {
            if self.store.with(|state| state.own_sessions_job()) == Some(self.job) {
                return Ok(());
            }
            if cancel.is_cancelled() {
                return Err(wellfed_rig::Error::Interrupted.into());
            }
            if std::time::Instant::now() >= deadline {
                return Err(ToolJobError::NotPaused { job: self.job }.into());
            }
            std::thread::sleep(TICK);
        }
    }

    /// Take this job's question out of the state, if one is there.
    pub(crate) fn withdraw(&self) {
        let job = self.job;
        self.store.update(|state| {
            if state
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.job == job)
            {
                state.prompt = None;
            }
        });
    }
}

/// Check `answer` against job `job`'s question `seq` in `prompt`, taking
/// the question out when it fits, so the same `seq` is never answered
/// twice.
pub(crate) fn take(
    prompt: &mut Option<PendingPrompt>,
    job: JobId,
    seq: u32,
    answer: Answer,
) -> Result<(), AnswerError> {
    let Some(pending) = prompt.as_ref().filter(|pending| pending.job == job) else {
        return Err(AnswerError::NoPrompt { job });
    };
    wellfed_formats::require(pending.seq == seq, || AnswerError::WrongSeq {
        job,
        seq,
        pending: pending.seq,
    })?;
    wellfed_formats::require(pending.prompt.accepts(answer), || AnswerError::Mismatched {
        name: pending.prompt.name().to_string(),
        expects: pending.expects,
        answer,
    })?;
    *prompt = None;
    Ok(())
}

/// Read `pipe` line by line into `events`, each line made an event by
/// `read`, and say when it ends. Lines are read as bytes and shown
/// lossily, so no output ends the reading early.
fn read_lines(
    pipe: impl Read + Send + 'static,
    events: mpsc::Sender<Event>,
    read: fn(String) -> Event,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = Vec::new();
        while matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            let text = String::from_utf8_lossy(&line).trim_end().to_string();
            if events.send(read(text)).is_err() {
                return;
            }
            line.clear();
        }
        let _ = events.send(Event::Closed);
    });
}

fn prompt_line(line: String) -> Event {
    match serde_json::from_str::<Prompt>(&line) {
        Ok(prompt) => Event::Prompt(prompt),
        Err(_) => Event::Garbled(line),
    }
}

/// The installer's message once it stops: what follows its last
/// `Error:` line, as a program that returns its error prints it.
#[derive(Default)]
struct LastWords {
    lines: Vec<String>,
}

impl LastWords {
    fn hear(&mut self, line: &str) {
        if line.starts_with("Error: ") {
            self.lines.clear();
        }
        if !self.lines.is_empty() || line.starts_with("Error: ") {
            self.lines.push(line.to_string());
        }
    }

    fn message(self) -> Option<String> {
        let text = self.lines.join("\n");
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

/// Run `wellfed install` to its end, answering its restart with the relay
/// and passing every other question to whoever answers at the bench.
pub(crate) fn install(
    bench: &mut Bench,
    args: &InstallArgs,
    tools: &ToolPaths,
    asking: &Asking,
) -> Result<u8, JobError> {
    let off = seconds_arg("--restart-seconds", args.restart_seconds)?;
    let path = &tools.wellfed_bin;
    wellfed_formats::require(path.is_file(), || ToolJobError::WellfedMissing {
        path: path.clone(),
    })?;
    asking.until_paused(&bench.cancel)?;

    let mut command = Command::new(path);
    own_group_with_input(&mut command)
        .current_dir(&bench.root)
        .args(args.arguments(&tools.stock_bin))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(wellfed_rig::Error::from)?;
    let tracked = bench
        .children
        .track(&child, ActivityKind::Install, "install");

    if let Some(stdout) = child.stdout.take() {
        read_lines(stdout, asking.sender.clone(), prompt_line);
    }
    if let Some(stderr) = child.stderr.take() {
        read_lines(stderr, asking.sender.clone(), Event::Output);
    }
    let mut talk = Conversation {
        input: child.stdin.take(),
        off,
        seq: 0,
        asked: None,
        stopping: None,
        cancelled: false,
        last_words: LastWords::default(),
    };
    talk.run(bench, asking);

    // Every pipe has closed, so the installer has exited or is about to.
    let waited = child.wait();
    drop(tracked);
    asking.withdraw();
    let status = waited.map_err(wellfed_rig::Error::from)?;

    if let Some(error) = talk.stopping {
        return Err(error);
    }
    if talk.cancelled {
        return Err(wellfed_rig::Error::Interrupted.into());
    }
    match status.code() {
        Some(0) => Ok(0),
        code => Err(ToolJobError::InstallStopped {
            code: code.unwrap_or(1),
            message: talk.last_words.message(),
        }
        .into()),
    }
}

/// One install's questions and answers, until its pipes close.
struct Conversation {
    input: Option<ChildStdin>,
    /// How long a restart keeps the feeder off.
    off: Duration,
    /// The number of the last question passed on.
    seq: u32,
    /// The question an answer is awaited for.
    asked: Option<u32>,
    /// Why the job stopped the installer, once it has.
    stopping: Option<JobError>,
    cancelled: bool,
    last_words: LastWords,
}

impl Conversation {
    fn run(&mut self, bench: &mut Bench, asking: &Asking) {
        let mut open = 2;
        while open > 0 {
            // A cancel signals the installer itself (`Children::interrupt`),
            // which stops its stock server and exits; its question goes.
            if !self.cancelled && bench.cancel.is_cancelled() {
                self.cancelled = true;
                self.asked = None;
                asking.withdraw();
            }
            let event = match asking.events.recv_timeout(TICK) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                // The job holds a sender itself, so this never comes.
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            match event {
                Event::Output(line) => {
                    eprintln!("{line}");
                    bench.status.line(&line);
                    self.last_words.hear(&line);
                }
                Event::Closed => open -= 1,
                Event::Garbled(line) => {
                    eprintln!("{line}");
                    self.stop(bench, ToolJobError::Garbled { line }.into());
                }
                // The installer is being stopped: nothing it asks now is
                // put to anyone.
                Event::Prompt(_) if self.cancelled || self.stopping.is_some() => {}
                Event::Prompt(Prompt::RestartFeeder) => match restart(bench, self.off) {
                    Ok(()) => self.reply(Answer::Done),
                    Err(error) => self.stop(bench, error.into()),
                },
                Event::Prompt(prompt) => {
                    self.seq += 1;
                    self.asked = Some(self.seq);
                    let pending = PendingPrompt::new(asking.job, self.seq, prompt);
                    wellfed_rig::say!("Job {} asks ({}): {}", asking.job, self.seq, pending.text);
                    wellfed_rig::say!(
                        "Answer on the page or the console, or: wellfed-bench answer {} {} {}",
                        asking.job,
                        self.seq,
                        pending.answer_words()
                    );
                    asking.ask(pending);
                }
                // The server passes an answer only for the question in the
                // state; one that crossed a cancel goes nowhere.
                Event::Answer(answer) => {
                    if self.asked.take().is_some() {
                        wellfed_rig::say!("Answered {answer}");
                        self.reply(answer);
                    }
                }
            }
        }
    }

    /// Write `answer` as the installer's next input line. A closed pipe
    /// means it is exiting, which its pipes closing then shows.
    fn reply(&mut self, answer: Answer) {
        let Some(input) = &mut self.input else {
            return;
        };
        let written = serde_json::to_string(&answer)
            .map_err(std::io::Error::from)
            .and_then(|line| writeln!(input, "{line}"))
            .and_then(|()| input.flush());
        if let Err(error) = written {
            eprintln!("Could not answer wellfed install: {error}");
        }
    }

    /// Stop the installer as a cancel does, on its own SIGTERM handler,
    /// and keep reading until it has gone.
    fn stop(&mut self, bench: &Bench, error: JobError) {
        if self.stopping.is_none() {
            self.stopping = Some(error);
            self.asked = None;
            bench.children.interrupt();
        }
    }
}

/// Restart the feeder for the installer: power off for `off`, then on
/// again, whatever came of the wait. A cancel during it still leaves the
/// feeder powered, as it was before the job.
fn restart(bench: &mut Bench, off: Duration) -> Result<(), wellfed_rig::Error> {
    bench.status.step("restart feeder");
    wellfed_rig::say!(
        "Restarting the feeder for the installer: relay 1 off for {} s, then on",
        off.as_secs_f64()
    );
    bench.cut_power()?;
    let waited = bench.pause(off);
    bench.on()?;
    waited?;
    wellfed_rig::say!("Feeder powered again; stock looks its broker name up as it starts");
    Ok(())
}

/// Run `wellfed-stock serve` until it exits or the job is cancelled, its
/// output as the job's.
pub(crate) fn stock_serve(
    bench: &mut Bench,
    args: &StockServeArgs,
    tools: &ToolPaths,
    asking: &Asking,
) -> Result<u8, JobError> {
    let path = &tools.stock_bin;
    wellfed_formats::require(path.is_file(), || ToolJobError::StockMissing {
        path: path.clone(),
    })?;
    let mut command = Command::new(path);
    own_group(&mut command)
        .current_dir(&bench.root)
        .args(args.arguments())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(wellfed_rig::Error::from)?;
    let tracked = bench.children.track(&child, ActivityKind::Stock, "serve");

    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    ];
    let mut open = 0;
    for pipe in pipes.into_iter().flatten() {
        read_lines(pipe, asking.sender.clone(), Event::Output);
        open += 1;
    }
    // A cancel signals the server itself; its pipes close as it exits.
    while open > 0 {
        match asking.events.recv_timeout(TICK) {
            Ok(Event::Output(line)) => {
                eprintln!("{line}");
                bench.status.line(&line);
            }
            Ok(Event::Closed) => open -= 1,
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let waited = child.wait();
    drop(tracked);
    let status = waited.map_err(wellfed_rig::Error::from)?;
    if bench.cancel.is_cancelled() {
        return Err(wellfed_rig::Error::Interrupted.into());
    }
    match status.code() {
        Some(0) => Ok(0),
        code => Err(ToolJobError::ServeExited {
            code: code.unwrap_or(1),
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use wellfed_formats::bench::prompt::Expects;

    use super::*;

    #[test]
    fn install_values_go_as_one_word_each_with_lines_prompts_and_the_server_s_stock() {
        let args = InstallArgs {
            review: "private/r".into(),
            manifest: "private/m.json".into(),
            stock_config: "private/installer.json".into(),
            broker_name: Some("-x.example".into()),
            from_bank0: false,
            stock_backup: None,
            enroll_minutes: Some(5),
            calibrate_clock: true,
            without_stock_plans: false,
            keys: None,
            address: None,
            restart_seconds: 10.0,
        };

        let words: Vec<String> = args
            .arguments(Path::new("/bin/stock"))
            .into_iter()
            .map(|word| word.into_string().unwrap())
            .collect();

        assert_eq!(
            words,
            [
                "install",
                "--prompts=lines",
                "--stock-server=/bin/stock",
                "--review=private/r",
                "--manifest=private/m.json",
                "--stock-config=private/installer.json",
                "--broker-name=-x.example",
                "--enroll-minutes=5",
                "--calibrate-clock",
            ]
        );
    }

    #[test]
    fn an_answer_is_taken_once_for_its_own_question_and_kind() {
        let asked = PendingPrompt::new(4, 2, Prompt::OpenRecoveryWindow);
        assert_eq!(asked.expects, Expects::Done);
        let mut prompt = Some(asked);

        assert_eq!(
            take(&mut prompt, 3, 2, Answer::Done),
            Err(AnswerError::NoPrompt { job: 3 })
        );
        assert_eq!(
            take(&mut prompt, 4, 1, Answer::Done),
            Err(AnswerError::WrongSeq {
                job: 4,
                seq: 1,
                pending: 2
            })
        );
        assert!(matches!(
            take(&mut prompt, 4, 2, Answer::Yes),
            Err(AnswerError::Mismatched {
                answer: Answer::Yes,
                ..
            })
        ));
        assert!(prompt.is_some(), "a refused answer leaves the question");

        take(&mut prompt, 4, 2, Answer::Done).unwrap();
        assert_eq!(prompt, None);
        assert_eq!(
            take(&mut prompt, 4, 2, Answer::Done),
            Err(AnswerError::NoPrompt { job: 4 })
        );
    }

    #[test]
    fn the_installer_s_last_words_start_at_its_last_error() {
        let mut words = LastWords::default();
        for line in [
            "Starting the stock server",
            "Error: first",
            "Error: Stopped at pairing",
            "",
            "Caused by:",
            "    no answer",
        ] {
            words.hear(line);
        }

        assert_eq!(
            words.message().as_deref(),
            Some("Error: Stopped at pairing\n\nCaused by:\n    no answer")
        );
        assert_eq!(LastWords::default().message(), None);
    }
}
