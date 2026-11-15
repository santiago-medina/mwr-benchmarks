#!/usr/bin/env python3
"""C0 metric: compile Servo's script crates against mozjs-facade and count errors.

Runs `cargo check` in a tree made by `prepare.py --facade`, then writes the
full log, a JSON summary and a short Markdown report to --output. rustc stops
before type checking when name resolution fails, so a falling count can
reveal errors that were hidden before (MOZJS_FACADE.md).
"""
import argparse
import collections
import json
from pathlib import Path
import re
import subprocess

ERROR = re.compile(r"^(?P<file>[^:\s]+):\d+:\d+: error(?:\[(?P<code>E\d+)\])?: (?P<msg>.*)$")


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolchain", default="+1.97.1")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    command = ["cargo", args.toolchain, "check", "--offline", "--keep-going", "--message-format=short",
               "-p", "servo-script-bindings", "-p", "servo-script"]
    run = subprocess.run(command, cwd=args.source, capture_output=True, text=True)
    log = run.stdout + run.stderr
    (args.output / "cargo-check.log").write_text(log)
    errors = [m.groupdict() for line in log.splitlines() if (m := ERROR.match(line))]
    crates = collections.Counter(e["file"].split("/src/")[0] for e in errors)
    codes = collections.Counter(e["code"] or "-" for e in errors)
    messages = collections.Counter(re.sub(r"`[^`]*`", "`_`", e["msg"]) for e in errors)
    summary = {
        "exit": run.returncode,
        "errors": len(errors),
        "by_crate": dict(crates.most_common()),
        "by_code": dict(codes.most_common()),
        "top_messages": messages.most_common(40),
        "aborted": [l for l in log.splitlines() if l.startswith("error: could not compile")],
    }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2))
    lines = [f"C0: {len(errors)} errors (cargo exit {run.returncode})", ""]
    lines += [f"- {k}: {v}" for k, v in summary["by_crate"].items()]
    lines += ["", "Codes: " + ", ".join(f"{k} {v}" for k, v in summary["by_code"].items()), ""]
    lines += [f"- {n} x {m}" for m, n in summary["top_messages"][:15]]
    report = "\n".join(lines)
    (args.output / "summary.md").write_text(report + "\n")
    print(report)


if __name__ == "__main__":
    main()
