// Serve a synthetic bench for the end-to-end tests. The live run's status
// names this process as its owner, so the page reads it as running for as
// long as the server is up.
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const port = process.argv[2] ?? "8797";
const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../../../..");
const binary = process.env.BENCH_BIN ?? join(repo, "target/debug/petlibre-bench");

const root = mkdtempSync(join(tmpdir(), "petlibre-e2e-"));
const write = (path, data) => {
  const full = join(root, path);
  mkdirSync(dirname(full), { recursive: true });
  writeFileSync(full, typeof data === "string" ? data : JSON.stringify(data));
};
const now = Date.now() / 1000;

write("private/bench-status/current.json", {
  pid: process.pid,
  command: "itest",
  name: "itest-e2e-04",
  started: now - 300,
  finished: null,
  outcome: "running",
  error: null,
  steps: [
    { label: "ota-update", state: "done", started: now - 300, finished: now - 120 },
    { label: "requests", state: "running", started: now - 120 },
    { label: "request-auth", state: "pending" },
  ],
  relay: { 1: true, 2: false },
  lines: [],
});
write(
  "private/bench-status/current.log",
  [
    "- ota-update: install the build under test over the air and see it confirm",
    "  ota-update: pass",
    "- requests: ping, health, clock sync, and a settings change read back and restored",
    "  Waiting for fresh health: health is 80 s old",
    "",
  ].join("\n"),
);
write(
  "private/bench-status/history.jsonl",
  JSON.stringify({ command: "on", name: null, started: now - 400, finished: now - 399, outcome: "pass", error: null }) + "\n",
);

const verdict = (scenario, v, seconds, reason = null) => ({ scenario, verdict: v, reason, seconds });
const run = (name, scenarios, silent = null) =>
  write(`private/verification/${name}-itest.json`, {
    name,
    evidence: "physical",
    scenarios,
    silent,
    captures: [`private/captures/${name}-uart-01`],
  });
run("itest-e2e-01", [verdict("ota-update", "pass", 240), verdict("requests", "pass", 9), verdict("request-auth", "pass", 60)]);
run("itest-e2e-02", [
  verdict("ota-update", "pass", 250),
  verdict("requests", "fail", 21, "No reply to ping within 20 s"),
  verdict("request-auth", "skip", 0, "requests failed"),
]);
run(
  "itest-e2e-03",
  [verdict("ota-update", "pass", 230), verdict("requests", "fail", 180, "no UART record or fresh health for 180 s")],
  "no UART record or fresh health for 180 s",
);
write(
  "private/captures/itest-e2e-03-uart-01/serial-rx.bin",
  [
    "PLFW1 start build=4d37c21a bank=1",
    "PLMQT1 ready ms=21",
    "PLQA1 start stage=optics",
    "PLQA1 check stage=optics status=pass",
    "PLQA1 start stage=power",
    "PLQA1 check stage=power status=pass",
    "PLQA1 start stage=buttons",
    "PLQA1 check stage=buttons status=pass",
    "PLQA1 start stage=indicators",
    "PLQA1 prompt look=PA0 yes=dispense no=lock",
  ].join("\r\n") + "\r\n",
);
write("private/verification/itest-e2e-03-servers.log", "serve: listening on 127.0.0.1:8765\n");

const server = spawn(binary, ["--root", root, "--ui", "--ui-port", port], { stdio: "inherit" });
const stop = () => {
  server.kill("SIGINT");
  rmSync(root, { recursive: true, force: true });
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
server.on("exit", (code) => {
  rmSync(root, { recursive: true, force: true });
  process.exit(code ?? 1);
});
