#!/usr/bin/env python3
"""Differential audit for direct eval introducing var shadowing during assignment RHS.

This is a diagnostic, not a pass-only conformance gate. Runs original Test262
cases in sloppy Tree/VM with threshold and stress GC and records status/reason.
"""
import json
import pathlib
import subprocess
import sys

binary = pathlib.Path(sys.argv[1]).resolve()
test262 = pathlib.Path(sys.argv[2]).resolve()
cases = [
    *(f"language/expressions/compound-assignment/S11.13.2_A6.{n}_T1.js" for n in range(1,12)),
    *(f"language/expressions/assignment/S11.13.1_A6_T{n}.js" for n in range(1,4)),
]
results = []
for name in cases:
    file = test262 / "test" / name
    if not file.is_file():
        raise SystemExit(f"missing pinned Test262 case: {file}")
    for engine in ("tree", "vm"):
        for gc_stress in (False, True):
            cmd = [str(binary), str(file), "--sloppy", f"--engine={engine}"]
            if gc_stress:
                cmd.append("--gc-stress")
            try:
                p = subprocess.run(cmd, capture_output=True, text=True, timeout=45)
                report = json.loads(p.stdout)
                status = report.get("status", "malformed")
                reason = report.get("reason", "")
            except Exception as exc:
                status = "runner-error"
                reason = repr(exc)
                p = None
            results.append(dict(path=name, engine=engine, gc_stress=gc_stress,
                                status=status, reason=reason[:400],
                                code=p.returncode if p else None))
with open("binding-eval-results.json", "w") as out:
    json.dump(results, out, indent=2)
counts = {}
for r in results:
    key = (r["engine"], r["gc_stress"], r["status"])
    counts[key] = counts.get(key, 0) + 1
print(json.dumps({"total": len(results), "counts": [
    dict(engine=e, gc_stress=g, status=s, count=n)
    for (e,g,s),n in sorted(counts.items())]}, indent=2))
assert len(results) == len(cases)*4
assert not any(r["status"] in ("runner-error","malformed") for r in results)
