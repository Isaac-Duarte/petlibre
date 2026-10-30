//! The `install` job, which runs one tool to its end rather than a rig
//! sequence: `wellfed install --prompts lines`, as a child of the server,
//! so the bench lease it takes comes back inherited and the server never
//! stops for it.
//!
//! The installer asks the person at the feeder as it goes. Each question is
//! one [`Prompt`] line on its standard output, and it waits for one
//! [`Answer`] line on its standard input. The job answers the restart
//! itself with the power relay. Every other question goes into the live
//! state as a [`PendingPrompt`], which the console shows and the page, the
//! CLI and an agent answer, until the server takes an answer for it
//! (`Shared::answer`) or the job ends. The installer's answers carry no number and are read
//! in order, so an answer reaches it only for the question it is waiting
//! on, never ahead of one.
//!
//! Its standard error is the job's output. A standard output line that is
//! no prompt means the installer and the server disagree about what is
//! asked, and nothing it waits for could be answered safely: the job shows
//! the line and stops the installer.
//!
//! With `--virtual-panel` the job also answers the panel's prompts itself,
//! as the restart is answered: it types the setup code's presses, or the
//! Dispense press, to a running `virtual-panel` build ([`Bench::press`]),
//! from this thread, each `wellfed-flash read press` a child under the
//! server's lease. The server's UART capture is already stopped for every
//! job and starts again only once it ends, so the port is free for them.
//! A press that does not echo and release stops the installer, and nothing
//! is pressed again: a code half keyed in is not keyed over, and a Dispense
//! pressed twice may feed.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use clap::Args;
use wellfed_core::buttons::Buttons;
use wellfed_core::virtual_panel::DEFAULT_HOLD_MS;
use wellfed_formats::bench::banks::{Bank, Content, Source};
use wellfed_formats::bench::live::{ActivityKind, AnswerError, JobId, PendingPrompt};
use wellfed_formats::bench::prompt::{Answer, DispensePurpose, Prompt};
use wellfed_rig::commands::{Bench, installed};
use wellfed_rig::tools::own_group_with_input;

use crate::jobs::{JobError, ToolPaths, press_name};
use crate::state::Store;

/// How often the job looks at its cancel while its child is quiet.
const TICK: Duration = Duration::from_millis(100);
/// How long the install waits for the server's session to show it closed
/// for the job: a session still opening sees the hold once its attempt
/// ends.
const PAUSE_WAIT: Duration = Duration::from_secs(60);
/// How long relay 1 stays off when the installer asks for the feeder to
/// restart: about 10 s unplugged, as stock's own instruction says. The
/// server's tests run the restart with a short one.
const RESTART_OFF: Duration = if cfg!(test) {
    Duration::from_millis(100)
} else {
    Duration::from_secs(10)
};

/// `install`: the flags of `wellfed install` the bench uses. The job adds
/// `--prompts lines` and the server's own `wellfed-stock`; the frame route
/// (`--ssid`) and `--home-ssid` read a passphrase at a terminal and have no
/// place here.
#[derive(Debug, Clone, Args)]
pub struct InstallArgs {
    /// The replacement's frozen review, root-relative.
    #[arg(long)]
    pub review: PathBuf,
    /// The signed update manifest for the same build. Without it the
    /// installer signs with --update-key (a consumer review), or the bench
    /// key (a bench review).
    #[arg(long)]
    pub manifest: Option<PathBuf>,
    /// The update-signing key the installer signs the promotion with and
    /// enrolls (a Dispense press) when the image does not build it in.
    #[arg(long, conflicts_with = "manifest")]
    pub update_key: Option<PathBuf>,
    /// A consumer image's network, from a private file of its `ssid` and
    /// `password` (private/hotspot.json on the bench).
    #[arg(long)]
    pub home_network: Option<PathBuf>,
    /// The feeder's Bluetooth address for a consumer image's setup.
    #[arg(long)]
    pub ble_address: Option<String>,
    /// Stock's MQTT client ID the owner pins: the feeder check passes
    /// without a question for it, and stops the run for any other.
    #[arg(long)]
    pub expect_device: Option<String>,
    /// Answer the installer's panel prompts by typing the presses to a
    /// running virtual-panel build: the setup code, and the Dispense press
    /// that enrolls the update key. The server's own flag; the installer
    /// never sees it.
    #[arg(long)]
    pub virtual_panel: bool,
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
        if let Some(manifest) = &self.manifest {
            value("manifest", manifest);
        }
        if let Some(key) = &self.update_key {
            value("update-key", key);
        }
        if let Some(network) = &self.home_network {
            value("home-network", network);
        }
        if let Some(address) = &self.ble_address {
            value("ble-address", address);
        }
        if let Some(device) = &self.expect_device {
            value("expect-device", device);
        }
        value("stock-config", &self.stock_config);
        if let Some(name) = &self.broker_name {
            value("broker-name", name);
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

/// Why a tool job stopped.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolJobError {
    #[error(
        "{} not found; build it with cargo build --release -p wellfed or pass serve --wellfed-bin",
        path.display()
    )]
    WellfedMissing { path: PathBuf },
    /// The installer wrote a line on its standard output that is no
    /// prompt; it was stopped.
    #[error("wellfed install wrote {line:?} where a prompt goes; it was stopped")]
    Garbled { line: String },
    /// A press for the installer's panel prompt did not echo and release;
    /// the installer was stopped, and nothing was pressed again.
    #[error(
        "the virtual panel's press {number} of {count} for the {prompt} prompt ({chord}) did not echo and release; wellfed install was stopped and nothing is pressed again"
    )]
    PressFailed {
        prompt: &'static str,
        number: usize,
        count: usize,
        chord: Buttons,
    },
    /// The installer stopped: its own last words, as it printed them.
    #[error("{}", message.as_deref().unwrap_or("wellfed install stopped without saying why"))]
    InstallStopped { code: i32, message: Option<String> },
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

/// The installer's own error once it stops. Its `main` returns every
/// failure, so the error is the last block on its standard error: an
/// `Error: ` line and anyhow's `Caused by:` list. Its stock server shares
/// that pipe and may print an `Error: ` of its own; any line after a block
/// that is not part of anyhow's shape means it was not the last word.
#[derive(Default)]
struct LastWords {
    lines: Vec<String>,
}

impl LastWords {
    fn hear(&mut self, line: &str) {
        if let Some(message) = line.strip_prefix("Error: ") {
            self.lines = vec![message.to_string()];
        } else if line.is_empty() || line == "Caused by:" || line.starts_with(' ') {
            if !self.lines.is_empty() {
                self.lines.push(line.to_string());
            }
        } else {
            self.lines.clear();
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
        virtual_panel: args.virtual_panel,
        input: child.stdin.take(),
        seq: 0,
        stopping: None,
        cancelled: false,
        last_words: LastWords::default(),
    };
    talk.run(bench, asking);

    // Every pipe has closed, so the installer has exited or is about to.
    let waited = child.wait();
    drop(tracked);
    asking.withdraw();
    let succeeded = waited.as_ref().is_ok_and(|status| status.success());
    record_install(bench, args, succeeded);
    let status = waited.map_err(wellfed_rig::Error::from)?;

    if let Some(error) = talk.stopping {
        return Err(error);
    }
    if talk.cancelled {
        return Err(wellfed_rig::Error::Interrupted.into());
    }
    match status.code() {
        Some(0) => Ok(0),
        Some(code) => Err(ToolJobError::InstallStopped {
            code,
            message: talk.last_words.message(),
        }
        .into()),
        // A signal: whatever it printed last was not its own error.
        None => Err(ToolJobError::InstallStopped {
            code: 1,
            message: None,
        }
        .into()),
    }
}

/// What an installer that ran left in the banks, decided by its exit
/// alone. A success leaves the review's build in bank 1, and in bank 0
/// unless it started there. Anything else may have stopped part-way
/// through either write, so those banks are unknown until a read.
fn record_install(bench: &mut Bench, args: &InstallArgs, succeeded: bool) {
    let content = if succeeded {
        installed(&bench.root, &args.review).unwrap_or(Content::Unknown)
    } else {
        Content::Unknown
    };
    if !args.from_bank0 {
        bench.wrote(Bank::Zero, content.clone(), Source::Install);
    }
    bench.wrote(Bank::One, content, Source::Install);
}

/// One install's questions and answers, until its pipes close.
struct Conversation {
    /// The job answers the panel's prompts with presses.
    virtual_panel: bool,
    input: Option<ChildStdin>,
    /// The number of the last question passed on.
    seq: u32,
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
                    self.stop(bench, asking, ToolJobError::Garbled { line }.into());
                }
                // The installer is being stopped: nothing it asks now is
                // put to anyone.
                Event::Prompt(_) if self.cancelled || self.stopping.is_some() => {}
                Event::Prompt(Prompt::RestartFeeder) => match restart(bench, RESTART_OFF) {
                    Ok(()) => self.reply(Answer::Done),
                    Err(error) => self.stop(bench, asking, error.into()),
                },
                Event::Prompt(prompt) if self.virtual_panel && panel_presses(&prompt).is_some() => {
                    match press_for(bench, &prompt) {
                        Ok(()) => self.reply(Answer::Done),
                        // A cancel interrupted the press: the loop's own
                        // check stops the installer.
                        Err(_) if bench.cancel.is_cancelled() => {}
                        Err(error) => self.stop(bench, asking, error),
                    }
                }
                Event::Prompt(prompt) => {
                    self.seq += 1;
                    let pending = PendingPrompt::new(asking.job, self.seq, prompt);
                    wellfed_rig::say!("Job {} asks ({}): {}", asking.job, self.seq, pending.text);
                    wellfed_rig::say!(
                        "Answer on the page, or: wellfed-bench answer {} {} {}",
                        asking.job,
                        self.seq,
                        pending.answer_words()
                    );
                    asking.ask(pending);
                }
                // The server passes an answer only for the question in the
                // state, and this loop withdraws that before it stops the
                // installer, so every answer here was taken while the
                // installer waited for it: it goes to the installer, even
                // one stopping by now.
                Event::Answer(answer) => {
                    wellfed_rig::say!("Answered {answer}");
                    self.reply(answer);
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
    fn stop(&mut self, bench: &Bench, asking: &Asking, error: JobError) {
        if self.stopping.is_none() {
            self.stopping = Some(error);
            asking.withdraw();
            bench.children.interrupt();
        }
    }
}

/// The chords that answer `prompt` on the panel, in order, each held for a
/// press; `None` for a prompt no press answers.
///
/// No Lock hold unlocks the panel first, though a person might: a code
/// being keyed in takes every gesture, whatever the lock says, so the hold
/// would count as a Lock press of the code; and a waiting enrollment takes
/// the next Dispense on a locked panel too. A locked panel also means a
/// stray Dispense outside a window feeds nothing.
fn panel_presses(prompt: &Prompt) -> Option<Vec<Buttons>> {
    match prompt {
        Prompt::KeySetupCode { presses } => {
            Some(presses.buttons().iter().map(|&b| Buttons::of(b)).collect())
        }
        Prompt::PressDispense {
            purpose: DispensePurpose::EnrollUpdateKey,
        } => Some(vec![Buttons::of(wellfed_core::buttons::Button::Dispense)]),
        Prompt::SetOverride { .. }
        | Prompt::RestartFeeder
        | Prompt::RecognizeFeeder { .. }
        | Prompt::EraseStockPlans
        | Prompt::SetupMode
        | Prompt::JoinBack
        | Prompt::OpenRecoveryWindow => None,
    }
}

/// Type `prompt`'s presses to the virtual panel, in order, each recorded
/// in the job's output; the first that does not echo and release ends it,
/// and none is typed again.
fn press_for(bench: &mut Bench, prompt: &Prompt) -> Result<(), JobError> {
    let chords = panel_presses(prompt).unwrap_or_default();
    let count = chords.len();
    for (index, chord) in chords.into_iter().enumerate() {
        let number = index + 1;
        wellfed_rig::say!(
            "Virtual panel, for the {} prompt: press {number} of {count}, {chord}",
            prompt.name()
        );
        // Numbered too: two presses must never share a millisecond's
        // evidence directory, which the second would refuse.
        let name = format!("{}-{number}", press_name());
        let code = bench.press(&name, chord, DEFAULT_HOLD_MS)?;
        wellfed_formats::require(code == 0, || ToolJobError::PressFailed {
            prompt: prompt.name(),
            number,
            count,
            chord,
        })?;
    }
    Ok(())
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
    // A cut that failed part-way may have left relay 1 off all the same,
    // so the feeder is powered again whatever came of the cut or the wait.
    let cut = bench.cut_power();
    let waited = if cut.is_ok() {
        bench.pause(off)
    } else {
        Ok(())
    };
    bench.on()?;
    cut?;
    waited?;
    wellfed_rig::say!("Feeder powered again; stock looks its broker name up as it starts");
    Ok(())
}

#[cfg(test)]
mod tests {
    use wellfed_formats::bench::prompt::Expects;

    use super::*;

    #[test]
    fn install_values_go_as_one_word_each_with_lines_prompts_and_the_server_s_stock() {
        let args = InstallArgs {
            review: "private/r".into(),
            manifest: Some("private/m.json".into()),
            stock_config: "private/installer.json".into(),
            broker_name: Some("-x.example".into()),
            update_key: None,
            home_network: None,
            ble_address: None,
            expect_device: None,
            virtual_panel: false,
            from_bank0: false,
            enroll_minutes: Some(5),
            calibrate_clock: true,
            without_stock_plans: false,
            keys: None,
            address: None,
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
            Some("Stopped at pairing\n\nCaused by:\n    no answer")
        );
        assert_eq!(LastWords::default().message(), None);

        // The stock server's error, followed by more of the run, is not it.
        let mut words = LastWords::default();
        for line in ["Error: broker refused", "Started the stock server."] {
            words.hear(line);
        }
        assert_eq!(words.message(), None);
    }
}
