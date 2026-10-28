//! `wellfed server …`: the owner's server over its HTTP API, through
//! `wellfed-server-client` alone ([API design]).
//!
//! `login` keeps a bearer token beside the device API's key file, and every
//! other verb sends it. The device verbs (`feeders`, `status`, `settings`,
//! `lock`, `desiccant`, `feed`, `cancel-feed`, `events`, `clients`, `revoke`, `reboot`)
//! reach the feeders the server holds, through its command journal; the
//! rest (`command`, `commands`, `users`, `tokens`, `releases`, `install`,
//! `stock`, `export`) only the server has.
//!
//! Output is text by default, and with `--json` the API's bodies, one JSON
//! document per line (a refusal's problem included). The exit status is 0
//! on success, 1 on an error and 3 when a write's outcome is unknown.
//!
//! A write goes out once, under a fresh `Idempotency-Key`, and is never
//! sent again by this command: not on this path under a new key, and never
//! over the device API. When its outcome is unknown (the server answered
//! `outcome_unknown`, or the answer was lost on the way), the command
//! prints the key and `wellfed server command <key>`, which reads the
//! journal entry the feeder's history settles.
//!
//! [API design]: ../../../../../docs/server/design.md#the-cli-on-the-server

mod accounts;
mod clients;
mod export;
mod feeders;
mod install;
mod login;
mod releases;
mod stock;
mod text;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use serde::Serialize;
use uuid::Uuid;
use wellfed_server_client::Error;
use wellfed_server_client::api::{ErrorCode, Problem};

/// The exit status of a write whose outcome is unknown.
const UNKNOWN: u8 = 3;

#[derive(Args)]
pub(crate) struct ServerArgs {
    /// Print the API's bodies as JSON, one per line, instead of text.
    #[arg(long, global = true)]
    json: bool,
    /// Where `login` keeps the server's address and its token (mode 0600),
    /// beside the device API's key file.
    #[arg(long, global = true, default_value = "private/wellfed-server.json")]
    token_file: PathBuf,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Log in with a password (read from standard input), and keep a new
    /// token for this computer.
    Login(login::LoginArgs),
    /// Revoke the kept token and delete its file.
    Logout,
    /// The feeders the server holds, with its connection to each.
    Feeders,
    /// One feeder and the status the server last read.
    Status(feeders::Which),
    /// Read the feeder's settings, or change them against the generation
    /// just read.
    Settings(feeders::SettingsArgs),
    /// Lock or unlock the feeder's panel.
    Lock(feeders::LockArgs),
    /// The desiccant reminder, and with --reset record that it was
    /// replaced today. Its period is a setting.
    Desiccant(feeders::DesiccantArgs),
    /// Dispense portions, and wait for the feed's result.
    Feed(feeders::FeedArgs),
    /// Stop a running feed.
    CancelFeed(feeders::CancelFeedArgs),
    /// The feeder's clients, and whose each is. Owners only.
    Clients(feeders::Which),
    /// Revoke one of the feeder's clients. Owners only.
    Revoke(clients::RevokeArgs),
    /// Restart the feeder; never sent again when its outcome is unknown.
    /// Owners only.
    Reboot(feeders::Which),
    /// The feeder's history as the server holds it, and with --follow its
    /// new events.
    Events(feeders::EventsArgs),
    /// A write sent through the server, by its key: how an unknown outcome
    /// is resolved.
    Command(feeders::CommandArgs),
    /// The writes sent through the server, oldest first.
    Commands(feeders::CommandsArgs),
    /// The household's accounts. Owners only.
    #[command(subcommand)]
    Users(accounts::Users),
    /// The tokens: your own, or everyone's for an owner.
    #[command(subcommand)]
    Tokens(accounts::Tokens),
    /// The releases the server keeps for installs. Owners only.
    #[command(subcommand)]
    Releases(releases::Releases),
    /// Install a kept release on a feeder and follow it to its end
    /// (owners only), or without one read the feeder's running or last
    /// install.
    Install(install::InstallArgs),
    /// The feeder's stock backup on the server: show it, read it from the
    /// feeder, upload or export it, or restore stock from it (owners
    /// only, signed here with your update key).
    Stock(stock::StockArgs),
    /// Save everything the server holds as one archive (a tar of a
    /// manifest and a file per table), with no secrets in it. Owners only.
    Export(export::ExportArgs),
}

pub(crate) async fn run(args: ServerArgs) -> Result<ExitCode> {
    let out = Output { json: args.json };
    let ran = verb(args.verb, &args.token_file, out).await;

    match ran {
        Ok(Settled::Known) => Ok(ExitCode::SUCCESS),
        Ok(Settled::Unknown) => Ok(ExitCode::from(UNKNOWN)),
        Err(error) => Err(out.failed(error)),
    }
}

async fn verb(verb: Verb, token_file: &Path, out: Output) -> Result<Settled> {
    // Every verb but the login's own sends the kept token.
    let client = || login::client(token_file);

    match verb {
        Verb::Login(args) => login::login(args, token_file, out).await,
        Verb::Logout => login::logout(token_file, out).await,
        Verb::Feeders => feeders::list(&client()?, out).await,
        Verb::Status(which) => feeders::status(&client()?, &which, out).await,
        Verb::Settings(args) => feeders::settings(&client()?, args, out).await,
        Verb::Lock(args) => feeders::lock(&client()?, args, out).await,
        Verb::Desiccant(args) => feeders::desiccant(&client()?, args, out).await,
        Verb::Feed(args) => feeders::feed(&client()?, args, out).await,
        Verb::CancelFeed(args) => feeders::cancel_feed(&client()?, args, out).await,
        Verb::Clients(which) => clients::clients(&client()?, &which, out).await,
        Verb::Revoke(args) => clients::revoke(&client()?, args, out).await,
        Verb::Reboot(which) => clients::reboot(&client()?, &which, out).await,
        Verb::Events(args) => feeders::events(&client()?, args, out).await,
        Verb::Command(args) => feeders::command(&client()?, &args, out).await,
        Verb::Commands(args) => feeders::commands(&client()?, args, out).await,
        Verb::Users(users) => accounts::users(&client()?, users, out).await,
        Verb::Tokens(tokens) => accounts::tokens(&client()?, tokens, out).await,
        Verb::Releases(releases) => releases::releases(&client()?, releases, out).await,
        Verb::Install(args) => install::install(&client()?, args, out).await,
        Verb::Stock(args) => stock::stock(&client()?, args, out).await,
        Verb::Export(args) => export::export(&client()?, args, out).await,
    }
}

/// How a command ended, short of an error: the exit status's 0 or 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    Known,
    /// A write may or may not have run, and the journal does not say yet.
    Unknown,
}

/// What a keyed write came to.
enum Written<T> {
    Answered(T),
    /// It may have run: its key and the command that resolves it are
    /// printed, and it is not sent again.
    Unknown,
}

/// Where a command prints: text for a person, or the API's bodies.
#[derive(Clone, Copy)]
struct Output {
    json: bool,
}

impl Output {
    /// An API body, as one line of JSON. Text output prints none.
    fn body(self, body: &impl Serialize) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string(body)?);
        }
        Ok(())
    }

    /// A line for a person. JSON output prints none.
    fn text(self, line: impl std::fmt::Display) {
        if !self.json {
            println!("{line}");
        }
    }

    /// A body as JSON, or as the text `show` makes of it.
    fn show<T: Serialize>(self, body: &T, show: impl FnOnce(&T) -> String) -> Result<()> {
        if self.json {
            return self.body(body);
        }
        let text = show(body);
        if !text.is_empty() {
            println!("{text}");
        }
        Ok(())
    }

    /// Say that the write under `key` may have run: the server's problem
    /// as a body, and the key with the command that resolves it, on
    /// standard output as text or on standard error beside JSON.
    fn unknown(self, key: Uuid, error: Option<&Error>) -> Result<()> {
        if let Some(Error::Problem { problem, .. }) = error {
            self.body(problem)?;
        }
        let lines = format!(
            "The outcome is unknown: the feeder may have run it, and it is not sent again.\n\
             Key: {key}\n\
             Resolve it with: wellfed server command {key}"
        );
        if self.json {
            eprintln!("{lines}");
        } else {
            println!("{lines}");
        }
        Ok(())
    }

    /// The command's error, its body printed first under `--json`: a
    /// problem, or the current settings a stale edit is answered with.
    fn failed(self, error: anyhow::Error) -> anyhow::Error {
        let Some(client) = error.downcast_ref::<Error>() else {
            return error;
        };

        let printed = match client {
            Error::Problem { problem, .. } => self.body(problem),
            Error::Stale(settings) => self.body(settings),
            _ => Ok(()),
        };
        if let Err(print) = printed {
            return print;
        }

        match client {
            Error::Problem {
                status, problem, ..
            } => anyhow!("{}", refusal(*status, problem)),
            _ => error,
        }
    }
}

/// A refusal as one line: the status, the title, and what the problem
/// adds.
fn refusal(status: wellfed_server_client::StatusCode, problem: &Problem) -> String {
    let mut line = format!("the server answered {status}: {}", problem.title);
    if let Some(detail) = &problem.detail {
        line.push_str(&format!(" ({detail})"));
    }
    if let Some(refusal) = problem.refusal {
        line.push_str(&format!(
            "; the feeder refused it: {}",
            text::word(&refusal)
        ));
    }
    if let Some(release) = problem.release {
        line.push_str(&format!("; the release: {}", text::word(&release)));
    }
    line
}

/// Send one keyed write, once. Whatever may have run comes back as
/// [`Written::Unknown`], already reported; a refusal that ran nothing is
/// the error.
async fn write<T>(
    out: Output,
    key: Uuid,
    request: impl Future<Output = Result<T, Error>>,
) -> Result<Written<T>> {
    match request.await {
        Ok(answer) => Ok(Written::Answered(answer)),
        Err(error) if uncertain(&error) => {
            out.unknown(key, Some(&error))?;
            Ok(Written::Unknown)
        }
        Err(error) => Err(error.into()),
    }
}

/// Whether a write that failed this way may have run. Only a refusal the
/// server answered, or a connection never made, says it did not; a lost
/// or garbled answer may follow a write that ran.
fn uncertain(error: &Error) -> bool {
    match error {
        Error::Problem { problem, .. } => {
            matches!(
                problem.code,
                ErrorCode::OutcomeUnknown | ErrorCode::Internal
            )
        }
        Error::Http(error) => !error.is_connect() && !error.is_builder(),
        Error::Unexpected { .. } | Error::Decode(_) => true,
        Error::Base(_) | Error::Build(_) | Error::Stale(_) | Error::NoSession => false,
    }
}

#[cfg(test)]
mod tests {
    use wellfed_server_client::StatusCode;

    use super::*;

    fn problem(code: ErrorCode) -> Error {
        Error::Problem {
            status: StatusCode::GATEWAY_TIMEOUT,
            problem: Box::new(Problem {
                kind: "about:blank".into(),
                title: "t".into(),
                status: 504,
                code,
                detail: None,
                refusal: None,
                release: None,
            }),
            retry_after: None,
        }
    }

    #[test]
    fn only_an_answered_refusal_says_a_write_did_not_run() {
        assert!(uncertain(&problem(ErrorCode::OutcomeUnknown)));
        assert!(uncertain(&problem(ErrorCode::Internal)));
        assert!(uncertain(&Error::Unexpected {
            status: StatusCode::BAD_GATEWAY,
            expected: StatusCode::ACCEPTED,
        }));

        for refused in [
            ErrorCode::FeederOffline,
            ErrorCode::FeederRefused,
            ErrorCode::KeyReused,
            ErrorCode::Forbidden,
        ] {
            assert!(!uncertain(&problem(refused)), "{refused:?}");
        }
        assert!(!uncertain(&Error::NoSession));
    }
}
