//! The worker process the harness steps: one bounded request/response
//! transport over its stdin and stdout.

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use clap::ValueEnum;

use crate::protocol::{Command, Event, Journal, Profile};
use crate::{HarnessError, Result};

/// How long the worker may take to answer one command.
pub(crate) const RESPONSE: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 65536;
const MAX_EVENTS: usize = 10000;

/// One worker process: a bounded request/response transport, with stderr
/// kept for diagnostics.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<std::io::Result<String>>,
    errors: fs::File,
}

/// A value's command-line spelling.
fn argument(value: &impl ValueEnum) -> String {
    value
        .to_possible_value()
        .map(|possible| possible.get_name().to_owned())
        .unwrap_or_default()
}

impl Worker {
    pub fn start(
        executable: &Path,
        image: &Path,
        profile: Profile,
        journal: Journal,
    ) -> Result<Self> {
        let errors = tempfile::tempfile()?;
        let mut child = Process::new(executable)
            .arg(image)
            .arg(argument(&profile))
            .arg(argument(&journal))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(errors.try_clone()?)
            .spawn()
            .map_err(HarnessError::WorkerStart)?;

        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            unreachable!("both pipes were requested above");
        };
        let (send, lines) = channel();
        // A reader thread feeds a channel, so `read` can wait with a deadline.
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line).is_err() {
                    return;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            lines,
            errors,
        })
    }

    /// Events up to and including the next status or power cut.
    pub fn read(&mut self) -> Result<Vec<Event>> {
        let deadline = Instant::now() + RESPONSE;
        let (mut events, mut bytes) = (Vec::new(), 0);

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(Ok(line)) => line,
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return Err(self.exited()),
                Err(RecvTimeoutError::Timeout) => return Err(HarnessError::WorkerTimeout),
            };

            bytes += line.len() + 1;
            let event: Event = serde_json::from_str(&line).map_err(HarnessError::WorkerEvent)?;

            let done = matches!(event, Event::Status(_) | Event::PowerCut { .. });
            events.push(event);
            if done {
                return Ok(events);
            }
            if bytes > MAX_RESPONSE_BYTES || events.len() > MAX_EVENTS {
                return Err(HarnessError::WorkerBounds);
            }
        }
    }

    fn exited(&mut self) -> HarnessError {
        let code = self.wait(RESPONSE).ok().flatten();

        let mut detail = String::new();
        let _ = self.errors.rewind();
        let _ = (&self.errors).take(4096).read_to_string(&mut detail);

        HarnessError::WorkerExited { code, detail }
    }

    pub fn command(&mut self, command: &Command) -> Result<Vec<Event>> {
        let line = serde_json::to_string(command)?;
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|_| HarnessError::WorkerPipe)?;
        self.read()
    }

    /// The exit status, if the worker exits within `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> std::io::Result<Option<i32>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status.code().unwrap_or(-1)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
