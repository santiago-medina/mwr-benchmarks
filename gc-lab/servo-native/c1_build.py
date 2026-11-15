#!/usr/bin/env python3
"""C1 metric: build servoshell against mozjs-facade, then probe its startup.

Runs `cargo build -p servoshell` in a tree made by `prepare.py --facade` and
records compile errors, link errors, build time and binary size. If the binary
links, it loads a page with no script in headless mode. Facade stubs panic with
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


def probe(args, binary):
    with tempfile.TemporaryDirectory(prefix="c1-probe-") as temp:
        work = Path(temp)
        env = dict(os.environ, RUST_BACKTRACE="1", LIBGL_ALWAYS_SOFTWARE="1", EGL_PLATFORM="surfaceless")
        for key, sub in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CACHE_HOME", "cache"),
                         ("XDG_CONFIG_HOME", "config"), ("TMPDIR", "tmp")]:
            (work / sub).mkdir(mode=0o700)
            env[key] = str(work / sub)
        page = work / "index.html"
        page.write_text(PAGE)
        image = work / "out.png"
        command = [str(binary), "--headless", "--window-size=400x300", "--temporary-storage",
                   "-x", "-o", str(image), page.as_uri()]
        try:
            run = subprocess.run(command, env=env, capture_output=True, timeout=args.timeout)
            code, log = run.returncode, run.stdout + run.stderr
        except subprocess.TimeoutExpired as e:
            code, log = "timeout", (e.stdout or b"") + (e.stderr or b"")
        log = log.decode(errors="replace")
        (args.output / "run.log").write_text(log)
        stubs = list(dict.fromkeys(STUB.findall(log)))
        return dict(exit=code, stubs_hit=stubs, rendered=image.exists() and image.stat().st_size > 1000,
                    tail=log.splitlines()[-40:])


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolchain", default="+1.97.1")
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    summary = {"build": None, "run": None}
    summary["build"], binary = build(args)
    if summary["build"]["exit"] == 0:
        summary["run"] = probe(args, binary)
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2))
    b, r = summary["build"], summary["run"]
    lines = [f"C1 build: exit {b['exit']}, {b['errors']} compile errors, {b['link_errors']} link symbols, "
             f"{b['seconds']} s, binary {b['binary_bytes']} bytes", ""]
    lines += [f"- {k}: {v}" for k, v in b["by_crate"].items()]
    lines += [f"- {n} x {m}" for m, n in b["top_messages"][:10]]
    lines += [f"- link: {s}" for s in b["link_symbols"][:20]]
    lines += [f"- {a}" for a in b["aborted"]]
    if r:
        lines += ["", f"C1 run: exit {r['exit']}, rendered {r['rendered']}, stubs hit: {', '.join(r['stubs_hit']) or '-'}",
                  "", "```", *r["tail"], "```"]
    report = "\n".join(lines)
    (args.output / "summary.md").write_text(report + "\n")
    print(report)


if __name__ == "__main__":
    main()
