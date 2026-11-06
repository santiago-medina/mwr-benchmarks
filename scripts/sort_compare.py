#!/usr/bin/env python3
import json, pathlib, statistics, subprocess
root=pathlib.Path(__file__).resolve().parents[1]
out=root/"results";out.mkdir(exist_ok=True)
programs={"C++":root/"build/cpp_sort","Rust":root/"build/rust_sort"}
sizes=(120_000,500_000,1_000_000)
runs=9
data={}
for size in sizes:
    data[str(size)]={}
    expected=None
    for language,exe in programs.items():
        samples=[]
        for i in range(runs+1):
            result=subprocess.run([str(exe),str(size)],check=True,text=True,capture_output=True)
            nanoseconds,checksum=map(int,result.stdout.strip().split())
            if expected is None:expected=checksum
            if checksum!=expected:raise RuntimeError(f"checksum differs: n={size}, {language}")
            if i>0:samples.append(nanoseconds/1e6)
        data[str(size)][language]={"median_ms":statistics.median(samples),"samples_ms":samples,"checksum":expected}
lines=["# Sorting-only benchmark","","Same deterministic unsigned 64-bit input; same ascending output; order-sensitive checksum and sortedness verified. Timing excludes generation, verification and process startup.","","| Elements | C++ std::sort (ms) | Rust sort_unstable (ms) | Rust/C++ |","|---:|---:|---:|---:|"]
for size in sizes:
    c=data[str(size)]["C++"]["median_ms"];r=data[str(size)]["Rust"]["median_ms"]
    lines.append(f"| {size:,} | {c:.3f} | {r:.3f} | {r/c:.2f}x |")
(out/"sorting.json").write_text(json.dumps(data,indent=2)+"\n")
(out/"sorting.md").write_text("\n".join(lines)+"\n")
print("\n".join(lines))
