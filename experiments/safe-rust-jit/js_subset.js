// Original JS workloads inspired by JetStream/SunSpider categories, NOT official suite scores.
// This measures Node.js (V8), not the experimental KVM guest (which cannot execute JS yet).
"use strict";
const { performance } = require("node:perf_hooks");
const fs = require("node:fs");
const assert = require("node:assert/strict");
const rounds = Number(process.env.JS_ROUNDS || 7);
const samples = Number(process.env.JS_SAMPLES || 100);
if (!Number.isInteger(rounds) || rounds < 3 || rounds > 99 ||
    !Number.isInteger(samples) || samples < 1 || samples > 100000) throw Error("Invalid benchmark settings");
let blackhole = 0;
const tasks = {
  "numeric-bitops": (seed) => { let x = seed | 0; for (let i = 0; i < 10000; i++) x = (Math.imul(x ^ (x >>> 13), 1664525) + 1013904223) | 0; return x; },
  "array-access": (seed) => { const a = Array.from({length: 256}, (_, i) => (i ^ seed) | 0); let x = 0; for (let i = 0; i < 3000; i++) { const j = (i * 37) & 255; x = (x + a[j]) | 0; a[j] = x ^ i; } return x; },
  "object-properties": (seed) => { const o = {a:seed,b:1,c:2,d:3}; for(let i=0;i<3000;i++){o.a=(o.a+o.b)|0;o.b=(o.b^o.c)|0;o.c=(o.c+o.d)|0;o.d=(o.d+1)|0;}return o.a ^ o.b ^ o.c ^ o.d; },
  "string-manipulation": (seed) => { let s = "benchmark" + seed; for(let i=0;i<200;i++) s = (s.slice(1) + s[0]).replace("a","z"); return s.length + s.charCodeAt(0); },
  "json-parse-stringify": (seed) => { const obj={seed,items:Array.from({length:64},(_,i)=>({n:i,value:seed+i}))};let n=0;for(let i=0;i<20;i++){const parsed=JSON.parse(JSON.stringify(obj));n+=parsed.items[(i*3)%64].value;}return n; },
  "regexp": (seed) => { const text=("ab12 cd345 ef6 " + seed + " ").repeat(50);let total=0;for(let i=0;i<30;i++){const m=text.match(/[a-z]+[0-9]+/g);total+=m.length;}return total; }
};
function median(values) { const a=[...values].sort((x,y)=>x-y);return a[Math.floor(a.length/2)]; }
function csvCell(x){return String(x).replaceAll('"','""');}
const rows = ["engine,workload,median_ms_per_operation,rounds,samples,checksum"];
for (const [name, fn] of Object.entries(tasks)) {
  // Check deterministic output on multiple seeds before measuring.
  for (const seed of [1,7,101]) assert.equal(fn(seed),fn(seed),name+" nondeterministic");
  for (let i=0;i<100;i++) blackhole ^= fn(i+1); // warmup
  const measurements=[]; let checksum=0;
  for (let r=0;r<rounds;r++){
    const start=performance.now();
    for(let i=0;i<samples;i++) checksum=(checksum ^ fn((i+1+r*31)|0))|0;
    measurements.push((performance.now()-start)/samples);
  }
  const ms=median(measurements);
  assert.ok(Number.isFinite(ms) && ms>=0);
  rows.push(["node-v8",name,ms.toFixed(6),rounds,samples,checksum>>>0].map(csvCell).join(","));
}
assert.equal(Object.keys(tasks).length,6);
fs.writeFileSync("js_results.csv",rows.join("\n")+"\n");
console.log(rows.join("\n"));
console.log("Node.js "+process.version+"; original representative workloads, NOT official JetStream scores; KVM JS execution unsupported.");
void blackhole;
