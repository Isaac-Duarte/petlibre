// Serve a synthetic bench for the end-to-end tests: the real
// `wellfed-bench serve --ui` on a temporary checkout, with a scripted relay
// board (`--fake-board`), a stand-in wellfed-flash and a stand-in
// `wellfed` for the install job's questions. Nothing touches hardware.
//
//   node e2e/serve.mjs PORT fixtures ROOT   evidence, notes and banks; no job
//   node e2e/serve.mjs PORT writing ROOT    a UART write for the whole run
//   node e2e/serve.mjs PORT idle ROOT       no job; the tests start their own
//
// The evidence views read nothing while the flash client holds the UART,
// so the fixtures are served by a server that runs no job. In `writing`,
// once the server answers, its own client powers the feeder and starts a
// flash-execute whose stand-in reports 3 of 19 sectors and then writes
// until it is stopped. ROOT is the checkout, which playwright.config.ts
// names; in `idle` the tests run their clients in it.
//
// Playwright stops this with SIGINT: the server cancels its job, waits for
// its cleanup and exits, then the checkout is removed. Should the server
// be killed instead, the stand-in exits once its parent is gone.
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, realpathSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const [port = "8797", mode = "writing", given] = process.argv.slice(2);
const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../../../..");
const binary = process.env.BENCH_BIN ?? join(repo, "target/debug/wellfed-bench");

// Cleared first: a run killed outright leaves its checkout behind, and the
// next run on that port removes it.
if (!given) throw new Error("serve.mjs: give the checkout's path");
let root = given;
rmSync(root, { recursive: true, force: true });
mkdirSync(root, { recursive: true });
// Canonical, as the server compares a client's checkout with its own.
root = realpathSync(root);

const write = (path, data) => {
  const full = join(root, path);
  mkdirSync(dirname(full), { recursive: true });
  writeFileSync(full, typeof data === "string" ? data : JSON.stringify(data));
};
const now = Date.now() / 1000;

write("private/bench.json", { relay_port: "relay", uart_port: "uart", baseline: "b", rom_report: "r" });

// Arguments: --root R SUBCOMMAND ACTION ...; anything else exits at once.
// An update of a session named `*-short` verifies its sectors in 3 s and
// ends; any other writes until it is stopped, or its parent is gone.
write(
  "wellfed-flash",
  `#!/bin/sh
parent=$PPID
alive() { kill -0 "$parent" 2>/dev/null; }
case "$3 $4" in
"update execute")
  trap 'exit 1' INT TERM
  echo "Loader answered; writing the reviewed plan" >&2
  echo "Erasing bank 1 sector 0x08006000 and every changed sector after it, in the plan's order, then verifying each one against the target image before the marker is written last" >&2
  echo "Verified changed sectors: 3/19" >&2
  case "$*" in
  *-short*)
    sleep 3
    echo "Verified changed sectors: 19/19" >&2
    exit 0
    ;;
  esac
  while alive; do sleep 1; done
  exit 1
  ;;
esac
`,
);
chmodSync(join(root, "wellfed-flash"), 0o755);

// `wellfed install --prompts lines`, as the install job runs it: two
// questions on stdout, each answer read back and kept in `answers`, and
// the recovery code on stderr, which the page shows beside the question.
write(
  "wellfed",
  `#!/bin/sh
echo "Starting the stock server" >&2
echo '{"kind":"recognize-feeder","device":"AF_E2E","address":"192.168.1.31"}'
read a; echo "$a" >> answers
echo "Recovery code 4711" >&2
echo '{"kind":"open-recovery-window"}'
read a; echo "$a" >> answers
echo "Installed" >&2
`,
);
chmodSync(join(root, "wellfed"), 0o755);

if (mode === "fixtures") writeFixtures();
else if (mode === "writing") write("private/verification/flash-e2e-05-plan.json", "{}");
else {
  // The fixtures as well, so a run's drawer can be open when a write starts.
  writeFixtures();
  write("private/verification/flash-e2e-short-plan.json", "{}");
}

// The server points the user's `wellfed` CLI at itself through
// $XDG_RUNTIME_DIR and ~/.local/state, and refuses to start beside another
// checkout's server there. Each of these has its own, so the three run side
// by side, and beside an operator's real server, without touching theirs.
const home = join(root, "home");
mkdirSync(join(home, "run"), { recursive: true });
const server = spawn(
  binary,
  [
    "--root",
    root,
    "serve",
    "--fake-board",
    "--flash-bin",
    join(root, "wellfed-flash"),
    "--wellfed-bin",
    join(root, "wellfed"),
    "--ui",
    "--ui-port",
    port,
  ],
  { stdio: ["inherit", "pipe", "inherit"], env: { ...process.env, HOME: home, XDG_RUNTIME_DIR: join(home, "run") } },
);

// The page's link carries the token an answer needs, made new at every
// start and printed only on the console: kept as `page-url` for the tests
// that answer.
let printed = "";
server.stdout.on("data", (chunk) => {
  process.stdout.write(chunk);
  printed += chunk.toString();
  const link = /Bench page on (\S+)/.exec(printed)?.[1];
  if (link) {
    write("page-url", link);
    printed = "";
  }
  printed = printed.slice(-4096);
});
const clients = new Set();
let stopping = false;
let failed = false;

// The server cancels its job, waits for its cleanup, then exits; one that
// does not within 12 s is killed.
const stop = () => {
  if (stopping) return;
  stopping = true;
  for (const child of clients) child.kill("SIGKILL");
  server.kill("SIGINT");
  setTimeout(() => server.kill("SIGKILL"), 12_000).unref();
};
const fail = (why) => {
  console.error(`serve.mjs: ${why}`);
  failed = true;
  stop();
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
server.on("exit", (code) => {
  rmSync(root, { recursive: true, force: true });
  process.exit(failed ? 1 : stopping ? 0 : (code ?? 1));
});

/** Start a client command; it is killed if this stops first. */
const start = (words) => {
  const child = spawn(binary, ["--root", root, ...words], { stdio: "ignore" });
  clients.add(child);
  child.on("exit", () => clients.delete(child));
  return child;
};
const finished = (child) => new Promise((done) => child.on("exit", (code) => done(code ?? 1)));

async function main() {
  for (let tries = 0; !existsSync(join(root, "private/bench.sock")); tries++) {
    if (stopping) return;
    if (tries > 200) return fail("the server never bound its socket");
    await sleep(50);
  }
  if (mode !== "writing") return;

  if ((await finished(start(["on"]))) !== 0) return fail("`on` failed");
  if (stopping) return;
  // Left following its job; the server's stop ends both.
  start(["flash-execute", "flash-e2e-05", "--source", "s", "--target", "t", "--expected-plan-sha256", "00"]);
}
await main();

function writeFixtures() {
  write(
    "private/bench-status/history.jsonl",
    JSON.stringify({
      command: "on",
      name: null,
      started: now - 400,
      finished: now - 399,
      outcome: "pass",
      error: null,
    }) + "\n",
  );
  write("private/bench-status/banks.json", {
    banks: {
      0: {
        content: { kind: "stock" },
        source: "seed",
        session: null,
        at: now - 7200,
        check: { state: "disputed", by: "status", at: now - 7100, seen: { kind: "build", id: "77aa00bb11cc" } },
      },
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
      note(now - 60, {
        kind: "timer",
        label: "boot",
        due: now + 840,
        until: now + 960,
        expect: { on: "line", text: "PLFW1 start" },
      }) +
      note(now - 3600, { kind: "timer", label: "soak", due: now - 3000, until: now - 2880, expect: null }) +
      note(now - 120, {
        kind: "timer",
        label: "fire",
        due: now - 90,
        until: now - 30,
        expect: { on: "event", kind: "fed" },
      }) +
      note(now - 40, { kind: "seen", label: "fire", line: "fed event" }) +
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
}
