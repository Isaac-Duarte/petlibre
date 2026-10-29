//! How `wellfed install` asks the person at the feeder, and reads the
//! answer: at a terminal, or as JSON lines for the bench server that runs it
//! as a job (`wellfed_formats::bench::prompt`).
//!
//! In either mode a closed standard input is an error, never "no" and never
//! "go on": nobody is there to answer, and the install must not carry on as
//! if someone had. Standard input is read on a thread of its own, so a
//! signal that ends the run is seen while a prompt waits.

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
}

impl<W: Write> Asker<W> {
    fn new(prompts: Prompts, output: W, lines: mpsc::Receiver<io::Result<String>>) -> Self {
        Self {
            prompts,
            output,
            lines,
        }
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
        match self.prompts {
            Prompts::Terminal => {
                let how = match prompt.expects() {
                    Expects::Done => "Then press Enter.",
                    Expects::YesNo => "[y/N]",
                };
                writeln!(self.output, "{} {how}", prompt.text()).map_err(AskError::Write)?;
            }
            Prompts::Lines => {
                let line = serde_json::to_string(&prompt).map_err(AskError::Encode)?;
                writeln!(self.output, "{line}").map_err(AskError::Write)?;
            }
        }
        // A job's reader sees the line only once it is flushed.
        self.output.flush().map_err(AskError::Write)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// An asker whose standard input holds `lines`, then ends.
    fn scripted(prompts: Prompts, lines: &[&str]) -> Asker<Vec<u8>> {
        let (send, receive) = mpsc::channel(lines.len().max(1));
        for line in lines {
            send.try_send(Ok((*line).to_string())).unwrap();
        }
        Asker::new(prompts, Vec::new(), receive)
    }

    fn written(asker: &Asker<Vec<u8>>) -> Vec<String> {
        String::from_utf8(asker.output.clone())
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

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
