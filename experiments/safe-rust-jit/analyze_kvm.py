#!/usr/bin/env python3
"""Paired descriptive statistics for native vs KVM requests (no significance claims)."""
import csv
import json
import math
import statistics
from collections import defaultdict
from pathlib import Path

source = Path("comparison_samples.csv")
if not source.exists():
    print("KVM samples absent: comparison skipped")
    Path("kvm_statistics.json").write_text(json.dumps({"status": "skipped", "reason": "no samples"}, indent=2) + "\n")
    raise SystemExit(0)

groups = defaultdict(dict)
with source.open(newline="") as f:
    for row in csv.DictReader(f):
        key = (int(row["batch"]), int(row["rep"]))
        mode = row["mode"]
        if mode in groups[key]:
            raise SystemExit(f"duplicate observation: {key} {mode}")
        groups[key][mode] = float(row["ns_per_request"])

def quantile(values, p):
    ordered = sorted(values)
    pos = (len(ordered) - 1) * p
    low = math.floor(pos)
    high = math.ceil(pos)
    return ordered[low] * (high - pos) + ordered[high] * (pos - low) if low != high else ordered[low]

result = {"status": "ok", "units": "ns/request", "method": "paired repetitions; warm-up excluded; sample quantiles linear interpolation", "batches": []}
for batch in sorted({k[0] for k in groups}):
    pairs = [v for (b, _), v in sorted(groups.items()) if b == batch]
    if not pairs or any(set(v) != {"native", "kvm_io_exit"} for v in pairs):
        raise SystemExit(f"incomplete native/KVM pair for batch {batch}")
    native = [p["native"] for p in pairs]
    kvm = [p["kvm_io_exit"] for p in pairs]
    ratios = [k / n for k, n in zip(kvm, native)]
    def stats(v):
        return {"n": len(v), "mean": statistics.mean(v), "median": statistics.median(v),
                "stdev": statistics.stdev(v) if len(v) > 1 else None,
                "min": min(v), "p05": quantile(v, .05), "p25": quantile(v, .25),
                "p75": quantile(v, .75), "p95": quantile(v, .95), "max": max(v)}
    entry = {"batch": batch, "native": stats(native), "kvm": stats(kvm),
             "paired_slowdown": stats(ratios),
             "median_delta_ns_per_request": statistics.median([k - n for k, n in zip(kvm, native)])}
    result["batches"].append(entry)
    print(f"batch={batch:>3} native median={entry['native']['median']:.1f} ns "
          f"KVM median={entry['kvm']['median']:.1f} ns "
          f"paired slowdown median={entry['paired_slowdown']['median']:.2f}x "
          f"(p05-p95 {entry['paired_slowdown']['p05']:.2f}-{entry['paired_slowdown']['p95']:.2f}x)")
Path("kvm_statistics.json").write_text(json.dumps(result, indent=2) + "\n")
