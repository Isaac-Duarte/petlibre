// Serve a synthetic bench for the end-to-end tests: the real
// `wellfed-bench serve --ui` on a temporary checkout, with a scripted relay
// board (`--fake-board`) and a stand-in wellfed-flash. Once the server
// answers, its own client powers the feeder and starts a flash-execute,
// whose stand-in reports 3 of 19 sectors and then writes until the server
// stops it, so the page shows a UART write for the whole run. Nothing
// touches hardware.
import { spawn } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const port = process.argv[2] ?? "8797";
const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../../../..");
const binary = process.env.BENCH_BIN ?? join(repo, "target/debug/wellfed-bench");

// Canonical, as the server compares a client's checkout with its own.
const root = realpathSync(mkdtempSync(join(tmpdir(), "wellfed-e2e-")));
const write = (path, data) => {
  const full = join(root, path);
  mkdirSync(dirname(full), { recursive: true });
  writeFileSync(full, typeof data === "string" ? data : JSON.stringify(data));
};
const now = Date.now() / 1000;

write("private/bench.json", { relay_port: "relay", uart_port: "uart", baseline: "b", rom_report: "r" });

// Arguments: --root R SUBCOMMAND ACTION ...; only an update is ever run.
write(
  "wellfed-flash",
  `#!/bin/sh
case "$3 $4" in
"update execute")
  trap 'exit 1' INT TERM
  echo "Loader answered; writing the reviewed plan" >&2
  echo "Erasing bank 1 sector 0x08006000 and every changed sector after it, in the plan's order, then verifying each one against the target image before the marker is written last" >&2
  echo "Verified changed sectors: 3/19" >&2
  while :; do sleep 1; done
  ;;
esac
`,
);
chmodSync(join(root, "wellfed-flash"), 0o755);
write("private/verification/flash-e2e-05-plan.json", "{}");

write(
  "private/bench-status/history.jsonl",
  JSON.stringify({ command: "on", name: null, started: now - 400, finished: now - 399, outcome: "pass", error: null }) +
    "\n",
);
write("private/bench-status/banks.json", {
  banks: {
    0: { content: { kind: "stock" }, source: "seed", session: null, at: now - 7200, check: { state: "unchecked" } },
    1: {
      content: { kind: "build", id: "4d37c21a00ff" },
      source: "flash-execute",
      session: "flash-e2e-04",
      at: now - 3600,
      check: { state: "confirmed", by: "banner", at: now - 3500 },
    },
  },
});
const note = (at, body) => JSON.stringify({ at, body }) + "\n";
write(
  "private/bench-status/notes.jsonl",
  note(now - 600, { kind: "step", text: "Install the candidate over UART" }) +
    note(now - 600, { kind: "next", steps: ["Boot and watch the banner", "Run the smoke tier"] }) +
    note(now - 60, { kind: "timer", label: "boot", seconds: 900, expect: "PLFW1 start" }) +
    note(now - 500, { kind: "evidence", card: "QA9", verdict: "pass", text: "banner seen", path: null }) +
    note(now - 400, {
      kind: "evidence",
      card: "QA9",
      verdict: "fail",
      text: "no status after boot",
      path: "private/captures/itest-e2e-03-uart-01",
    }),
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
run("itest-e2e-01", [
  verdict("ota-update", "pass", 240),
  verdict("requests", "pass", 9),
  verdict("request-auth", "pass", 60),
]);
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

// A flash session's evidence, so the sessions view has a row and a detail.
write("private/captures/flash-e2e-01-loader/serial-rx.bin", "loader ready\r\n");
// An hour old, so the integration run's capture stays the latest one.
const hourAgo = now - 3600;
utimesSync(join(root, "private/captures/flash-e2e-01-loader/serial-rx.bin"), hourAgo, hourAgo);
utimesSync(join(root, "private/captures/flash-e2e-01-loader"), hourAgo, hourAgo);
write("private/verification/flash-e2e-01-inspect/report.json", {
  status: "complete",
  error: null,
  result: { bytes: 4096, partition_table_sha256: "0f".repeat(32) },
});
write("private/verification/flash-e2e-01-plan.json", {
  operation_plan: { bank: 1, erase_sectors: 2, ordered_sectors: [{}, {}] },
});

const server = spawn(
  binary,
  ["--root", root, "serve", "--fake-board", "--flash-bin", join(root, "wellfed-flash"), "--ui", "--ui-port", port],
  { stdio: "inherit" },
);
let stopping = false;
const clean = (code) => {
  rmSync(root, { recursive: true, force: true });
  process.exit(code);
};
// The server cancels its job, waits for its cleanup, then exits.
const stop = () => {
  stopping = true;
  server.kill("SIGINT");
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
server.on("exit", (code) => {
  clean(stopping ? 0 : (code ?? 1));
});

/** Run a client command to its end; its code. */
const client = (words) =>
  new Promise((done) => {
    spawn(binary, ["--root", root, ...words], { stdio: "inherit" }).on("exit", (code) => {
      done(code ?? 1);
    });
  });

for (let tries = 0; !existsSync(join(root, "private/bench.sock")); tries++) {
  if (tries > 200) {
    console.error("serve.mjs: the server never bound its socket");
    stop();
  }
  await sleep(50);
}
if ((await client(["on"])) !== 0) {
  console.error("serve.mjs: `on` failed");
  stop();
}
// Left following its job; the server's stop ends both.
spawn(
  binary,
  ["--root", root, "flash-execute", "flash-e2e-05", "--source", "s", "--target", "t", "--expected-plan-sha256", "00"],
  { stdio: "ignore" },
);
