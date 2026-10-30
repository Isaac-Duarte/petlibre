//! How `wellfed install` asks the person at the feeder, and reads the
//! answer: at a terminal, or as JSON lines for the bench server that runs it
//! as a job (`wellfed_formats::bench::prompt`).
//!
//! In either mode a closed standard input is an error, never "no" and never
//! "go on": nobody is there to answer, and the install must not carry on as
//! if someone had. Standard input is read on a thread of its own, so a
//! signal that ends the run is seen while a prompt waits.
//!
//! A press at the panel inside a window the feeder times (the setup code,
//! the Dispense that enrolls an update key) is announced rather than asked
//! ([`Asker::announce`]): the run goes on at once, since waiting for the
//! answer would only spend the feeder's window, and the feeder, not the
//! answer, says whether the press came. In lines mode its answer is still
//! read, but only before the next prompt is written, so the bench server's
//! strict one-answer-per-prompt alternation holds. At a terminal nothing is
//! read for it, as before.

use std::io::{self, BufRead, Write};

use clap::ValueEnum;
use tokio::sync::mpsc;
use wellfed_formats::bench::prompt::{Answer, Expects, Prompt};

/// Where prompts go and how answers come back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum Prompts {
    /// Each prompt as text on standard error; Enter, or `y` for yes, on
    /// standard input.
    #[default]
    Terminal,
    /// Each prompt as one JSON line on standard output, and one JSON answer
    /// line back on standard input. Nothing else goes to standard output.
    Lines,
}

/// Why a prompt got no answer the run can act on.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AskError {
    #[error("standard input closed with the {} prompt unanswered", prompt.name())]
    Closed { prompt: Prompt },
    #[error("the answer to the {} prompt is not an answer: {line:?}", prompt.name())]
    Unreadable {
        prompt: Prompt,
        line: String,
        #[source]
        source: serde_json::Error,
    },
    /// A yes to an instruction, or done to a question.
    #[error("the {} prompt takes {}, not {answer}", prompt.name(), prompt.expects())]
    Mismatched { prompt: Prompt, answer: Answer },
    #[error("read standard input")]
    Read(#[source] io::Error),
    #[error("encode the prompt as JSON")]
    Encode(#[source] serde_json::Error),
    #[error("write the prompt")]
    Write(#[source] io::Error),
}

/// The run's one way to ask; it holds the mode for every prompt.
pub(crate) struct Asker<W> {
    prompts: Prompts,
    output: W,
    /// Standard input's lines, in order; closed at its end.
    lines: mpsc::Receiver<io::Result<String>>,
    /// An announced prompt whose answer has not been read yet (lines mode
    /// only).
    unread: Option<Prompt>,
}

impl Asker<Box<dyn Write>> {
    /// Ask on this process's standard streams. Call it only once anything
    /// else that reads standard input (the frame's passphrase) has.
    pub(crate) fn standard(prompts: Prompts) -> Self {
        let (send, lines) = mpsc::channel(1);
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                if send.blocking_send(line).is_err() {
                    return;
                }
            }
        });
        let output: Box<dyn Write> = match prompts {
            Prompts::Terminal => Box::new(io::stderr()),
            Prompts::Lines => Box::new(io::stdout()),
        };
        Self::new(prompts, output, lines)
    }

    /// An asker for a command that only announces, at the terminal: `wellfed
    /// setup`, `recover` and `update-key enroll` show the code or the press
    /// and read nothing back, so standard input is left alone.
    pub(crate) fn terminal_notices() -> Self {
        let (_, lines) = mpsc::channel(1);
        Self::new(Prompts::Terminal, Box::new(io::stderr()), lines)
    }
}

impl<W: Write> Asker<W> {
    fn new(prompts: Prompts, output: W, lines: mpsc::Receiver<io::Result<String>>) -> Self {
        Self {
            prompts,
            output,
            lines,
            unread: None,
        }
    }

    /// Show `prompt`, which takes done, and go on without waiting: at a
    /// terminal as `terminal`, the words a person there has always read;
    /// in lines mode as its line, its answer read before the next prompt.
    pub(crate) async fn announce(
        &mut self,
        prompt: Prompt,
        terminal: &str,
    ) -> Result<(), AskError> {
        self.settle().await?;
        match self.prompts {
            Prompts::Terminal => {
                writeln!(self.output, "{terminal}").map_err(AskError::Write)?;
            }
            Prompts::Lines => {
                self.write_line(&prompt)?;
                self.unread = Some(prompt);
            }
        }
        self.output.flush().map_err(AskError::Write)
    }

    /// Read the answer to the prompt announced last, if it is still
    /// unread: done, as it takes.
    pub(crate) async fn settle(&mut self) -> Result<(), AskError> {
        let Some(prompt) = self.unread.take() else {
            return Ok(());
        };
        match self.read(prompt).await? {
            (_, Answer::Done) => Ok(()),
            (prompt, answer) => Err(AskError::Mismatched { prompt, answer }),
        }
    }

    fn write_line(&mut self, prompt: &Prompt) -> Result<(), AskError> {
        let line = serde_json::to_string(prompt).map_err(AskError::Encode)?;
        writeln!(self.output, "{line}").map_err(AskError::Write)
    }

    /// Wait for the person to do what `prompt` says.
    pub(crate) async fn done(&mut self, prompt: Prompt) -> Result<(), AskError> {
        match self.ask(prompt).await? {
            (_, Answer::Done) => Ok(()),
            (prompt, answer) => Err(AskError::Mismatched { prompt, answer }),
        }
    }

    /// The person's yes or no to `prompt`.
    pub(crate) async fn yes(&mut self, prompt: Prompt) -> Result<bool, AskError> {
        match self.ask(prompt).await? {
            (_, Answer::Yes) => Ok(true),
            (_, Answer::No) => Ok(false),
            (prompt, answer) => Err(AskError::Mismatched { prompt, answer }),
        }
    }

    /// Show `prompt`, then read one answer of the kind it takes.
    async fn ask(&mut self, prompt: Prompt) -> Result<(Prompt, Answer), AskError> {
        self.settle().await?;
        match self.prompts {
            Prompts::Terminal => {
                let how = match prompt.expects() {
                    Expects::Done => "Then press Enter.",
                    Expects::YesNo => "[y/N]",
                };
                writeln!(self.output, "{} {how}", prompt.text()).map_err(AskError::Write)?;
            }
            Prompts::Lines => self.write_line(&prompt)?,
        }
        // A job's reader sees the line only once it is flushed.
        self.output.flush().map_err(AskError::Write)?;
        self.read(prompt).await
    }

    /// Read one answer to `prompt`, of the kind it takes.
    async fn read(&mut self, prompt: Prompt) -> Result<(Prompt, Answer), AskError> {
        let line = match self.lines.recv().await {
            Some(line) => line.map_err(AskError::Read)?,
            None => return Err(AskError::Closed { prompt }),
        };
        let answer = match (self.prompts, prompt.expects()) {
            (Prompts::Terminal, Expects::Done) => Answer::Done,
            (Prompts::Terminal, Expects::YesNo) => match line.trim() {
                "y" | "Y" | "yes" => Answer::Yes,
                _ => Answer::No,
            },
            (Prompts::Lines, _) => match serde_json::from_str(line.trim()) {
                Ok(answer) => answer,
                Err(source) => {
                    return Err(AskError::Unreadable {
                        prompt,
                        line,
                        source,
                    });
                }
            },
        };

        if !prompt.accepts(answer) {
            return Err(AskError::Mismatched { prompt, answer });
        }
        Ok((prompt, answer))
    }
}

/// An asker whose standard input holds `lines`, then ends.
#[cfg(test)]
pub(crate) fn scripted(prompts: Prompts, lines: &[&str]) -> Asker<Vec<u8>> {
    let (send, receive) = mpsc::channel(lines.len().max(1));
    for line in lines {
        send.try_send(Ok((*line).to_string())).unwrap();
    }
    Asker::new(prompts, Vec::new(), receive)
}

/// What `asker` wrote, line by line.
#[cfg(test)]
pub(crate) fn written(asker: &Asker<Vec<u8>>) -> Vec<String> {
    String::from_utf8(asker.output.clone())
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ask::{scripted, written};

    #[tokio::test]
    async fn each_prompt_is_one_json_line_and_each_answer_one_line_back() {
        let mut asker = scripted(Prompts::Lines, &[r#""done""#, r#""no""#]);

        asker.done(Prompt::RestartFeeder).await.unwrap();
        let recognize = Prompt::RecognizeFeeder {
            device: "AF_TEST".into(),
            address: "192.168.1.31".into(),
        };
        assert!(!asker.yes(recognize.clone()).await.unwrap());

        let lines = written(&asker);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], r#"{"kind":"restart-feeder"}"#);
        assert_eq!(
            serde_json::from_str::<Prompt>(&lines[1]).unwrap(),
            recognize
        );
    }

    #[tokio::test]
    async fn a_closed_input_is_no_answer_at_all() {
        for prompts in [Prompts::Lines, Prompts::Terminal] {
            let mut asker = scripted(prompts, &[]);
            let error = asker.yes(Prompt::EraseStockPlans).await.unwrap_err();
            assert!(
                matches!(
                    error,
                    AskError::Closed {
                        prompt: Prompt::EraseStockPlans
                    }
                ),
                "{error}"
            );
            let error = asker.done(Prompt::RestartFeeder).await.unwrap_err();
            assert!(matches!(error, AskError::Closed { .. }), "{error}");
        }
    }

    /// A yes does not confirm a restart, and done does not say yes.
    #[tokio::test]
    async fn an_answer_of_the_other_kind_is_refused() {
        let mut asker = scripted(Prompts::Lines, &[r#""yes""#, r#""done""#]);
        let error = asker.done(Prompt::RestartFeeder).await.unwrap_err();
        assert!(
            matches!(
                error,
                AskError::Mismatched {
                    prompt: Prompt::RestartFeeder,
                    answer: Answer::Yes
                }
            ),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "the restart-feeder prompt takes done, not yes"
        );
        let error = asker.yes(Prompt::EraseStockPlans).await.unwrap_err();
        assert!(
            matches!(
                error,
                AskError::Mismatched {
                    answer: Answer::Done,
                    ..
                }
            ),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_line_that_is_not_an_answer_is_refused() {
        let mut asker = scripted(Prompts::Lines, &["y"]);
        let error = asker.yes(Prompt::EraseStockPlans).await.unwrap_err();
        assert!(matches!(error, AskError::Unreadable { .. }), "{error}");
    }

    fn code() -> Prompt {
        use wellfed_formats::bench::prompt::CodePresses;
        use wellfed_session::setup_code::Button;

        Prompt::KeySetupCode {
            presses: CodePresses::new([Button::Lock; 6]),
        }
    }

    /// An announced press does not wait for its answer, which is read
    /// before the next prompt goes out, so each prompt still gets exactly
    /// one answer, in order.
    #[tokio::test]
    async fn an_announced_prompt_is_answered_before_the_next_one_is_written() {
        let mut asker = scripted(Prompts::Lines, &[r#""done""#, r#""done""#]);

        asker
            .announce(code(), "unused at a terminal")
            .await
            .unwrap();
        assert_eq!(written(&asker).len(), 1);
        assert_eq!(asker.lines.len(), 2, "nothing read on the announcement");

        asker.done(Prompt::JoinBack).await.unwrap();
        let lines = written(&asker);
        assert_eq!(serde_json::from_str::<Prompt>(&lines[0]).unwrap(), code());
        assert_eq!(lines[1], r#"{"kind":"join-back"}"#);
        assert!(asker.lines.is_empty());

        // An announcement that takes done refuses another kind of answer,
        // when it is read.
        let mut asker = scripted(Prompts::Lines, &[r#""yes""#]);
        asker.announce(code(), "").await.unwrap();
        let error = asker.yes(Prompt::EraseStockPlans).await.unwrap_err();
        assert!(
            matches!(
                error,
                AskError::Mismatched {
                    prompt: Prompt::KeySetupCode { .. },
                    answer: Answer::Yes
                }
            ),
            "{error}"
        );
    }

    /// At a terminal an announcement is today's words and reads nothing:
    /// the Enter that follows answers the next prompt.
    #[tokio::test]
    async fn a_terminal_announcement_shows_its_own_words_and_reads_nothing() {
        let mut asker = scripted(Prompts::Terminal, &[""]);
        asker
            .announce(code(), "Within 60 s, press these buttons")
            .await
            .unwrap();
        asker.done(Prompt::JoinBack).await.unwrap();

        let lines = written(&asker);
        assert_eq!(lines[0], "Within 60 s, press these buttons");
        assert!(
            lines[1].starts_with("Join this computer back"),
            "{}",
            lines[1]
        );
    }

    /// The terminal keeps its words: Enter goes on, and only `y` is yes.
    #[tokio::test]
    async fn the_terminal_reads_enter_and_y() {
        let mut asker = scripted(Prompts::Terminal, &["", "y", "nope"]);
        asker.done(Prompt::JoinBack).await.unwrap();
        assert!(asker.yes(Prompt::EraseStockPlans).await.unwrap());
        assert!(!asker.yes(Prompt::EraseStockPlans).await.unwrap());

        let lines = written(&asker);
        assert_eq!(
            lines[0],
            "Join this computer back to your own network. Then press Enter."
        );
        assert!(lines[1].ends_with("go on? [y/N]"), "{}", lines[1]);
    }
}
