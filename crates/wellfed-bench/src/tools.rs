//! The child processes the bench chains: `wellfed-flash` and `sigrok-cli`.
//!
//! `wellfed-flash` runs as its own process, never linked in. Its plans bind
//! the SHA-256 of the running executable, and that has to stay the flash
//! client's own. It keeps its stdout summary, exit codes and Ctrl-C handling.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::status::{FlashTool, Status, relative};
use crate::{Error, Result, interrupted, require, sha256_hex};

/// How long a receive-only capture may take to hold the UART port.
const OBSERVER_START: Duration = Duration::from_secs(10);

pub trait Tools {
    /// Start a receive-only capture and return once it holds the UART port.
    fn observe(&mut self, port: &str, output: &Path, seconds: f64) -> Result<Box<dyn Observer>>;
    /// Run one `wellfed-flash` command to completion, echoing and recording
    /// its output; returns its exit status (0 complete, 1 stopped, 2 refused).
    fn flash(&mut self, args: &[String], status: &mut Status) -> Result<i32>;
    /// Start one bounded sigrok capture of all eight channels as raw bytes.
    fn analyzer(
        &mut self,
        output: &Path,
        seconds: f64,
        samplerate: u64,
    ) -> Result<Box<dyn Analyzer>>;
    fn sigrok_available(&self) -> bool;
    fn flash_tool(&self) -> Option<FlashTool>;
}

pub trait Observer {
    /// Wait for the capture to end; its report's `result` on success.
    fn finish(&mut self) -> Result<Value>;
    fn kill(&mut self);
}

pub trait Analyzer {
    fn running(&mut self) -> bool;
    /// The exit status, or None if it is still running after `timeout`.
    fn wait(&mut self, timeout: Duration) -> Result<Option<i32>>;
    fn kill(&mut self);
    fn stderr(&mut self) -> String;
}

/// The real tools, run from the checkout root.
pub struct Processes {
    pub root: PathBuf,
    pub flash_bin: PathBuf,
}

/// `wellfed-flash` beside this executable, as `cargo build` places it.
pub fn default_flash_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("wellfed-flash")))
        .unwrap_or_else(|| PathBuf::from("wellfed-flash"))
}

pub fn flash_tool(root: &Path, path: &Path) -> Option<FlashTool> {
    let data = fs::read(path).ok()?;
    Some(FlashTool {
        path: relative(root, path),
        sha256: sha256_hex(&data),
    })
}

impl Processes {
    fn flash_command(&self) -> Result<Command> {
        require(
            self.flash_bin.is_file(),
            format!(
                "{} not found; build it with cargo build -p wellfed-flash or pass --flash-bin",
                self.flash_bin.display()
            ),
        )?;
        let mut command = Command::new(&self.flash_bin);
        command
            .current_dir(&self.root)
            .arg("--root")
            .arg(&self.root);
        Ok(command)
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    // A signal-ended child counts as stopped.
    status.code().unwrap_or(1)
}

impl Tools for Processes {
    fn observe(&mut self, port: &str, output: &Path, seconds: f64) -> Result<Box<dyn Observer>> {
        let child = self
            .flash_command()?
            .args(["read", "observe", "--port", port, "--output"])
            .arg(output)
            .args(["--seconds", &seconds.to_string()])
            .stdout(Stdio::null())
            .spawn()?;
        let mut observer = Observing {
            child,
            output: output.to_path_buf(),
        };

        // The observer's report says `running` once it holds the port.
        let report = output.join("report.json");
        let deadline = Instant::now() + OBSERVER_START;
        while Instant::now() < deadline {
            if interrupted() {
                observer.kill();
                return Err(Error::Interrupted);
            }
            if observer.child.try_wait()?.is_some() {
                return Err(Error::Bench(
                    "Observer exited before holding the port".into(),
                ));
            }
            let running = fs::read_to_string(&report)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|report| report["status"] == "running");
            if running {
                return Ok(Box::new(observer));
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        observer.kill();
        Err(Error::Bench(
            "Observer did not hold the port in time".into(),
        ))
    }

    fn flash(&mut self, args: &[String], status: &mut Status) -> Result<i32> {
        let mut child = self
            .flash_command()?
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Drain stdout on its own thread so neither pipe fills and blocks the
        // child while stderr streams here.
        let mut stdout = child.stdout.take().expect("piped stdout");
        let summary = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        });

        for line in BufReader::new(child.stderr.take().expect("piped stderr")).lines() {
            let line = line?;
            eprintln!("{line}");
            status.line(&line);
        }

        let code = exit_code(child.wait()?);
        // The one-line JSON summary: shown and kept with the progress.
        for line in summary.join().unwrap_or_default().lines() {
            println!("{line}");
            status.line(line);
        }
        Ok(code)
    }

    fn analyzer(
        &mut self,
        output: &Path,
        seconds: f64,
        samplerate: u64,
    ) -> Result<Box<dyn Analyzer>> {
        let channels: Vec<String> = (0..8).map(|i| format!("D{i}")).collect();
        let child = Command::new("sigrok-cli")
            .current_dir(&self.root)
            .args(["--driver", "fx2lafw", "--config"])
            .arg(format!("samplerate={samplerate}"))
            .arg("--time")
            .arg(((seconds * 1000.0) as u64).to_string())
            .arg("--channels")
            .arg(channels.join(","))
            .args(["--output-format", "binary", "--output-file"])
            .arg(output)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(Box::new(Sigrok(child)))
    }

    fn sigrok_available(&self) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("sigrok-cli").is_file())
        })
    }

    fn flash_tool(&self) -> Option<FlashTool> {
        flash_tool(&self.root, &self.flash_bin)
    }
}

struct Observing {
    child: Child,
    output: PathBuf,
}

impl Observer for Observing {
    fn finish(&mut self) -> Result<Value> {
        let code = exit_code(self.child.wait()?);
        require(
            code == 0,
            format!("Observer failed; see {}", self.output.display()),
        )?;
        let report: Value =
            serde_json::from_str(&fs::read_to_string(self.output.join("report.json"))?)?;
        Ok(report["result"].clone())
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Sigrok(Child);

impl Analyzer for Sigrok {
    fn running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }

    fn wait(&mut self, timeout: Duration) -> Result<Option<i32>> {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(Some(exit_code(status)));
            }
            if interrupted() {
                return Err(Error::Interrupted);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }

    fn stderr(&mut self) -> String {
        let mut text = String::new();
        if let Some(stderr) = &mut self.0.stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `wellfed-flash`: progress on stderr, a summary on stdout.
    fn fake_flash(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("wellfed-flash");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn flash_streams_progress_and_returns_the_exit_status() {
        let tmp = tempfile::tempdir().unwrap();
        let flash_bin = fake_flash(
            tmp.path(),
            r#"echo "Read 4096/8192 bytes" >&2; echo '{"status":"failed"}'; exit 1"#,
        );
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin,
        };
        let mut status = Status::new(tmp.path(), tmp.path().join("status"));
        status.begin("dump", Some("unit"), &[]).unwrap();

        let code = tools
            .flash(&["read".into(), "full".into()], &mut status)
            .unwrap();

        assert_eq!(code, 1);
        let state = status.state.unwrap();
        assert_eq!(state.progress.unwrap().done, 4096);
        assert_eq!(state.lines.last().unwrap(), r#"{"status":"failed"}"#);
    }

    #[test]
    fn observer_waits_for_the_port_then_returns_the_result() {
        let tmp = tempfile::tempdir().unwrap();
        // Arguments: --root R read observe --port P --output O --seconds S
        let flash_bin = fake_flash(
            tmp.path(),
            r#"mkdir -p "$8"; echo '{"status":"running"}' > "$8/report.json"; sleep 0.2
echo '{"status":"complete","result":{"bytes":7}}' > "$8/report.json""#,
        );
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin,
        };
        let output = tmp.path().join("capture");

        let mut observer = tools.observe("uart", &output, 1.0).unwrap();
        assert_eq!(observer.finish().unwrap()["bytes"], 7);
    }

    #[test]
    fn an_observer_that_exits_early_is_a_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let flash_bin = fake_flash(tmp.path(), "exit 2");
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin,
        };

        let error = tools
            .observe("uart", &tmp.path().join("c"), 1.0)
            .err()
            .unwrap();

        assert!(error.to_string().contains("exited before holding the port"));
    }

    #[test]
    fn a_missing_flash_binary_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin: tmp.path().join("absent"),
        };
        let mut status = Status::disabled();

        let error = tools.flash(&[], &mut status).unwrap_err();
        assert!(error.to_string().contains("cargo build -p wellfed-flash"));
    }
}
