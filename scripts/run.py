#!/usr/bin/env python3
import json, pathlib, statistics, subprocess, time
root=pathlib.Path(__file__).resolve().parents[1]
out=root/"results";out.mkdir(exist_ok=True)
tests=["integers","primes","sorting"]
programs={"C++":root/"build/cpp_bench","Rust":root/"build/rust_bench"}
data={}
for test in tests:
    data[test]={}
    expected=None
    for language,exe in programs.items():
        samples=[]
        for i in range(10):
            start=time.perf_counter_ns()
            result=subprocess.run([str(exe),test],capture_output=True,text=True,check=True)
            elapsed=(time.perf_counter_ns()-start)/1e6
            checksum=result.stdout.strip()
            if expected is None:expected=checksum
            if checksum!=expected:raise RuntimeError(f"Mismatch {test}: {language} {checksum} vs {expected}")
            if i:samples.append(elapsed)  # discard first run
        data[test][language]={"median_ms":statistics.median(samples),"samples_ms":samples,"checksum":checksum}
lines=["# C++ vs Rust benchmarks","","9 measured runs per language (plus 1 warm-up), optimized builds, one GitHub-hosted runner.","","| Algorithm | C++ median (ms) | Rust median (ms) | Rust / C++ |","|---|---:|---:|---:|"]
for t in tests:
    c=data[t]["C++"]["median_ms"];r=data[t]["Rust"]["median_ms"]
    lines.append(f"| {t} | {c:.3f} | {r:.3f} | {r/c:.2f}x |")
(out/"results.json").write_text(json.dumps(data,indent=2)+"\n")
(out/"summary.md").write_text("\n".join(lines)+"\n")
print("\n".join(lines))
