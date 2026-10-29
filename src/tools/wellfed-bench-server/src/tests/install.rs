//! The `install` job against a stand-in for `wellfed`: shell scripts
//! that write prompt lines, record the answers they read and the signal
//! that stops them.

use wellfed_formats::bench::banks::{Bank, Content, Source};
use wellfed_formats::bench::live::{AnswerError, PendingPrompt};
use wellfed_formats::bench::prompt::{Answer, Prompt};
use wellfed_relay::UART;

use super::*;

/// The words of an install job on the DNS route.
const INSTALL: &[&str] = &[
    "install",
    "--review",
    "private/verification/r",
    "--manifest",
    "private/m.json",
    "--stock-config",
    "private/installer.json",
    "--broker-name",
    "broker.example.net",
];

/// Write `script` as the checkout's `name`, executable.
fn stand_in(checkout: &Checkout, name: &str, script: &str) {
    let path = checkout.root().join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// An installer that asks for the restart, whether this is the feeder and
/// for the recovery window, recording each answer it reads in `answers`.
const ASKS: &str = r#"#!/bin/sh
echo "$@" > args
echo "Starting the stock server" >&2
echo '{"kind":"restart-feeder"}'
read a; echo "restart $a" >> answers
echo '{"kind":"recognize-feeder","device":"AF_TEST","address":"192.168.1.31"}'
read a; echo "recognize $a" >> answers
echo '{"kind":"open-recovery-window"}'
read a; echo "window $a" >> answers
echo "Recovery code 1234" >&2
echo "Installed" >&2
"#;

/// An installer that asks one question and waits, never reading the
/// answer, until a signal stops it.
const WAITS: &str = r#"#!/bin/sh
trap 'echo TERM > signal; kill $! 2>/dev/null; exit 1' TERM
echo '{"kind":"open-recovery-window"}'
sleep 30 & wait $!
"#;

fn read(checkout: &Checkout, name: &str) -> String {
    fs::read_to_string(checkout.root().join(name)).unwrap_or_default()
}

async fn prompt_until(server: &Running, seq: u32) -> PendingPrompt {
    let state = state_until(server, "the question", |state| {
        state
            .prompt
            .as_ref()
            .is_some_and(|prompt| prompt.seq == seq)
    })
    .await;
    state.prompt.unwrap()
}

fn refused(result: Result<(), ClientError>) -> AnswerError {
    match result {
        Err(ClientError::Refused(api::Refusal::Answer { error })) => error,
        other => panic!("not refused: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_install_restarts_the_feeder_itself_and_asks_the_rest_through_the_state() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.0);
    stand_in(&checkout, "wellfed", ASKS);
    checkout.board.lock().on[1] = true;
    let server = Running::start(checkout.open());

    let id = server.submit(INSTALL).await.unwrap();

    // The restart went by the relay, with no question in the state: the
    // first one put to anyone is whether this is the feeder.
    let asked = prompt_until(&server, 1).await;
    assert_eq!(
        asked.prompt,
        Prompt::RecognizeFeeder {
            device: "AF_TEST".into(),
            address: "192.168.1.31".into()
        }
    );
    assert_eq!(
        (asked.job, asked.text.as_str()),
        (id, &*asked.prompt.text())
    );
    assert_eq!(read(&checkout, "answers"), "restart \"done\"\n");
    let log = checkout.board.log();
    let off = log.iter().position(|step| *step == (POWER, false)).unwrap();
    let on = log.iter().rposition(|step| *step == (POWER, true)).unwrap();
    assert!(off < on, "{log:?}");
    let state = server.state().await;
    assert_eq!(state.activity[0].kind, ActivityKind::Install);
    assert_eq!(state.job.unwrap().label, "install");

    // Another job, another number, another kind: each refused, and the
    // question stays.
    let client = &server.client;
    assert_eq!(
        refused(client.answer(id + 1, 1, Answer::Yes).await),
        AnswerError::NotRunning { job: id + 1 }
    );
    assert_eq!(
        refused(client.answer(id, 2, Answer::Yes).await),
        AnswerError::WrongSeq {
            job: id,
            seq: 2,
            pending: 1
        }
    );
    assert!(matches!(
        refused(client.answer(id, 1, Answer::Done).await),
        AnswerError::Mismatched {
            answer: Answer::Done,
            ..
        }
    ));
    assert!(server.state().await.prompt.is_some());
    assert_eq!(read(&checkout, "answers"), "restart \"done\"\n");

    // The right answer reaches the installer, once.
    client.answer(id, 1, Answer::Yes).await.unwrap();
    let window = prompt_until(&server, 2).await;
    assert_eq!(window.prompt, Prompt::OpenRecoveryWindow);
    assert!(matches!(
        refused(client.answer(id, 1, Answer::Yes).await),
        AnswerError::WrongSeq { pending: 2, .. }
    ));
    let waited = client
        .wait(&Wait {
            what: WaitFor::Prompt,
            id: None,
            text: None,
            kind: None,
            build: None,
            bank: None,
            label: None,
            timeout_ms: 1_000,
        })
        .await
        .unwrap();
    assert!(
        matches!(&waited, api::Waited::Met { detail } if detail.contains("open-recovery-window")),
        "{waited:?}"
    );
    client.answer(id, 2, Answer::Done).await.unwrap();

    let (end, lines) = follow(client, id).await;
    assert_eq!((end.code, end.error), (0, None));
    assert_eq!(
        read(&checkout, "answers"),
        "restart \"done\"\nrecognize \"yes\"\nwindow \"done\"\n"
    );
    assert!(
        lines.iter().any(|line| line == "Recovery code 1234"),
        "{lines:?}"
    );
    let state = server.state().await;
    assert!(state.prompt.is_none());
    // No review files here, so what it installed reads as unknown; both
    // banks are the install's.
    for bank in [Bank::Zero, Bank::One] {
        assert_eq!(state.banks.banks[&bank].source, Source::Install, "{bank:?}");
    }
    let args = read(&checkout, "args");
    assert!(
        args.starts_with("install --prompts=lines --stock-server=")
            && args.contains("--broker-name=broker.example.net")
            && !args.contains("restart"),
        "{args}"
    );
    assert_eq!(
        refused(client.answer(id, 3, Answer::Done).await),
        AnswerError::NotRunning { job: id }
    );

    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_stops_the_installer_and_takes_its_question_away() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.0);
    stand_in(&checkout, "wellfed", WAITS);
    let server = Running::start(checkout.open());

    let id = server.submit(INSTALL).await.unwrap();
    prompt_until(&server, 1).await;
    server.client.cancel(id, DuringWrite::Refuse).await.unwrap();
    let (end, _) = follow(&server.client, id).await;

    assert_eq!(end.code, 1);
    assert_eq!(end.error.as_deref(), Some("Interrupted by the operator"));
    assert_eq!(read(&checkout, "signal").trim(), "TERM");
    let state = server.state().await;
    assert!(state.prompt.is_none() && state.activity.is_empty());
    // An answer that comes late reaches nothing.
    assert_eq!(
        refused(server.client.answer(id, 1, Answer::Done).await),
        AnswerError::NotRunning { job: id }
    );
    // It never needed relays to stop.
    assert!(checkout.board.log().is_empty());
    // The installer had started, so either write may have begun.
    for bank in [Bank::Zero, Bank::One] {
        let entry = &state.banks.banks[&bank];
        assert_eq!(
            (&entry.content, entry.source),
            (&Content::Unknown, Source::Install)
        );
    }

    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_line_that_is_no_prompt_stops_the_installer_and_its_own_error_ends_the_job() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.0);
    stand_in(
        &checkout,
        "wellfed",
        r#"#!/bin/sh
trap 'echo TERM > signal; kill $! 2>/dev/null; exit 1' TERM
echo 'Is this your feeder? [y/N]'
sleep 30 & wait $!
"#,
    );
    let server = Running::start(checkout.open());

    let id = server.submit(INSTALL).await.unwrap();
    let (end, lines) = follow(&server.client, id).await;

    assert!(
        end.error
            .as_deref()
            .is_some_and(|error| error.contains("where a prompt goes")),
        "{end:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "Is this your feeder? [y/N]")
    );
    assert_eq!(read(&checkout, "signal").trim(), "TERM");
    assert!(server.state().await.prompt.is_none());

    // An installer that stops on its own: its words are the job's error.
    stand_in(
        &checkout,
        "wellfed",
        r#"#!/bin/sh
echo "Starting the stock server" >&2
echo "Error: Stopped at pairing: the window closed" >&2
exit 1
"#,
    );
    let words: Vec<&str> = INSTALL
        .iter()
        .copied()
        .filter(|word| !word.starts_with("broker") && *word != "--broker-name")
        .chain(["--from-bank0"])
        .collect();
    // The stopped installer before it marked bank 0; this one starts
    // from bank 0 and leaves that entry as it was.
    let before = server.state().await.banks.banks[&Bank::Zero].clone();
    let id = server.submit(&words).await.unwrap();
    let (end, _) = follow(&server.client, id).await;
    assert_eq!(
        (end.code, end.error.as_deref()),
        (1, Some("Stopped at pairing: the window closed"))
    );
    let banks = server.state().await.banks.banks;
    assert_eq!(banks[&Bank::One].content, Content::Unknown);
    assert_eq!(banks[&Bank::Zero], before);

    server.quit().await;
}

/// A board that refuses to cut the first UART line, as a board that stops
/// answering part-way through a cut would.
struct RefusesUartCut(AtBoard);

impl Board for RefusesUartCut {
    fn set(&mut self, channel: u8, switch: Switch) -> wellfed_relay::Result<()> {
        if channel == UART[0] && switch == Switch::Off {
            return Err(wellfed_relay::Error::Refused {
                command: format!("AT+CH{channel}=0"),
                reply: "Error".into(),
            });
        }
        self.0.set(channel, switch)
    }

    fn get(&mut self, channel: u8) -> wellfed_relay::Result<bool> {
        self.0.get(channel)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_whose_cut_fails_part_way_powers_the_feeder_again() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.0);
    stand_in(&checkout, "wellfed", ASKS);
    checkout.board.lock().on[1] = true;
    let fake = checkout.board.clone();
    let opened = Server::open(checkout.options(), move |_| {
        Ok(Box::new(RefusesUartCut(AtBoard::new(Box::new(fake)))))
    })
    .unwrap();
    let server = Running::start(opened);

    let id = server.submit(INSTALL).await.unwrap();
    let (end, _) = follow(&server.client, id).await;

    assert!(
        end.error
            .as_deref()
            .is_some_and(|error| error.contains("AT+CH3=0")),
        "{end:?}"
    );
    // Relay 1 went off before the refused line, and on again after it.
    let log = checkout.board.log();
    let off = log.iter().position(|step| *step == (POWER, false)).unwrap();
    assert_eq!(log.last(), Some(&(POWER, true)), "{log:?}");
    assert!(off < log.len() - 1);
    assert!(checkout.board.state()[0]);
    // The restart was never answered, so no question reached the state.
    assert_eq!(read(&checkout, "answers"), "");

    server.quit().await;
}
