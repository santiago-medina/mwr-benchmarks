#!/bin/bash
# Usage: run.sh LABEL [binary]  -> appends "label case seconds status" to results.tsv, prints them.
cd "$(dirname "$0")"
G=$(cd ../.. && pwd)
B=${2:-$G/target/release/js_check}
T262=$G/.test262-source
for f in loop_let fib objects closures strings regex_scan; do
  s=$(date +%s.%N); out=$(timeout 120 "$B" $f.js 2>&1 | tail -c 150); e=$(date +%s.%N)
  printf '%s\t%s\t%.2f\t%s\n' "$1" $f $(echo "$e - $s" | bc) "$(echo "$out" | tr '\n\t' '  ' | cut -c1-90)" | tee -a results.tsv
done
P=$T262/test/built-ins/RegExp/property-escapes/generated/Script_-_Manichaean.js
s=$(date +%s.%N); out=$(timeout 120 "$B" $P --include=$T262/harness/assert.js --include=$T262/harness/sta.js --include=$T262/harness/regExpUtils.js 2>&1 | tail -c 150); e=$(date +%s.%N)
printf '%s\t%s\t%.2f\t%s\n' "$1" pe-manichaean $(echo "$e - $s" | bc) "$(echo "$out" | tr '\n\t' '  ' | cut -c1-90)" | tee -a results.tsv
