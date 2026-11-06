#!/usr/bin/env python3
"""Compare identical max-heapsort implementations; also check standard sort output."""
import json
import pathlib
import statistics
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
out = root / "results"
out.mkdir(exist_ok=True)
programs = {
    "C++ heapsort": root / "build/cpp_heapsort",
    "Rust heapsort": root / "build/rust_heapsort",
    "C++ std::sort": root / "build/cpp_sort",
    "Rust sort_unstable": root / "build/rust_sort",
}
sizes = (0, 1, 2, 3, 17, 120_000, 500_000, 1_000_000)
runs = 9
results = {}
for n in sizes:
    results[str(n)] = {}
    reference = None
    for name, binary in programs.items():
        samples = []
        for i in range(runs + 1 if n >= 120_000 else 1):
            r = subprocess.run([str(binary), str(n)], check=True, capture_output=True, text=True)
            nanoseconds, checksum = map(int, r.stdout.strip().split())
            if reference is None:
                reference = checksum
            if checksum != reference:
                raise RuntimeError(f"checksum mismatch n={n} {name}: {checksum} != {reference}")
            if i > 0:
                samples.append(nanoseconds / 1e6)
        results[str(n)][name] = {"checksum": checksum, "median_ms": statistics.median(samples) if samples else None, "samples_ms": samples}
rows = [
    "# Identical heapsort: C++ vs Rust",
    "",
    "Same LCG input, same max-heap algorithm, comparisons and swaps; sortedness and order-sensitive checksum verified. Nine measured sorting-only runs plus warm-up.",
    "",
    "| Elements | C++ heapsort (ms) | Rust heapsort (ms) | Rust/C++ | C++ std::sort (ms) | Rust sort_unstable (ms) |",
    "|---:|---:|---:|---:|---:|---:|",
]
for n in (120_000, 500_000, 1_000_000):
    x = results[str(n)]
    c = x["C++ heapsort"]["median_ms"]
    r = x["Rust heapsort"]["median_ms"]
    cs = x["C++ std::sort"]["median_ms"]
    rs = x["Rust sort_unstable"]["median_ms"]
    rows.append(f"| {n:,} | {c:.3f} | {r:.3f} | {r/c:.2f}x | {cs:.3f} | {rs:.3f} |")
(out / "identical_heapsort.json").write_text(json.dumps(results, indent=2) + "\n")
(out / "identical_heapsort.md").write_text("\n".join(rows) + "\n")
print("\n".join(rows))
