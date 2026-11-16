#!/usr/bin/env python3
"""C1 metric: build servoshell against mozjs-facade, then probe its startup.

Runs `cargo build -p servoshell` in a tree made by `prepare.py --facade` and
records compile errors, link errors, build time and binary size. If the binary
links, it loads a page with no script in headless mode (the startup probe), then
a page whose ordinary <script> runs a fixed list of steps (the script probe).
Each step logs "C1-STEP <name> ok|fail: <message>" through console.log and
console.error, so the steps that ran are counted from the log. Facade stubs panic with
"mozjs facade: <name> is not implemented yet", so the first such name is the
next entry point to implement (MOZJS_FACADE.md, C1). Writes build.log,
run.log, summary.json and summary.md to --output.
"""
import argparse
import collections
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time

ERROR = re.compile(r"^(?P<file>[^:\s]+):\d+:\d+: error(?:\[(?P<code>E\d+)\])?: (?P<msg>.*)$")
LINK = re.compile(r"undefined reference to `([^']+)'|undefined symbol: (\S+)")
STUB = re.compile(r"mozjs facade: (\S+) is not implemented yet")
PAGE = "<!doctype html><meta charset=utf-8><title>c1</title><p>facade startup probe</p>\n"
STEP = re.compile(r"C1-STEP (\S+) (ok|fail)(?:: (.*))?")
# Each step is one string so that console sees a single string argument and the
# marker needs no value-to-source conversion. Later steps run from microtasks and
# timers, so a step that never logs means its job or task never ran.
SCRIPT_PAGE = """<!doctype html><meta charset=utf-8><title>c1 script</title><p id=out>script probe</p>
<script>
function step(name, body) {
  var line;
  try { var r = body(); line = 'C1-STEP ' + name + (r === true ? ' ok' : ' fail: returned ' + String(r)); }
  catch (e) { line = 'C1-STEP ' + name + ' fail: ' + String(e && e.message); }
  console.log(line); console.error(line);
}
step('language', function () { var a = [3, 1, 2].sort(); return a.join() === '1,2,3' && ({x: 1}).x === 1; });
step('closure', function () { var f = (function (n) { return function () { return ++n; }; })(1); f(); return f() === 3; });
step('dom-read', function () { return document.getElementById('out').textContent === 'script probe'; });
step('dom-write', function () {
  var p = document.createElement('p'); p.textContent = 'written'; document.body.appendChild(p);
  document.title = 'c1 done'; return document.body.lastChild === p && document.title === 'c1 done';
});
step('json', function () { return JSON.stringify(JSON.parse('{"a":[1,2]}')) === '{"a":[1,2]}'; });
queueMicrotask(function () { step('microtask', function () { return true; }); });
Promise.resolve(7).then(function (v) { step('promise-then', function () { return v === 7; }); });
setTimeout(function () { step('timeout', function () { return true; }); }, 0);
</script>
"""
STEPS = ["language", "closure", "dom-read", "dom-write", "json", "microtask", "promise-then", "timeout"]


def build(args):
    command = ["cargo", args.toolchain, "build", "--offline", "--message-format=short",
               "-p", "servoshell", "--no-default-features", "--features", "bundled"]
    start = time.monotonic()
    run = subprocess.run(command, cwd=args.source, capture_output=True, text=True)
    seconds = round(time.monotonic() - start)
    log = run.stdout + run.stderr
    (args.output / "build.log").write_text(log)
    errors = [m.groupdict() for line in log.splitlines() if (m := ERROR.match(line))]
    links = sorted({a or b for a, b in LINK.findall(log)})
    binary = args.source / "target/debug/servoshell"
    return dict(
        exit=run.returncode, seconds=seconds, errors=len(errors),
        by_crate=dict(collections.Counter(e["file"].split("/src/")[0] for e in errors).most_common()),
        top_messages=collections.Counter(re.sub(r"`[^`]*`", "`_`", e["msg"]) for e in errors).most_common(20),
        link_errors=len(links), link_symbols=links[:60],
        aborted=[l for l in log.splitlines() if l.startswith("error: could not compile")],
        binary_bytes=binary.stat().st_size if run.returncode == 0 and binary.exists() else None,
    ), binary


def probe(args, binary, html, name):
    with tempfile.TemporaryDirectory(prefix="c1-probe-") as temp:
        work = Path(temp)
        env = dict(os.environ, RUST_BACKTRACE="1", LIBGL_ALWAYS_SOFTWARE="1", EGL_PLATFORM="surfaceless")
        for key, sub in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CACHE_HOME", "cache"),
                         ("XDG_CONFIG_HOME", "config"), ("TMPDIR", "tmp")]:
            (work / sub).mkdir(mode=0o700)
            env[key] = str(work / sub)
        page = work / "index.html"
        page.write_text(html)
        image = work / "out.png"
        command = [str(binary), "--headless", "--window-size=400x300", "--temporary-storage",
                   "-x", "-o", str(image), page.as_uri()]
        try:
            run = subprocess.run(command, env=env, capture_output=True, timeout=args.timeout)
            code, log = run.returncode, run.stdout + run.stderr
        except subprocess.TimeoutExpired as e:
            code, log = "timeout", (e.stdout or b"") + (e.stderr or b"")
        log = log.decode(errors="replace")
        (args.output / f"{name}.log").write_text(log)
        stubs = list(dict.fromkeys(STUB.findall(log)))
        steps = {}
        for step, verdict, message in STEP.findall(log):
            steps.setdefault(step, verdict if verdict == "ok" else f"fail: {message}")
        return dict(exit=code, stubs_hit=stubs, rendered=image.exists() and image.stat().st_size > 1000,
                    steps={s: steps.get(s, "missing") for s in STEPS} if html is SCRIPT_PAGE else None,
                    tail=log.splitlines()[-40:])


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolchain", default="+1.97.1")
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    summary = {"build": None, "run": None, "script": None}
    summary["build"], binary = build(args)
    if summary["build"]["exit"] == 0:
        summary["run"] = probe(args, binary, PAGE, "run")
        summary["script"] = probe(args, binary, SCRIPT_PAGE, "script")
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2))
    b, r, sc = summary["build"], summary["run"], summary["script"]
    lines = [f"C1 build: exit {b['exit']}, {b['errors']} compile errors, {b['link_errors']} link symbols, "
             f"{b['seconds']} s, binary {b['binary_bytes']} bytes", ""]
    lines += [f"- {k}: {v}" for k, v in b["by_crate"].items()]
    lines += [f"- {n} x {m}" for m, n in b["top_messages"][:10]]
    lines += [f"- link: {s}" for s in b["link_symbols"][:20]]
    lines += [f"- {a}" for a in b["aborted"]]
    if r:
        lines += ["", f"C1 run: exit {r['exit']}, rendered {r['rendered']}, stubs hit: {', '.join(r['stubs_hit']) or '-'}",
                  "", "```", *r["tail"], "```"]
    if sc:
        ok = sum(v == "ok" for v in sc["steps"].values())
        lines += ["", f"C1 script: exit {sc['exit']}, steps ok {ok}/{len(STEPS)}, "
                  f"stubs hit: {', '.join(sc['stubs_hit']) or '-'}", ""]
        lines += [f"- {k}: {v}" for k, v in sc["steps"].items()]
        lines += ["", "```", *sc["tail"][-20:], "```"]
    report = "\n".join(lines)
    (args.output / "summary.md").write_text(report + "\n")
    print(report)


if __name__ == "__main__":
    main()
