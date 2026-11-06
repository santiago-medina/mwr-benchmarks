#!/usr/bin/env python3
"""Compare safe Rust heapsort variants against C++ using same input and hashes."""
from pathlib import Path
import subprocess
import statistics
import json

root = Path(__file__).resolve().parents[1]
out = root / "results"
out.mkdir(exist_ok=True)
programs = {
    "C++ heapsort": root / "build/cpp_heapsort",
    "Rust baseline": root / "build/rust_heapsort",
    "Rust safe slice": root / "build/rust_safe_slice",
}
sizes = (0, 1, 2, 3, 17, 120_000, 500_000, 1_000_000)
results = {}

for n in sizes:
    samples = {k: [] for k in programs}
    expected = None
    rounds = 10 if n >= 120_000 else 1
    # Rotate execution order by round to mitigate systematic order effects.
    items = list(programs.items())
    for i in range(rounds):
        for name, exe in items[i % len(items):] + items[:i % len(items)]:
            p = subprocess.run([str(exe), str(n)], capture_output=True, text=True, check=True)
            ns, digest = map(int, p.stdout.strip().split())
            if expected is None:
                expected = digest
            if digest != expected:
                raise RuntimeError(f"Mismatch n={n} {name}: {digest} != {expected}")
            if i:
                samples[name].append(ns / 1e6)
    results[str(n)] = {
        name: {
            "median_ms": statistics.median(samples[name]) if samples[name] else None,
            "samples_ms": samples[name],
            "checksum": expected,
        }
        for name in programs
    }

lines = [
    "# Safe slice-bounded Rust heapsort",
    "",
    "All variants perform the same heapsort comparisons and swaps, using identical seeded 64-bit inputs.",
    "The timing includes only sorting. Correctness is checked by sortedness and an order-sensitive checksum.",
    "One warm-up plus 9 timed runs per size; execution order rotates each round.",
    "",
    "| Elements | C++ (ms) | Rust baseline (ms) | Rust safe slice (ms) | Safe/C++ | Safe/Baseline |",
    "|---:|---:|---:|---:|---:|---:|",
]
for n in (120_000, 500_000, 1_000_000):
    x = results[str(n)]
    c = x["C++ heapsort"]["median_ms"]
    b = x["Rust baseline"]["median_ms"]
    s = x["Rust safe slice"]["median_ms"]
    lines.append(f"| {n:,} | {c:.3f} | {b:.3f} | {s:.3f} | {s/c:.2f}x | {s/b:.2f}x |")

(out / "safe_slice.json").write_text(json.dumps(results, indent=2) + "\n")
(out / "safe_slice.md").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
