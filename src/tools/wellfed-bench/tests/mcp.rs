//! `wellfed-bench mcp` end to end over stdio, as an agent's client speaks
//! it, against a bench server in a temporary checkout on a scripted relay
//! board. One test: the server takes descriptors 1 and 2 for its output
//! pipe, which no other test in this process may share.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use wellfed_bench_server::{Console, Options, Server, ToolPaths};
use wellfed_relay::{AtBoard, FakeBoard};

struct Agent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Agent {
    fn start(root: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wellfed-bench"))
            .arg("--root")
            .arg(root)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next: 1,
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// One request, and the response with its id.
    fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "the MCP server closed stdout"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    /// A tool's result: whether it is an error, and its text.
    fn call(&mut self, tool: &str, arguments: Value) -> (bool, String) {
        let response = self.ask("tools/call", json!({"name": tool, "arguments": arguments}));
        let result = &response["result"];
        assert!(result.is_object(), "{response}");
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (result["isError"] == json!(true), text)
    }
}

#[test]
fn an_agent_lists_reads_posts_a_job_waits_and_is_refused_motion_and_feeds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(
        root.join("private/bench.json"),
        r#"{"relay_port": "relay", "uart_port": "uart", "baseline": "b", "rom_report": "r"}"#,
    )
    .unwrap();
    let options = Options {
        root: root.clone(),
        tools: ToolPaths {
            flash_bin: root.join("absent"),
            stock_bin: root.join("absent"),
            build_bin: root.join("absent"),
        },
        console: Console::Terminal,
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = Server::open(options, |_| {
        Ok(Box::new(AtBoard::new(Box::new(FakeBoard::default()))))
    })
    .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = runtime.spawn(server.run(async {
        let _ = stopped.await;
    }));
    let mut agent = Agent::start(&root);

    let hello = agent.ask(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    assert_eq!(
        hello["result"]["serverInfo"]["name"],
        json!("wellfed-bench"),
        "{hello}"
    );
    agent.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let listed = agent.ask("tools/list", json!({}));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in [
        "bench_status",
        "wait_for",
        "note_timer",
        "flash_execute",
        "power",
    ] {
        assert!(names.contains(&name), "{name} not in {names:?}");
    }

    let (failed, status) = agent.call("bench_status", json!({}));
    assert!(!failed, "{status}");
    let status: Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["server"]["phase"], json!("serving"));

    // A hardware tool posts the job's words; the server runs it.
    let (failed, posted) = agent.call("power", json!({"state": "on"}));
    assert!(!failed, "{posted}");
    let job = serde_json::from_str::<Value>(&posted).unwrap()["job"].clone();
    let (failed, waited) = agent.call("wait_for", json!({"for": "job", "id": job, "timeout": 20}));
    assert!(!failed, "{waited}");
    assert!(waited.contains("\"met\""), "{waited}");
    let (_, status) = agent.call("bench_status", json!({}));
    let status: Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["relays"]["1"]["on"], json!(true), "{status}");

    // What needs a person at the bench never reaches it.
    let (failed, refused) = agent.call(
        "flash_execute",
        json!({"name": "u1", "target": "t", "plan_sha256": "00", "attended_motion": true}),
    );
    assert!(failed && refused.contains("attended_motion"), "{refused}");
    let (failed, refused) = agent.call("device_read", json!({"op": "feed", "portions": 1}));
    assert!(failed && refused.contains("Only reads"), "{refused}");

    // A wait that nothing meets ends at its timeout.
    let (failed, waited) = agent.call(
        "wait_for",
        json!({"for": "line", "text": "never printed", "timeout": 0.3}),
    );
    assert!(!failed, "{waited}");
    assert!(waited.contains("timed-out"), "{waited}");

    drop(agent.stdin);
    agent.child.wait().unwrap();
    let _ = stop.send(());
    runtime.block_on(served).unwrap().unwrap();
    wellfed_bench_server::restore_output();
}
