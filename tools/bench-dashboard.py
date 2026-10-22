#!/usr/bin/env python3
"""Serve a read-only local page showing relay-bench progress.

`tools/bench.py` records each command in private/bench-status/ (current.json
and history.jsonl). This server reads those files and the active capture's
received bytes, and serves them on 127.0.0.1 only. It never opens a serial
port, so it cannot disturb an update, a dump or a capture, and it never
writes. Stop it with Ctrl+C. The page shows private UART output: keep it
on this machine.

An optional private/bench-status/script.json gives the operator timed
instructions for a capture: {"title", "capture" (a private/captures/ path),
"anchor" (bytes whose arrival in that capture starts the clock), "steps":
[{"at": seconds, "text"}]}. The page shows the step under way and the next.
"""
import argparse
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import time

ROOT = Path(__file__).resolve().parents[1]
STATUS_DIR = ROOT / "private/bench-status"
CAPTURES = ROOT / "private/captures"
TAIL_BYTES = 12000
TAIL_LINES = 60
HISTORY = 15


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def printable(data):
    text = data.decode("latin-1")
    return "".join(c if c == "\n" or 32 <= ord(c) < 127 else "" for c in text.replace("\r", ""))


def rx_view(directory):
    """Size, modification time and printable tail of a capture's serial-rx.bin."""
    rx = directory / "serial-rx.bin"
    if not rx.is_file():
        return {"path": str(directory.relative_to(ROOT)), "bytes": 0, "modified": None, "tail": []}
    stat = rx.stat()
    with rx.open("rb") as stream:
        stream.seek(max(0, stat.st_size - TAIL_BYTES))
        tail = [line for line in printable(stream.read()).split("\n") if line.strip()][-TAIL_LINES:]
    return {"path": str(directory.relative_to(ROOT)), "bytes": stat.st_size, "modified": stat.st_mtime, "tail": tail}


def capture_view(capture):
    """Elapsed time and bytes so far of an active capture under private/captures/."""
    path = (ROOT / capture["path"]).resolve()
    if not path.is_relative_to(CAPTURES.resolve()):
        return None
    return {**rx_view(path), "seconds": capture["seconds"],
            "elapsed": min(time.time() - capture["started"], capture["seconds"])}


def transfer_view(transfer):
    """Bytes sent and received so far by an update session, against its estimate."""
    path = (ROOT / transfer["path"]).resolve()
    if not path.is_relative_to((ROOT / "private/verification").resolve()):
        return None
    size = {d: (path / f"serial-{d}.bin").stat().st_size if (path / f"serial-{d}.bin").is_file() else 0
            for d in ("tx", "rx")}
    return {"tx": size["tx"], "rx": size["rx"], "tx_expected": transfer["tx"], "rx_expected": transfer["rx"],
            "started": transfer.get("started")}


def anchor_time(directory, anchor):
    """Arrival time (UTC epoch) of the first `anchor` bytes in a capture, if seen."""
    rx = directory / "serial-rx.bin"
    events = directory / "serial-events.jsonl"
    if not rx.is_file() or not events.is_file():
        return None
    offset = rx.read_bytes().find(anchor.encode())
    if offset < 0:
        return None
    for line in events.read_text().splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("direction") == "rx" and event["offset"] <= offset < event["offset"] + event["bytes"]:
            return datetime.fromisoformat(event["utc"]).timestamp()
    return None


def script_view():
    try:
        script = json.loads((STATUS_DIR / "script.json").read_text())
    except (OSError, ValueError):
        return None
    path = (ROOT / script.get("capture", "")).resolve()
    if not path.is_relative_to(CAPTURES.resolve()):
        return None
    return {"title": script.get("title", ""), "capture": script["capture"], "steps": script.get("steps", []),
            "anchor": script.get("anchor", ""), "started": anchor_time(path, script.get("anchor", "")),
            "capture_exists": path.is_dir()}


def latest_rx():
    """The most recently written capture under private/captures/, if any."""
    newest = None
    for rx in CAPTURES.glob("*/serial-rx.bin"):
        mtime = rx.stat().st_mtime
        if newest is None or mtime > newest[0]:
            newest = (mtime, rx.parent)
    return rx_view(newest[1]) if newest else None


def snapshot():
    current = None
    try:
        current = json.loads((STATUS_DIR / "current.json").read_text())
    except (OSError, ValueError):
        pass
    if current:
        current["alive"] = current.get("outcome") == "running" and alive(current.get("pid", -1))
        if current.get("outcome") == "running" and not current["alive"]:
            current["outcome"] = "ended"  # The process exited without recording a finish.
        current["capture"] = capture_view(current["capture"]) if current.get("capture") else None
        current["transfer"] = transfer_view(current["transfer"]) if current.get("transfer") else None
    history = []
    try:
        for line in (STATUS_DIR / "history.jsonl").read_text().splitlines()[-HISTORY:]:
            history.append(json.loads(line))
    except (OSError, ValueError):
        pass
    return {"now": time.time(), "current": current, "history": history[::-1], "rx": latest_rx(),
            "script": script_view()}


PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Feeder Bench</title>
<style>
:root{--bg:#f6f7f9;--card:#fff;--ink:#1b1f24;--muted:#5f6b7a;--line:#e1e5ea;--accent:#2563eb;--ok:#15803d;--bad:#b91c1c;--warn:#b45309;--mono:ui-monospace,SFMono-Regular,Menlo,monospace}
@media (prefers-color-scheme:dark){:root{--bg:#0f1216;--card:#171b21;--ink:#e6e9ee;--muted:#93a0b0;--line:#2a313a;--accent:#60a5fa;--ok:#4ade80;--bad:#f87171;--warn:#fbbf24}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.45 system-ui,sans-serif}
main{max-width:960px;margin:0 auto;padding:20px 16px 40px}h1{font-size:20px;margin:0 0 4px}
.sub{color:var(--muted);font-size:13px;margin-bottom:16px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:16px;margin-bottom:14px}
.row{display:flex;flex-wrap:wrap;gap:10px;align-items:center;justify-content:space-between}
.title{font-weight:600;font-size:16px}.badge{font-size:12px;font-weight:600;padding:2px 9px;border-radius:99px;border:1px solid currentColor}
.running{color:var(--accent)}.pass{color:var(--ok)}.stopped,.failed,.ended{color:var(--bad)}.pending{color:var(--muted)}.done{color:var(--ok)}
.bar{height:10px;background:var(--line);border-radius:99px;overflow:hidden;margin:10px 0 4px}.bar>div{height:100%;background:var(--accent);width:0;transition:width .4s}
.muted{color:var(--muted);font-size:13px}ol{margin:8px 0 0;padding-left:22px}li{margin:3px 0}
.relays{display:flex;gap:8px}.relay{font-size:12px;padding:3px 9px;border-radius:6px;border:1px solid var(--line);color:var(--muted)}
.relay.on{color:var(--warn);border-color:var(--warn)}
.now{font-size:28px;font-weight:700;margin:6px 0 2px}.next{font-size:15px;color:var(--muted)}
.card.op{border-width:2px;border-color:var(--accent)}.steps{margin-top:10px;font-size:13px}.steps div{padding:2px 0;color:var(--muted)}
.steps .cur{color:var(--ink);font-weight:600}.steps .past{text-decoration:line-through}
pre{font:12px/1.4 var(--mono);background:var(--bg);border:1px solid var(--line);border-radius:8px;padding:10px;margin:8px 0 0;max-height:320px;overflow:auto;white-space:pre-wrap;word-break:break-word}
table{width:100%;border-collapse:collapse;font-size:13px}td,th{text-align:left;padding:6px 4px;border-top:1px solid var(--line)}th{color:var(--muted);font-weight:500;border-top:0}
</style></head><body><main>
<h1>Feeder bench</h1><div class="sub">Read-only view of <code>tools/bench.py</code>. Updates every second; it never touches the serial ports.</div>
<div id="script"></div><div id="current"></div><div id="capture"></div><div id="rx"></div><div id="log"></div><div id="history"></div>
</main><script>
const $=id=>document.getElementById(id);
const esc=s=>String(s).replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
const dur=s=>{s=Math.max(0,Math.round(s));const m=Math.floor(s/60);return m?m+"m "+(s%60)+"s":s+"s"};
const when=t=>new Date(t*1000).toLocaleTimeString();
// Finish time by linear extrapolation from a baseline (t0, v0) to now; null until the rate is known.
function eta(now,t0,v0,v,total){if(!t0||v<=v0||now-t0<3)return null;return now+(total-v)*(now-t0)/(v-v0)}
const clock=t=>new Date(t*1000).toLocaleTimeString([], {hour:"numeric",minute:"2-digit"});
const etaText=t=>t?` · ETA ${clock(t)}`:"";
function bar(done,total){const p=total?Math.min(100,100*done/total):0;return `<div class="bar"><div style="width:${p.toFixed(1)}%"></div></div>`}
function renderScript(d){
  const s=d.script;if(!s){$("script").innerHTML="";return}
  const fmt=x=>{x=Math.max(0,Math.ceil(x));return Math.floor(x/60)+":"+String(x%60).padStart(2,"0")};
  let body;
  if(!s.started){body=`<div class="now">Get ready</div><div class="next">${s.capture_exists?"Waiting for the feeder to start the test (LEDs blinking)…":"Waiting for the capture to start…"}</div>`}
  else{const t=d.now-s.started;let i=-1;s.steps.forEach((st,k)=>{if(t>=st.at)i=k});
    const cur=i>=0?s.steps[i]:null,next=s.steps[i+1];
    body=`<div class="muted">Test clock ${fmt(t)}</div><div class="now">${esc(cur?cur.text:"Get ready")}</div>`+
      (next?`<div class="next">Next in ${fmt(next.at-t)}: ${esc(next.text)}</div>`+bar(t-(cur?cur.at:0),next.at-(cur?cur.at:0)):"")+
      `<div class="steps">${s.steps.map((st,k)=>`<div class="${k===i?"cur":k<i?"past":""}">${fmt(st.at)} · ${esc(st.text)}</div>`).join("")}</div>`}
  $("script").innerHTML=`<div class="card op"><div class="title">What to do now${s.title?" · "+esc(s.title):""}</div>${body}</div>`;
}
function render(d){
  renderScript(d);
  const rx=d.rx;
  $("rx").innerHTML=rx?`<div class="card"><div class="title">Latest RX log</div><div class="muted">${esc(rx.path)} · ${rx.bytes} bytes${rx.modified?" · updated "+when(rx.modified):""}</div>
    <pre id="rxpre">${esc(rx.tail.join("\\n"))||"No output yet."}</pre></div>`:"";
  const pre=$("rxpre");if(pre)pre.scrollTop=pre.scrollHeight;
  const c=d.current;
  if(!c){$("current").innerHTML='<div class="card muted">No bench command recorded yet.</div>';$("capture").innerHTML="";$("log").innerHTML="";$("history").innerHTML="";return}
  const end=c.finished||d.now;
  const relay=(n,label)=>`<span class="relay ${c.relay[n]?"on":""}">${label} ${c.relay[n]?"on":"off"}</span>`;
  const total=c.steps.length, done=c.steps.filter(s=>s.state==="done").length;
  let overall=total?`<div class="muted" style="margin-top:10px">Overall: ${done} of ${total} steps</div>`+bar(c.outcome==="pass"?total:done,total):"";
  let xfer="";
  if(c.transfer){const x=c.transfer;const got=x.tx+x.rx, want=x.tx_expected+x.rx_expected;const pct=Math.min(99,100*got/want);
    xfer=`<div class="muted" style="margin-top:10px">Install (estimated): sent ${(x.tx/1024).toFixed(0)} of ~${(x.tx_expected/1024).toFixed(0)} KiB, read back ${(x.rx/1048576).toFixed(2)} of ~${(x.rx_expected/1048576).toFixed(2)} MiB</div>`+bar(pct,100)+`<div class="muted">~${pct.toFixed(0)}%${etaText(eta(d.now,x.started,0,got,want))}</div>`}
  let prog="";
  if(c.progress){const p=c.progress;const val=p.unit==="bytes"?`${(p.done/1048576).toFixed(2)} / ${(p.total/1048576).toFixed(2)} MiB`:`${p.done} / ${p.total} ${p.unit}`;
    prog=bar(p.done,p.total)+`<div class="muted">${val} (${(100*p.done/p.total).toFixed(1)}%)${c.outcome==="running"?etaText(eta(d.now,p.since,p.since_done,p.done,p.total)):""}</div>`}
  $("current").innerHTML=`<div class="card"><div class="row"><div class="title">${esc(c.command)} ${esc(c.name||"")}</div>
    <span class="badge ${esc(c.outcome)}">${esc(c.outcome)}</span></div>
    <div class="muted">Started ${when(c.started)} · ${c.finished?"took":"running for"} ${dur(end-c.started)}</div>
    ${c.error?`<div class="stopped" style="margin-top:6px">${esc(c.error)}</div>`:""}
    <div class="row" style="margin-top:10px"><div class="relays">${relay("1","Power")}${relay("2","Loader strap")}</div></div>
    <ol>${c.steps.map(s=>`<li class="${esc(s.state)}">${esc(s.label)} <span class="muted">· ${esc(s.state)}</span></li>`).join("")}</ol>${overall}${xfer}${prog}</div>`;
  const cap=c.capture;
  $("capture").innerHTML=cap?`<div class="card"><div class="title">Capture</div><div class="muted">${esc(cap.path)} · ${cap.bytes} bytes · ${dur(cap.elapsed)} of ${dur(cap.seconds)} · ends ${clock(d.now+cap.seconds-cap.elapsed)}</div>
    ${bar(cap.elapsed,cap.seconds)}</div>`:"";
  $("log").innerHTML=c.lines.length?`<div class="card"><div class="title">Tool output</div><pre>${esc(c.lines.slice(-15).join("\\n"))}</pre></div>`:"";
  $("history").innerHTML=d.history.length?`<div class="card"><div class="title">Recent commands</div><table><tr><th>Command</th><th>Name</th><th>Started</th><th>Took</th><th>Outcome</th></tr>
    ${d.history.map(h=>`<tr><td>${esc(h.command)}</td><td>${esc(h.name||"")}</td><td>${when(h.started)}</td><td>${dur(h.finished-h.started)}</td><td class="${esc(h.outcome)}">${esc(h.outcome)}${h.error?" · "+esc(h.error):""}</td></tr>`).join("")}</table></div>`:"";
}
async function tick(){try{const r=await fetch("/api/status",{cache:"no-store"});render(await r.json())}catch(e){}setTimeout(tick,1000)}
tick();
</script></body></html>
"""


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/":
            body, kind = PAGE.encode(), "text/html; charset=utf-8"
        elif self.path == "/api/status":
            body, kind = json.dumps(snapshot()).encode(), "application/json"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=int, default=8780)
    args = parser.parse_args(argv)
    server = ThreadingHTTPServer(("0.0.0.0", args.port), Handler)
    print(f"Bench dashboard on http://127.0.0.1:{args.port}/ (Ctrl+C to stop)", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
