"use strict";
// Read-only bench page. Status and log arrive over one event stream; sessions
// and health are polled. Nothing here can reach the relays or the UART.

const $ = id => document.getElementById(id);
const esc = s => String(s ?? "").replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const dur = s => { s = Math.max(0, Math.round(s)); const m = Math.floor(s / 60); return m ? `${m}m ${s % 60}s` : `${s}s`; };
const when = t => new Date(t * 1000).toLocaleTimeString();
const clock = t => new Date(t * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const short = h => h ? String(h).slice(0, 12) + "…" : "?";
const api = path => path;

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

// ---- Now: one summary line (state, elapsed, overall progress), then steps beside the meters.
function meter(label, value, done, total) { return `<div class="meter"><div class="row"><span>${label}</span><span>${value}</span></div>${bar(done, total)}</div>`; }
function renderNow(d) {
  const c = d.current;
  if (!c) { $("now").innerHTML = '<div class="card empty">No bench command recorded yet.</div>'; document.title = "Feeder Bench"; return; }
  const now = nowS(), end = c.finished || now;
  const relay = (n, label) => `<span class="pill ${c.relay[n] ? "on" : ""}">${label} ${c.relay[n] === undefined ? "—" : c.relay[n] ? "on" : "off"}</span>`;
  const total = c.steps.length, done = c.outcome === "pass" ? total : c.steps.filter(s => !["pending", "running"].includes(s.state)).length;
  const meters = [];
  if (c.transfer) {
    const x = c.transfer, got = x.tx + x.rx, want = x.tx_expected + x.rx_expected, pct = Math.min(99, 100 * got / want);
    meters.push(meter(`Install (estimated) · sent ${(x.tx / 1024).toFixed(0)} of ~${(x.tx_expected / 1024).toFixed(0)} KiB, read back ${(x.rx / 1048576).toFixed(2)} of ~${(x.rx_expected / 1048576).toFixed(2)} MiB`,
      `~${pct.toFixed(0)}%${etaText(eta(now, x.started, 0, got, want))}`, pct, 100));
  }
  if (c.progress) {
    const p = c.progress;
    const val = p.unit === "bytes" ? `${(p.done / 1048576).toFixed(2)} / ${(p.total / 1048576).toFixed(2)} MiB` : `${p.done} / ${p.total} ${p.unit}`;
    meters.push(meter(esc(val), `${(100 * p.done / p.total).toFixed(1)}%${c.outcome === "running" ? etaText(eta(now, p.since, p.since_done, p.done, p.total)) : ""}`, p.done, p.total));
  }
  const cap = c.capture;
  if (cap) meters.push(meter(`Capture ${esc(cap.path)} · ${cap.bytes} bytes`, `${dur(cap.elapsed)} of ${dur(cap.seconds)} · ends ${clock(now + cap.seconds - cap.elapsed)}`, cap.elapsed, cap.seconds));
  const steps = c.steps.map((s, i) => `<li class="${esc(s.state)}"><span class="n">${i + 1}</span><span>${esc(s.label)}</span><span class="st">${esc(s.state)}</span></li>`).join("");
  document.title = `${c.outcome} · ${c.command} — Feeder Bench`;
  $("now").innerHTML = `<div class="card"><div class="summary">
      <span class="badge ${stateClass(c.outcome)}">${esc(c.outcome)}</span>
      <span class="title">${esc(c.command)} <span class="muted">${esc(c.name || "")}</span></span>
      <span class="elapsed" title="${c.finished ? "took" : "running for"}">${dur(end - c.started)}</span>
      ${total ? `<span class="mini" title="${done} of ${total} steps">${bar(done, total)}</span><span class="muted">${done}/${total} steps</span>` : ""}
    </div>
    <div class="muted">Started ${when(c.started)}${c.flash_tool ? ` · petlibre-flash <span class="hash">${esc(short(c.flash_tool.sha256))}</span>` : ""}</div>
    ${c.error ? `<div class="stopped error">${esc(c.error)}</div>` : ""}
    <div class="now-body ${meters.length ? "" : "solo"}"><div><ol class="stepper">${steps}</ol><div class="pills">${relay("1", "Power")}${relay("2", "Loader strap")}</div></div>
      ${meters.length ? `<div>${meters.join("")}</div>` : ""}</div>
    ${c.next ? commandBox("Next, after review", c.next) : ""}</div>`;
}

function commandBox(label, text) {
  return `<div class="muted" style="margin-top:10px">${esc(label)}</div><div class="cmd"><pre>${esc(text)}</pre><button type="button" data-copy="${esc(text)}">Copy</button></div>`;
}

function renderRx(d) {
  const rx = d.rx, pre = rx ? `<pre class="rx" data-empty="No output yet.">${esc(rx.tail.join("\n"))}</pre>` : '<pre class="rx" data-empty="No capture yet."></pre>';
  $("rx").innerHTML = `<h2>Latest RX log</h2>${rx ? `<div class="muted">${esc(rx.path)} · ${rx.bytes} bytes${rx.modified ? " · updated " + when(rx.modified) : ""}</div>` : ""}${pre}`;
  const el = $("rx").querySelector("pre"); el.scrollTop = el.scrollHeight;
}

// Long lists show the newest few until "Show all"; the choice survives refreshes.
const SHORT = { sessions: 10, history: 8 }, showAll = { sessions: false, history: false };
function more(list, key, n) {
  const b = $(key + "-more"); b.hidden = n <= SHORT[key];
  b.textContent = showAll[key] ? "Show fewer" : `Show all ${n}`;
  return showAll[key] ? list : list.slice(0, SHORT[key]);
}
const ago = t => { const s = nowS() - t; return s < 60 ? "now" : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`; };
const full = t => new Date(t * 1000).toLocaleString();
// Colour by meaning, since reports spell states many ways ("read complete", "verified_via_supplied_transports").
const stateClass = o => (o = String(o || ""), /^(running|started)/.test(o) ? "running" : /stopped|failed|refused|interrupted|fault/.test(o) ? "stopped"
  : /^ended/.test(o) ? "ended" : /verified|complete|^pass|^done|^ok/.test(o) ? "pass" : /prepared|planned/.test(o) ? "prepared" : "unknown");

function renderHistory(d) {
  $("history-meta").textContent = d.history.length ? `${d.history.length} newest` : "";
  $("history").innerHTML = d.history.length ? `<table><tr><th>Command</th><th class="r">Started</th><th class="r wide">Took</th><th>Outcome</th></tr>
    ${more(d.history, "history", d.history.length).map(h => `<tr><td class="wrap">${esc(h.command)}${h.name ? `<span class="sub-name">${esc(h.name)}</span>` : ""}</td>
      <td class="age" title="${full(h.started)}">${clock(h.started)}</td><td class="took wide">${h.finished ? dur(h.finished - h.started) : "—"}</td>
      <td class="state why ${stateClass(h.outcome)}" title="${esc(h.outcome + (h.error ? " · " + h.error : ""))}">${esc(h.outcome)}${h.error ? " · " + esc(h.error) : ""}</td></tr>`).join("")}</table>`
    : '<div class="empty">No commands recorded yet.</div>';
}

let data = null;
function render() {
  if (!data) return;
  renderScript(data); renderNow(data);
}

// ---- Live log: full scrollback of the current command, filtered locally.
const LOG_LINES = 20000;
let logLines = [];
// The flash client's one-line JSON summaries carry "error":null on success, so
// judge those by their fields rather than by the words in them.
function classifyJson(line) {
  let v; try { v = JSON.parse(line); } catch (e) { return null; }
  if (!v || typeof v !== "object") return null;
  if (v.error) return "l-bad";
  if (/^(stopped|failed|refused|interrupted)/.test(v.status || "")) return "l-bad";
  if (/^(complete|verified|prepared)/.test(v.status || "")) return "l-ok";
  return "";
}
function classify(line) {
  if (/^\+ /.test(line)) return "l-cmd";
  if (line.startsWith("{")) { const c = classifyJson(line); if (c !== null) return c; }
  if (/\b(stopped|refused|fault|failed|error|not confirmed|interrupted)\b/i.test(line)) return "l-bad";
  if (/\bNAK/i.test(line)) return "l-warn";
  if (/\b(complete|completed|verified|confirmed|captured|prepared)\b/i.test(line)) return "l-ok";
  return "";
}
function renderLog() {
  const pre = $("log"), filter = $("filter").value.trim().toLowerCase();
  const shown = filter ? logLines.filter(l => l.toLowerCase().includes(filter)) : logLines;
  pre.innerHTML = shown.map(l => { const c = classify(l); return c ? `<span class="${c}">${esc(l)}</span>` : esc(l); }).join("\n");
  $("log-meta").textContent = logLines.length ? `${logLines.length} lines${filter ? `, ${shown.length} shown` : ""}` : "";
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
let selected = null, sessionList = null;
function renderSessions() {
  const all = sessionList; if (!all) return;
  $("sessions-meta").textContent = all.length ? `${all.length} recent` : "";
  $("sessions").innerHTML = all.length ? `<table><tr><th>Name</th><th class="wide">Evidence</th><th>State</th><th class="r">Age</th></tr>${more(all, "sessions", all.length).map(s =>
    `<tr class="session-row ${s.name === selected ? "sel" : ""}" data-session="${esc(s.name)}"><td class="name wrap">${esc(s.name)}</td>
     <td class="wide"><div class="parts">${s.parts.map(p => `<span class="part">${esc(p)}</span>`).join("")}</div></td>
     <td class="state ${stateClass(s.outcome)}">${esc(s.outcome)}</td><td class="age" title="${s.modified ? full(s.modified) : ""}">${s.modified ? ago(s.modified) : ""}</td></tr>`).join("")}</table>`
    : '<div class="empty">No session evidence under private/ yet.</div>';
}
async function loadSessions() {
  try {
    const r = await fetch(api("/api/sessions"), { cache: "no-store" }); const d = await r.json();
    sessionList = d.sessions; renderSessions();
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
        return `<div style="margin-top:10px"><div class="row"><span><b>${esc(p.kind)}</b> <span class="muted">${esc(p.path)}</span></span><span class="badge ${stateClass(state)}">${esc(state)}</span></div>${kv(facts)}</div>`;
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
for (const key of ["sessions", "history"]) $(key + "-more").addEventListener("click", () => {
  showAll[key] = !showAll[key]; if (key === "sessions") renderSessions(); else if (data) renderHistory(data);
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
