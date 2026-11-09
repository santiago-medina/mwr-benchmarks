# GC scheduling: from stress-by-default to an allocation threshold

Status: design note for roadmap B (performance), 2026-11-09.

## Problem (measured)

Callgrind on `scratch/s1.js` (1000 calls to `assert.sameValue`, release `js_check`):
**31.9 G instructions, about 32 M per call**.

- `Runtime::collect_joint` accounts for 80 % inclusive and `Runtime::reachable` for 68 %.
  The callers are `Engine::new_environment` (8,003 calls) and `Engine::new_object` (2,074 calls),
  through `Engine::allocate`.
- SipHash (`std` `RandomState`) on `dual::Id` keys accounts for about 65 % self time,
  and hashbrown for another 5 %.

Causes in the code:

1. `Engine::new` sets `stress: true` (`src/script/vm.rs`). In stress mode, every `Engine::allocate`
   runs a full stop-the-world joint collection. Every embedding inherits this default:
   `js_check`, `module_check`, `script_demo`, `servo_bridge` and the Servo native path
   (`servo-native/mwr.rs`). It is a test guard that reaches the web-page path.
2. `Engine::allocate` calls `prune_metadata()` after **every** allocation. That retains over
   `objects`, `remote_nodes` and `template_cache`, so each allocation costs O(heap) hashing
   even when nothing was collected.
3. `Runtime::fits()` calls `charged_bytes()`, which sums every record. It runs on every
   `allocate` and `link`, so it is also O(heap), even with no budget set.
4. All `Id`-keyed maps and sets use SipHash. `Id` values are internal allocation counters, never
   attacker-chosen keys, so HashDoS resistance buys nothing here.

## Options

| Option | Cost | Risk |
|---|---|---|
| A. Keep stress and make the GC itself cheaper | GC per allocation stays O(heap), so total work is O(n²) | none, but it does not fix the asymptotics |
| B. **Allocation threshold that grows with the live heap** (collect when `live_objects >= max(MIN, F × live_after_last_gc)`) | amortised O(1) GC work per allocation | see "Correctness" |
| C. Generational / incremental GC | large | needs write barriers, which this tree-walker does not have. Out of scope until the bytecode VM. |

Choice: **B**, with F = 2 and MIN = 4096 objects. This is the classic heap-growth trigger:
Lua's `pause` of 200 % and the trigger in Go's GOGC=100 both mean "collect when the heap
has doubled since the last mark". SpiderMonkey's non-incremental trigger is also
heap-size × growth factor. Literature check: not requested, because this is a standard
scheduling heuristic and not a new collector design (the rearchitecture rule targets the
collector and binding API, which are unchanged here). taskygpt was unavailable (HTTP 429)
at the time of writing.

## Correctness argument

A collection still runs only at an allocation safepoint, exactly where stress mode runs one.
Stress mode collects at **every** safepoint, and the threshold policy collects at a
**subset** of them. Any rooting that is correct when every safepoint collects is therefore
correct when only some of them do. The policy cannot introduce a use-after-collect that
stress mode would not already expose on the same path. The converse does not hold:
unrooted values on untested paths become harder to find. For that reason:

- The `Engine` keeps stress available through `set_gc_policy(GcPolicy::Stress)` and `set_stress(true)`.
- `cargo test` keeps the stress default, because the library default does not change. The
  rooting tests in `tests/` therefore keep exercising every safepoint.
- `js_check --gc-stress` re-enables stress for conformance debugging.
- Embeddings that run real code (`js_check`, `module_check`, `servo_bridge`, `servo-native/mwr.rs`)
  explicitly select `GcPolicy::Threshold`.

The byte budget (`set_budget`) is unchanged. `Runtime::allocate` still collects when an
allocation would exceed the budget, whatever the policy.

## Changes

1. `dual::Runtime` keeps a running `charged` counter. `allocate`, `link`, `unlink`,
   `clear_edges` and `collect_joint` update it. `charged_bytes()` returns it in O(1), and a
   debug assertion compares it with the full sum after each collection. It also counts its
   collections (`collections()`), so the engine can tell whether a budget-triggered
   collection happened inside `Runtime::allocate`.
2. `Engine::allocate` runs `prune_metadata()` only when a collection actually happened.
3. `GcPolicy::{Stress, Threshold}` is selected per engine. Under `Threshold`, `Engine::allocate`
   collects when `live_objects >= next_gc`. After any collection, `next_gc = max(4096, 2 × live)`.
4. The per-microtask `collect()` calls in `run_one_microtask` and `microtask_checkpoint` keep
   their current behaviour under `Stress`. Under `Threshold` they become threshold checks.
   Host-boundary collections (`call_global`, the end of `execute`, `release_node`,
   `collect_native`) stay unconditional, because embedders and tests rely on them to
   release remote nodes deterministically.
5. A small deterministic hasher (`IdHasher`, a multiply-rotate hasher written in safe Rust)
   replaces SipHash for maps and sets keyed by `Id`. There is no new dependency.

## Migration and verification

- Every pass must survive. Gate: `cargo test` (debug and release), `verify.sh` gated profiles,
  then checkpoint 10's full run triaged against `t262-full16`.
- Benchmark set (`bench.py` in the task workspace): `s1.js` plus seven cases that timed out in
  full16 (Array sort stability, decodeURI, encodeURI, a RegExp class escape, unicode identifiers,
  a TypedArray case and a resizable-buffer case). Wall time is measured before and after, and
  callgrind instruction counts are measured on `s1.js`.

## Results

Release `js_check`, same machine, sequential, 2026-11-09. Before = stress default, after = threshold.

| Case | Before (s) | After (s) |
|---|---|---|
| `s1.js` (1000 × `assert.sameValue`) | 3.81 | 0.16 |
| `s1.js` with `--gc-stress` (new code) | 3.81 | 0.58 |
| Array/prototype/sort/stability-513-elements | 9.34 | 0.50 |
| decodeURI/S15.1.3.1_A1.10_T1 | 124.9 | 3.48 |
| encodeURI/S15.1.3.3_A2.1_T1 | 3.47 | 0.29 |
| RegExp/character-class-escape-non-whitespace | 126.4 | 13.6 |
| language/identifiers/start-unicode-10.0.0 | 7.11 | 5.20 |
| TypedArray/prototype/byteLength/return-bytelength | 5.53 | 0.19 |
| Array/prototype/slice/resizable-buffer | 3.11 | 0.10 |

The two cases near 125 s did not finish within the harness's 10 s timeout before. They now
complete, and both report the known "unpaired UTF-16 string storage" Unsupported, so the
result is not a pass.

Callgrind on `s1.js`: **31.9 G → 0.375 G instructions (85× fewer)**. The profile is now spread
across allocation (malloc/free), `Data::edges` and `Data::clone`. That is interpreter cost, the
input for the bytecode-VM work in roadmap B.

Gates: `cargo test` passes in debug and release, still under the stress default. `verify.sh` gated
profiles (default 1998, arrays 1807, promises 170, async 122 passes) show 0 new failures and 0 regressions.
