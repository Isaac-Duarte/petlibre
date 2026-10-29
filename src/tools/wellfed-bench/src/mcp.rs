//! `wellfed-bench mcp`: the bench's tools for an agent, as a Model Context
//! Protocol server on stdin and stdout, which is a client of the running
//! bench server's socket like every other command.
//!
//! It holds nothing of its own: no lease, no board, no session. A hardware
//! tool posts the words a person would type, so the server parses them and
//! every gate behind the command applies (the plan hash, the strap check,
//! the lease, the no-retry rule). The tools add their own refusals for what
//! needs a person at the bench (`tools`). Device requests are reads and a
//! clock sync only, through the server's one session; a read that came
//! back uncertain is reported as such and never sent again.
//!
//! Stdout carries the protocol alone, so nothing here prints to it.

mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::Value;
use wellfed_bench_server::Client;
use wellfed_bench_server::api::{ReadOnly, WAIT_MAX_MS, Wait, WaitFor};
use wellfed_formats::bench::live::BenchState;
use wellfed_formats::private::{SYSTEM_RANDOM, random};

use tools::{Call, LogSource, Until, WaitArgs};

/// What an agent reads first.
const INSTRUCTIONS: &str = "The PLAF103 research bench, through its running `wellfed-bench serve`. \
Hardware tools post a job and return its number: follow it with wait_for {for: job} and job_output. \
One job runs at a time. Write tools (flash_execute, give_way_execute, stock_execute) need the plan \
SHA-256 a prepare printed, after review. During a UART write nothing on the bench may be touched and \
the bench host should stay idle. Motion needs a person at the bench and is refused here. Record steps, \
timers and evidence with the note tools. Label findings physical observation, static analysis or \
simulation.";

/// The newest lines a trimmed state keeps of each tail.
const TRIMMED_TAIL: usize = 20;

pub(crate) async fn serve(root: &Path) -> anyhow::Result<u8> {
    let bench = Bench {
        client: Arc::new(Client::new(root)),
        root: root.to_path_buf(),
    };

    let running = bench
        .serve(rmcp::transport::stdio())
        .await
        .context("Starting the MCP session on stdio")?;
    running.waiting().await?;
    Ok(0)
}

#[derive(Clone)]
struct Bench {
    client: Arc<Client>,
    root: PathBuf,
}

impl ServerHandler for Bench {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "wellfed-bench",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        let done = match tools::call(&request.name, args) {
            Ok(call) => self.run(call).await.map_err(|error| format!("{error:#}")),
            Err(error) => Err(error.to_string()),
        };

        let result = match done {
            Ok(value) => {
                let text =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
        };
        Ok(result.into())
    }
}

impl Bench {
    async fn run(&self, call: Call) -> anyhow::Result<Value> {
        let client = &self.client;
        Ok(match call {
            Call::Status => serde_json::to_value(trim(client.state().await?))?,
            Call::JobOutput { id, after } => serde_json::to_value(client.output(id, after).await?)?,
            Call::JobCancel { id } => {
                let id = match id {
                    Some(id) => id,
                    None => client
                        .state()
                        .await?
                        .job
                        .filter(|job| job.end.is_none())
                        .map(|job| job.id)
                        .context("No job is running")?,
                };
                client.cancel(id).await?;
                serde_json::json!({ "cancelled": id, "next": "wait_for {for: job, id} for its cleanup" })
            }
            Call::WaitFor(args) => serde_json::to_value(client.wait(&wait(&args)?).await?)?,
            Call::LogsTail { source, lines } => self.tail(source, lines).await?,
            Call::Banks => serde_json::to_value(client.banks().await?)?,

            Call::Note(words) => {
                let args = crate::note::from_words(&words)?;
                let body = crate::note::body(args, &self.root, &self.root, &jiff::Zoned::now())?;
                let note = client.note(&body).await?;
                serde_json::json!({ "noted": crate::note::describe(&note) })
            }

            Call::Device(op) => {
                let read: ReadOnly = serde_json::from_value(op).context(
                    "Only reads and the clock sync go to the feeder from here: status, settings, \
                     time, plans, clients, update_state, calibration, call or stats",
                )?;
                let id = random::<16>(Path::new(SYSTEM_RANDOM))?;
                serde_json::to_value(client.ask(id, read.op(jiff::Timestamp::now())).await?)?
            }

            Call::Job(words) => {
                let id = client.submit(&self.root, &words).await?;
                serde_json::json!({
                    "job": id,
                    "words": words,
                    "next": "wait_for {for: job, id}, then job_output {id}",
                })
            }
        })
    }

    async fn tail(&self, source: LogSource, lines: usize) -> anyhow::Result<Value> {
        let newest = |all: &[String]| all[all.len().saturating_sub(lines)..].to_vec();
        let kept = match source {
            LogSource::Device => newest(&self.client.state().await?.feeder.log),
            LogSource::Uart => newest(&self.client.state().await?.capture.lines),
            LogSource::Job => {
                let job = self
                    .client
                    .state()
                    .await?
                    .job
                    .context("No job yet on this server")?;
                let end = self.client.output(job.id, u64::MAX).await?.next;
                let from = end.saturating_sub(u64::try_from(lines).unwrap_or(u64::MAX));
                self.client.output(job.id, from).await?.lines
            }
        };
        Ok(serde_json::json!({ "lines": kept }))
    }
}

/// The server's wait, from the tool's: seconds become milliseconds, at
/// most the server's longest.
fn wait(args: &WaitArgs) -> anyhow::Result<Wait> {
    anyhow::ensure!(
        args.timeout.is_finite() && args.timeout >= 0.0,
        "timeout {} must be a number of seconds",
        args.timeout
    );
    let timeout = std::time::Duration::try_from_secs_f64(args.timeout)?;
    let timeout_ms = u64::try_from(timeout.as_millis())
        .unwrap_or(u64::MAX)
        .min(WAIT_MAX_MS);
    let what = match args.until {
        Until::Job => WaitFor::Job,
        Until::Line => WaitFor::Line,
        Until::Event => WaitFor::Event,
        Until::Status => WaitFor::Status,
        Until::Timer => WaitFor::Timer,
        Until::Idle => WaitFor::Idle,
    };
    Ok(Wait {
        what,
        id: args.id,
        text: args.text.clone(),
        kind: args.kind.clone(),
        build: args.build.clone(),
        bank: args.bank,
        label: args.label.clone(),
        timeout_ms,
    })
}

/// The state without its long tails, which `logs_tail` reads.
fn trim(mut state: BenchState) -> BenchState {
    let keep = |lines: &mut Vec<String>| {
        let drop = lines.len().saturating_sub(TRIMMED_TAIL);
        lines.drain(..drop);
    };
    keep(&mut state.feeder.log);
    keep(&mut state.capture.lines);
    if let Some(record) = state.job.as_mut().and_then(|job| job.record.as_mut()) {
        keep(&mut record.lines);
    }
    state
}
