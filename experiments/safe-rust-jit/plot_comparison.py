#!/usr/bin/env python3
"""Generate self-contained SVG benchmark charts from comparison_results.csv."""
import csv
import html
from pathlib import Path

HERE = Path(__file__).resolve().parent
rows = list(csv.DictReader((HERE / "comparison_results.csv").open(encoding="utf-8")))
modes = ["native", "process_roundtrip", "kvm_io_exit"]
names = {"native": "Native (same process)", "process_roundtrip": "Separate process (IPC)", "kvm_io_exit": "KVM"}
colors = {"native": "#16a34a", "process_roundtrip": "#2563eb", "kvm_io_exit": "#d97706"}
data = {(r["mode"], int(r["batch"])): float(r["median_ns_per_request"]) for r in rows if r.get("mode") in modes}
batches = sorted({batch for mode, batch in data})
if not batches or any((mode, batch) not in data for mode in modes for batch in batches):
    raise SystemExit("Missing comparison results; cannot generate graphs")

def graph(log_scale):
    width, height = 1000, 560
    left, right, top, bottom = 100, 35, 95, 105
    plotw, ploth = width - left - right, height - top - bottom
    maximum = max(data.values()) * 1.15
    minimum = min(data.values()) / 1.5 if log_scale else 0
    import math
    def y(value):
        if log_scale:
            return top + ploth * (math.log(maximum) - math.log(value)) / (math.log(maximum) - math.log(minimum))
        return top + ploth * (1 - value / maximum)
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
             '<rect width="100%" height="100%" fill="white"/>',
             f'<text x="{left}" y="35" font-size="23" font-family="sans-serif" font-weight="bold">Native vs IPC vs KVM — {"logarithmic" if log_scale else "linear"} scale</text>',
             f'<text x="{left}" y="60" font-size="14" font-family="sans-serif" fill="#475569">Median ns per request · lower is better · native optimization caveat</text>']
    for i in range(5):
        value = math.exp(math.log(minimum) + (math.log(maximum) - math.log(minimum))*i/4) if log_scale else maximum*i/4
        yy = y(value)
        parts += [f'<line x1="{left}" x2="{width-right}" y1="{yy:.2f}" y2="{yy:.2f}" stroke="#e2e8f0"/>',
                  f'<text x="{left-10}" y="{yy+4:.2f}" text-anchor="end" font-family="sans-serif" font-size="12" fill="#64748b">{value:,.2f}</text>']
    group_width = plotw / len(batches)
    bar_width = min(55, group_width / 5)
    for i, batch in enumerate(batches):
        center = left + group_width * (i + 0.5)
        parts.append(f'<text x="{center:.2f}" y="{height-bottom+27}" text-anchor="middle" font-family="sans-serif" font-size="15">{batch}</text>')
        for j, mode in enumerate(modes):
            value = data[(mode, batch)]
            x = center + (j-1)*bar_width*1.25 - bar_width/2
            yy = y(value)
            h = max(1, top+ploth-yy)
            parts.append(f'<rect x="{x:.2f}" y="{yy:.2f}" width="{bar_width:.2f}" height="{h:.2f}" fill="{colors[mode]}"/>')
            # Exact value labels keep native readable on the linear graph.
            label_y = max(top + 12, yy - 6) if h > 20 else top + ploth - 8 - j*15
            parts.append(f'<text x="{x+bar_width/2:.2f}" y="{label_y:.2f}" text-anchor="middle" font-family="sans-serif" font-size="11" fill="#334155">{value:,.2f}</text>')
    parts.append(f'<text x="{left+plotw/2}" y="{height-46}" text-anchor="middle" font-family="sans-serif" font-size="14">Batch size</text>')
    for j, mode in enumerate(modes):
        x = left + j*285
        parts += [f'<rect x="{x}" y="{height-25}" width="13" height="13" fill="{colors[mode]}"/>',
                  f'<text x="{x+20}" y="{height-13}" font-family="sans-serif" font-size="13">{html.escape(names[mode])}</text>']
    parts.append("</svg>")
    filename = "comparison_log.svg" if log_scale else "comparison_linear.svg"
    (HERE / filename).write_text("\n".join(parts), encoding="utf-8")
    print(f"Generated {filename}")

graph(False)
graph(True)
