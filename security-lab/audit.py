#!/usr/bin/env python3
"""Conservative static triage for a narrow Chromium URL code surface.

No finding in this report is a confirmed vulnerability. We fetch source only,
do not run Chrome, and do not attempt exploitation.
"""
import argparse
import datetime
import hashlib
import json
import pathlib
import re
import urllib.request

FILES = {
    "url/url_canon_path.cc": "https://raw.githubusercontent.com/chromium/chromium/main/url/url_canon_path.cc",
    "url/url_canon_unittest.cc": "https://raw.githubusercontent.com/chromium/chromium/main/url/url_canon_unittest.cc",
}
RULES = [
    ("INDEX_OFFSET", "Review indexed access with arithmetic", re.compile(r"\b(?:path_value|spec)\s*\[\s*[A-Za-z_]\w*\s*\+\s*\d+\s*\]"), "medium"),
    ("ASSERTION_ONLY", "Review debug-only guard for untrusted-data preconditions", re.compile(r"\bDCHECK(?:_[A-Z]+)?\s*\("), "low"),
    ("COMPAT_TODO", "Known implementation limitation; investigate callers before any security claim", re.compile(r"TODO\([^)]*\):.*(?:Non-special|non.special|URL)", re.I), "info"),
]

def triage(source: str, path: str):
    findings = []
    for line_number, line in enumerate(source.splitlines(), 1):
        for rule_id, title, pattern, priority in RULES:
            if pattern.search(line):
                findings.append({
                    "rule": rule_id,
                    "priority": priority,
                    "file": path,
                    "line": line_number,
                    "title": title,
                    "snippet": line.strip()[:240],
                    "status": "needs-human-review",
                    "confirmed_vulnerability": False,
                })
    return findings

def download(url: str) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": "MWR-Security-Lab/0.1"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode("utf-8")

def audit(sources: dict[str, str]):
    files = {}
    findings = []
    for path, content in sorted(sources.items()):
        files[path] = {
            "sha256": hashlib.sha256(content.encode()).hexdigest(),
            "lines": len(content.splitlines()),
        }
        # Only source files are scanned; tests are retained for context.
        if path.endswith("_unittest.cc"):
            continue
        findings.extend(triage(content, path))
    return {
        "project": "MWR Security Lab",
        "date_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "method": "source-only deterministic static triage",
        "confirmed_vulnerabilities": 0,
        "disclaimer": "Indicators are review prompts, not security findings or bounty-eligible reports.",
        "files": files,
        "review_candidates": findings,
    }

def markdown(report: dict):
    rows = [
        "# MWR Security Lab — Chromium URL review",
        "",
        "This report contains **review candidates, not confirmed vulnerabilities**.",
        "No Chrome binary or security impact has been tested.",
        "",
        "| Source | Lines | SHA256 |",
        "|---|---:|---|",
    ]
    for path, meta in report["files"].items():
        rows.append(f'| `{path}` | {meta["lines"]} | `{meta["sha256"][:16]}` |')
    rows += ["", f'Potential review locations: **{len(report["review_candidates"])}**', "",
             "| Rule | File:line | Why review |", "|---|---|---|"]
    for f in report["review_candidates"]:
        rows.append(f'| {f["rule"]} | `{f["file"]}:{f["line"]}` | {f["title"]} |')
    rows += [
        "", "## Limitations",
        "- Static regex indicators cannot establish exploitability or even a bug.",
        "- All flagged accesses need caller/invariant analysis and appropriate testing.",
        "- A Chrome VRP report requires reproduction on an official affected Chrome build.",
        "- Do not submit this automated report as a vulnerability.",
        "",
    ]
    return "\n".join(rows)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture-dir", type=pathlib.Path, help="Use local source snapshots for offline verification")
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("security-lab/results"))
    args = parser.parse_args()
    if args.fixture_dir:
        sources = {path: (args.fixture_dir / path).read_text() for path in FILES}
    else:
        sources = {path: download(url) for path, url in FILES.items()}
    report = audit(sources)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    (args.output / "report.md").write_text(markdown(report))
    print(markdown(report))

if __name__ == "__main__":
    main()
