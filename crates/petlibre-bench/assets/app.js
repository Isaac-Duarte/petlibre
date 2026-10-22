"use strict";
// Read-only bench page. Status and log arrive over one event stream; sessions
// and health are polled. Nothing here can reach the relays or the UART.

const $ = id => document.getElementById(id);
const esc = s => String(s ?? "").replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const dur = s => { s = Math.max(0, Math.round(s)); const m = Math.floor(s / 60); return m ? `${m}m ${s % 60}s` : `${s}s`; };
const when = t => new Date(t * 1000).toLocaleTimeString();
const clock = t => new Date(t * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const short = h => h ? String(h).slice(0, 12) + "…" : "?";
const TOKEN = new URLSearchParams(location.search).get("token");
const api = path => path + (TOKEN ? (path.includes("?") ? "&" : "?") + "token=" + encodeURIComponent(TOKEN) : "");

// Server clock minus ours, so timers agree with the bench host.
let skew = 0;
const nowS = () => Date.now() / 1000 + skew;

// Finish time by linear extrapolation from a baseline (t0, v0) to now; null until the rate is known.
function eta(now, t0, v0, v, total) { if (!t0 || v <= v0 || now - t0 < 3) return null; return now + (total - v) * (now - t0) / (v - v0); }
const etaText = t => t ? ` · ETA ${clock(t)}` : "";
function bar(done, total) { const p = total ? Math.min(100, 100 * done / total) : 0; return `<div class="bar"><div style="width:${p.toFixed(1)}%"></div></div>`; }

// ---- Operator script: sound needs a click first, as browsers require.
let audio = null, speak = false, lastStep = null;
function enableSound() { audio = audio || new (window.AudioContext || window.webkitAudioContext)(); audio.resume(); tone(880, 0.12, 0); render(); }
function tone(freq, len, at) {
  if (!audio) return;
  const o = audio.createOscillator(), g = audio.createGain(), t0 = audio.currentTime + at;
  o.frequency.value = freq; o.connect(g); g.connect(audio.destination);
  g.gain.setValueAtTime(0.25, t0); g.gain.exponentialRampToValueAtTime(0.001, t0 + len); o.start(t0); o.stop(t0 + len + 0.02);
}
function cue(text) {
  if (/^done/i.test(text)) tone(660, 0.9, 0);
  else if (/move the probe/i.test(text)) { tone(520, 0.15, 0); tone(520, 0.15, 0.25); }
  else tone(880, 0.18, 0);
  if (speak && window.speechSynthesis) { speechSynthesis.cancel(); speechSynthesis.speak(new SpeechSynthesisUtterance(text.replace(/[():]/g, " "))); }
}
function soundControls() {
  return `<div class="row" style="margin-top:8px">${audio ? '<span class="muted">Sound on</span>' : '<button type="button" data-act="sound">Enable sound</button>'}
    <label class="muted"><input type="checkbox" data-act="speak" ${speak ? "checked" : ""}> Speak steps</label></div>`;
}
function renderScript(d) {
  const s = d.script; if (!s) { $("script").innerHTML = ""; lastStep = null; return; }
  const fmt = x => { x = Math.max(0, Math.ceil(x)); return Math.floor(x / 60) + ":" + String(x % 60).padStart(2, "0"); };
  let body;
  if (!s.started) {
    body = `<div class="now-big">Get ready</div><div class="next">${s.capture_exists ? "Waiting for the feeder to start the test (LEDs blinking)…" : "Waiting for the capture to start…"}</div>`;
  } else {
    const t = nowS() - s.started; let i = -1; s.steps.forEach((st, k) => { if (t >= st.at) i = k; });
    const cur = i >= 0 ? s.steps[i] : null, next = s.steps[i + 1];
    const key = s.started + ":" + i; if (cur && key !== lastStep) { if (lastStep !== null || t - cur.at < 2) cue(cur.text); lastStep = key; }
    body = `<div class="muted">Test clock ${fmt(t)}</div><div class="now-big">${esc(cur ? cur.text : "Get ready")}</div>` +
      (next ? `<div class="next">Next in ${fmt(next.at - t)}: ${esc(next.text)}</div>` + bar(t - (cur ? cur.at : 0), next.at - (cur ? cur.at : 0)) : "") +
      `<div class="steps">${s.steps.map((st, k) => `<div class="${k === i ? "cur" : k < i ? "past" : ""}">${fmt(st.at)} · ${esc(st.text)}</div>`).join("")}</div>`;
  }
  $("script").innerHTML = `<div class="card op"><div class="title">What to do now${s.title ? " · " + esc(s.title) : ""}</div>${body}${soundControls()}</div>`;
}

// ---- Now: the running (or last) command.
function renderNow(d) {
  const c = d.current;
  if (!c) { $("now").innerHTML = '<div class="card muted">No bench command recorded yet.</div>'; return; }
  const now = nowS(), end = c.finished || now;
  const relay = (n, label) => `<span class="pill ${c.relay[n] ? "on" : ""}">${label} ${c.relay[n] === undefined ? "—" : c.relay[n] ? "on" : "off"}</span>`;
  const total = c.steps.length, done = c.steps.filter(s => s.state === "done").length;
  const overall = total ? `<div class="muted" style="margin-top:10px">Overall: ${done} of ${total} steps</div>` + bar(c.outcome === "pass" ? total : done, total) : "";
  let xfer = "";
  if (c.transfer) {
    const x = c.transfer, got = x.tx + x.rx, want = x.tx_expected + x.rx_expected, pct = Math.min(99, 100 * got / want);
    xfer = `<div class="muted" style="margin-top:10px">Install (estimated): sent ${(x.tx / 1024).toFixed(0)} of ~${(x.tx_expected / 1024).toFixed(0)} KiB, read back ${(x.rx / 1048576).toFixed(2)} of ~${(x.rx_expected / 1048576).toFixed(2)} MiB</div>` +
      bar(pct, 100) + `<div class="muted">~${pct.toFixed(0)}%${etaText(eta(now, x.started, 0, got, want))}</div>`;
  }
  let prog = "";
  if (c.progress) {
    const p = c.progress;
    const val = p.unit === "bytes" ? `${(p.done / 1048576).toFixed(2)} / ${(p.total / 1048576).toFixed(2)} MiB` : `${p.done} / ${p.total} ${p.unit}`;
    prog = bar(p.done, p.total) + `<div class="muted">${val} (${(100 * p.done / p.total).toFixed(1)}%)${c.outcome === "running" ? etaText(eta(now, p.since, p.since_done, p.done, p.total)) : ""}</div>`;
  }
  const cap = c.capture;
  const capture = cap ? `<div class="muted" style="margin-top:10px">Capture ${esc(cap.path)} · ${cap.bytes} bytes · ${dur(cap.elapsed)} of ${dur(cap.seconds)} · ends ${clock(now + cap.seconds - cap.elapsed)}</div>${bar(cap.elapsed, cap.seconds)}` : "";
  const next = c.next ? commandBox("Next, after review", c.next) : "";
  $("now").innerHTML = `<div class="card"><div class="row"><div class="title">${esc(c.command)} ${esc(c.name || "")}</div>
    <span class="badge ${esc(c.outcome)}">${esc(c.outcome)}</span></div>
    <div class="muted">Started ${when(c.started)} · ${c.finished ? "took" : "running for"} ${dur(end - c.started)}${c.flash_tool ? ` · petlibre-flash <span class="hash">${esc(short(c.flash_tool.sha256))}</span>` : ""}</div>
    ${c.error ? `<div class="stopped" style="margin-top:6px">${esc(c.error)}</div>` : ""}
    <div class="pills" style="margin-top:10px">${relay("1", "Power")}${relay("2", "Loader strap")}</div>
    <ol>${c.steps.map(s => `<li class="${esc(s.state)}">${esc(s.label)} <span class="muted">· ${esc(s.state)}</span></li>`).join("")}</ol>
    ${overall}${xfer}${prog}${capture}${next}</div>`;
}

function commandBox(label, text) {
  return `<div class="muted" style="margin-top:10px">${esc(label)}</div><div class="cmd"><pre>${esc(text)}</pre><button type="button" data-copy="${esc(text)}">Copy</button></div>`;
}

function renderRx(d) {
  const rx = d.rx;
  $("rx").innerHTML = rx ? `<div class="card"><h2>Latest RX log</h2><div class="muted">${esc(rx.path)} · ${rx.bytes} bytes${rx.modified ? " · updated " + when(rx.modified) : ""}</div>
    <pre id="rxpre">${esc(rx.tail.join("\n")) || "No output yet."}</pre></div>` : "";
  const pre = $("rxpre"); if (pre) pre.scrollTop = pre.scrollHeight;
}

function renderHistory(d) {
  $("history").innerHTML = d.history.length ? `<div class="card"><h2>Recent commands</h2><table><tr><th>Command</th><th>Name</th><th>Started</th><th>Took</th><th>Outcome</th></tr>
    ${d.history.map(h => `<tr><td>${esc(h.command)}</td><td>${esc(h.name || "")}</td><td>${when(h.started)}</td><td>${h.finished ? dur(h.finished - h.started) : "—"}</td><td class="${esc(h.outcome)}">${esc(h.outcome)}${h.error ? " · " + esc(h.error) : ""}</td></tr>`).join("")}</table></div>` : "";
}

let data = null;
function render() {
  if (!data) return;
  renderScript(data); renderNow(data);
}

// ---- Live log: full scrollback of the current command, filtered locally.
const LOG_LINES = 20000;
let logLines = [];
function classify(line) {
  if (/^\+ /.test(line)) return "l-cmd";
  if (/\b(stopped|refused|fault|failed|error|not confirmed|interrupted)\b/i.test(line)) return "l-bad";
  if (/\bNAK/i.test(line)) return "l-warn";
  if (/\b(complete|completed|verified|confirmed|captured|prepared)\b/i.test(line)) return "l-ok";
  return "";
}
function renderLog() {
  const pre = $("log"), filter = $("filter").value.trim().toLowerCase();
  const shown = filter ? logLines.filter(l => l.toLowerCase().includes(filter)) : logLines;
  pre.innerHTML = shown.map(l => { const c = classify(l); return c ? `<span class="${c}">${esc(l)}</span>` : esc(l); }).join("\n");
  $("log-meta").textContent = logLines.length ? `${logLines.length} lines${filter ? `, ${shown.length} shown` : ""}` : "No output yet.";
  if ($("follow").checked) pre.scrollTop = pre.scrollHeight;
}
$("filter").addEventListener("input", renderLog);
$("follow").addEventListener("change", renderLog);
$("log").addEventListener("scroll", () => {
  const pre = $("log"), atEnd = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 20;
  if (!atEnd && $("follow").checked) $("follow").checked = false;
});
$("copy-log").addEventListener("click", () => copy(logLines.join("\n")));

// ---- Sessions: evidence per session name, with the reviewed next command.
let selected = null;
async function loadSessions() {
  try {
    const r = await fetch(api("/api/sessions"), { cache: "no-store" }); const d = await r.json();
    $("sessions-meta").textContent = d.sessions.length ? `${d.sessions.length} recent` : "";
    $("sessions").innerHTML = d.sessions.length ? `<table><tr><th>Name</th><th>Evidence</th><th>State</th><th>Updated</th></tr>${d.sessions.map(s =>
      `<tr class="session-row ${s.name === selected ? "sel" : ""}" data-session="${esc(s.name)}"><td>${esc(s.name)}</td><td><div class="parts">${s.parts.map(p => `<span class="part">${esc(p)}</span>`).join("")}</div></td>
       <td class="${esc(s.outcome.split(" ").pop())}">${esc(s.outcome)}</td><td class="muted">${s.modified ? when(s.modified) : ""}</td></tr>`).join("")}</table>`
      : '<div class="muted">No session evidence under private/ yet.</div>';
    if (selected) loadSession(selected);
  } catch (e) { }
}
async function loadSession(name) {
  try {
    const r = await fetch(api("/api/sessions/" + encodeURIComponent(name)), { cache: "no-store" });
    if (!r.ok) { $("session").innerHTML = ""; return; }
    const d = await r.json();
    const kv = obj => `<dl class="kv">${Object.entries(obj || {}).map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(typeof v === "object" ? JSON.stringify(v) : v)}</dd>`).join("")}</dl>`;
    $("session").innerHTML = `<div class="detail"><div class="row"><div class="title">${esc(d.name)}</div><button type="button" data-act="close">Close</button></div>
      ${d.parts.map(p => {
        const report = p.report || {}, state = p.plan ? "prepared" : (report.status || "no report");
        const facts = p.plan ? p.plan : { ...(report.error ? { error: report.error } : {}), ...(report.result || {}), ...(p.serial ? { serial_tx: p.serial.tx, serial_rx: p.serial.rx } : {}) };
        return `<div style="margin-top:10px"><div class="row"><span><b>${esc(p.kind)}</b> <span class="muted">${esc(p.path)}</span></span><span class="badge ${esc(state)}">${esc(state)}</span></div>${kv(facts)}</div>`;
      }).join("")}
      ${d.next ? commandBox("Execute after review", d.next) : ""}</div>`;
  } catch (e) { }
}

// ---- Health strip: ports, last relay states, flash client, installed image.
async function loadHealth() {
  try {
    const r = await fetch(api("/api/health"), { cache: "no-store" }); const h = await r.json();
    const pill = (ok, text, title) => `<span class="pill ${ok === true ? "good" : ok === false ? "bad" : ""}" title="${esc(title || "")}">${esc(text)}</span>`;
    const p = h.ports || {}, relays = h.relays || {};
    const relayPill = (n, label) => relays[n] ? `<span class="pill ${relays[n].on ? "on" : ""}" title="last set or read ${when(relays[n].at)}">${label} ${relays[n].on ? "on" : "off"}</span>` : pill(null, `${label} ?`);
    const inst = h.installed;
    $("health").innerHTML = [
      h.config_ok ? pill(p.relay_port, "Relay port") + pill(p.uart_port, "UART port") : pill(false, "No bench.json", p.error),
      relayPill("1", "Power"), relayPill("2", "Strap"),
      h.flash_tool && h.flash_tool.present ? pill(true, "petlibre-flash " + short(h.flash_tool.sha256), h.flash_tool.path + " " + h.flash_tool.sha256) : pill(false, "petlibre-flash missing", h.flash_tool && h.flash_tool.path),
      inst ? pill(null, `Booted bank ${inst.bank ?? "?"} · ${short(inst.build)}${inst.role === "recovery" ? " · recovery" : ""}`, `${inst.capture} at ${when(inst.at)}`) : pill(null, "No replacement banner seen"),
      h.busy ? `<span class="pill on">Running: ${esc(h.busy)}</span>` : "",
    ].join("");
  } catch (e) { }
}

// ---- Clicks: delegated, so re-rendered markup needs no rebinding.
async function copy(text) { try { await navigator.clipboard.writeText(text); } catch (e) { } }
document.addEventListener("click", e => {
  const t = e.target.closest("[data-copy],[data-act],[data-session]"); if (!t) return;
  if (t.dataset.copy !== undefined) { copy(t.dataset.copy); t.textContent = "Copied"; setTimeout(() => t.textContent = "Copy", 1200); }
  else if (t.dataset.act === "sound") enableSound();
  else if (t.dataset.act === "close") { selected = null; $("session").innerHTML = ""; loadSessions(); }
  else if (t.dataset.session) { selected = t.dataset.session; loadSessions(); }
});
document.addEventListener("change", e => { if (e.target.dataset && e.target.dataset.act === "speak") speak = e.target.checked; });

// ---- Event stream; the browser reconnects on its own after a drop.
let lastOutcome = null;
const events = new EventSource(api("/api/events"));
events.addEventListener("status", e => {
  data = JSON.parse(e.data); skew = data.now - Date.now() / 1000;
  render(); renderRx(data); renderHistory(data);
  const outcome = data.current ? data.current.started + ":" + data.current.outcome : null;
  if (outcome !== lastOutcome) { lastOutcome = outcome; loadSessions(); loadHealth(); }
});
events.addEventListener("log-reset", () => { logLines = []; renderLog(); });
events.addEventListener("log", e => {
  const lines = e.data.split("\n"); if (lines[lines.length - 1] === "") lines.pop();
  logLines.push(...lines); if (logLines.length > LOG_LINES) logLines = logLines.slice(-LOG_LINES);
  renderLog();
});

// Timers and ETAs move between server updates; faster while a script runs.
(function tick() { render(); setTimeout(tick, data && data.script && data.script.started ? 250 : 1000); })();
setInterval(loadHealth, 5000);
setInterval(loadSessions, 10000);
loadHealth(); loadSessions();
