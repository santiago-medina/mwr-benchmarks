# Object layout before shapes (roadmap B, stage 5)

Status: design note, 2026-11-15, HEAD eb06a7f (checkpoint 51). No code yet. It follows
OBJECT_MODEL.md (stage 4, slices 1-4 done) and replaces the order of its "Slice 5: shapes and
property inline caches": the measurements below show that slice 5 is not the next cost.

## Problem (measured)

### Benchmark set B0 (fixed from now on)

`vm-bench/` in the task workspace (not in this repo), release `js_check` with line tables, `MWR_ENGINE=vm`, callgrind. The stage script
is `scratch/prof_b0.sh` in the task workspace (it calls `scratch/prof.sh`, per benchmark).
`scratch/b0_model.py` turns the callgrind output into the table below.

- test262-shaped: `decode_tiny`, `copywithin_small` (the stage-4 set);
- interpreter: `fib_small`, `loop_small`, `closures`, `global_intrinsics`, `strings`;
- objects (new in B0, Servo-shaped): `objects` (literals plus named get/set), `props_class`
  (class instances, prototype methods, a getter), `props_poly` (one site over 4 shapes and one
  over 12 shapes), `tree_walk` (a DOM-like node tree, recursive walk with a property filter).

### Self cost by mechanism (% of instructions; first matching bucket wins)

| bench | Ir (M) | allocator | memcpy | object store | Value clone/drop | property path | names | dispatch | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| decode_tiny | 224.1 | 10.9 | 4.6 | 2.6 | 11.2 | 4.9 | 12.5 | 24.1 | 27.7 |
| copywithin_small | 2519.0 | 13.2 | 5.5 | 11.2 | 3.9 | 33.5 | 0.0 | 5.4 | 26.0 |
| fib_small | 39.0 | 18.1 | 6.2 | 8.0 | 9.1 | 15.9 | 2.0 | 10.4 | 28.2 |
| loop_small | 366.4 | 22.7 | 9.0 | 8.1 | 5.7 | 12.0 | 7.1 | 14.9 | 19.3 |
| closures | 1683.8 | 20.6 | 7.0 | 9.5 | 5.2 | 13.6 | 6.5 | 8.2 | 26.7 |
| objects | 1361.9 | 14.0 | 6.2 | 9.9 | 6.9 | 21.0 | 12.7 | 16.8 | 10.9 |
| global_intrinsics | 520.9 | 8.1 | 7.2 | 3.2 | 9.0 | 7.1 | 20.0 | 23.0 | 21.4 |
| strings | 558.2 | 11.9 | 5.0 | 3.0 | 9.4 | 8.6 | 16.3 | 22.9 | 21.7 |
| props_class | 913.9 | 22.6 | 8.9 | 11.3 | 4.9 | 12.4 | 9.0 | 6.5 | 22.6 |
| props_poly | 452.9 | 5.1 | 2.8 | 5.1 | 11.8 | 6.2 | 16.6 | 28.1 | 23.3 |
| tree_walk | 630.2 | 13.3 | 4.5 | 9.9 | 9.7 | 23.1 | 2.3 | 17.4 | 18.0 |

Buckets: *allocator* = malloc/free/realloc and glibc's internal allocator functions; *memcpy* =
glibc memmove; *object store* = hashbrown on `IdMap` (`HashMap<Id, Object>`, the GC's
`HashMap<Id, Record>`); *property path* = `PropertyMap`, `Key`, the `objects` module
(`ordinary_get/set`, `define_in_place`, `new_object`, ...), so it is an upper bound;
*names* = environments and global-name hints; *dispatch* = `execute_from` and register access
(with everything inlined into them).

What shapes and inline caches would remove is the key search: `PropertyMap::position`, at most
**4.3%** inclusive across B0 (copywithin_small), 1.5% on props_class and 2.1% on tree_walk.
OBJECT_MODEL.md gated slice 5 on `position` + `ordinary_get` staying above ~15%. `ordinary_get`
is 22-36% inclusive only where it includes getter and method calls (props_class, tree_walk), or
the TypedArray iterator harness (copywithin_small). The key search itself is small.

### Why the allocator and memcpy rows are large

Type sizes at eb06a7f (a throwaway `size_of` test, not committed):

| type | bytes |
|---|---:|
| `Object` | 3 112 |
| `Data` | 2 976 |
| `PropertyMap` | 112 |
| `Property` | 120 |
| `Value` | 56 |
| `Key` | 24 |

`Data` carries every internal slot of every exotic or builtin kind inline: about 35 `Option`
fields (`regexp`, `buffer`, `view`, `typed`, `promise`, `generator`, `collection`, `iterator`,
`activation`, `parameter_map`, `response`, `request`, temporal, ...). A plain `{x, y}` is a
3 KB value. The memmove callers in props_class and objects are `HashMap<Id, Object>::insert`
(2 x 2.2% on props_class): each object creation moves 3 KB into the store, and every store
growth moves all of them. On top of that, each creation does `Dual::allocate` (a zeroed payload
`Vec`, a `records` insert), a `PropertyMap` `Vec`, and one `String` per key (`Key::String(String)`;
interned keys were deferred in stage 4).

`Value` is 56 bytes because of `Value::Native { receiver: Option<Id>, name: String }`, a
placeholder for the host binding. Every register copy, argument and property value pays for
that width (the Value clone/drop row, 4-12%).

## What the current code constrains

- 108 `replace_data` sites, and about 400 reads or writes of the side slots
  (`collection` 85, `iterator` 57, `promise` 48, `buffer`/`view` 43 each, `source` 44,
  `typed` 30, ...). Any layout change must keep the edge multiset that `replace_data` and
  `Data::edges` report to the GC (DUAL_HEAP.md): a slot that moves must still be traced.
- `Object` is an enum (`Ordinary`, `Builtin`, `Bound`, `Closure`, ...) around `Data`. `plain()`
  and the fast paths match on it.
- `Id` is `enum { Dom(Handle), Js(Handle) }`: two heaps. A dense store needs one slab per domain
  and the handle's index (and generation, for reuse safety).
- `#![forbid(unsafe_code)]`: no tagged pointers, no NaN boxing through transmute. A compact
  `Value` must be a plain enum.
- `Value::Native` is part of the Servo bridge (SERVO_INTEGRATION.md). Shrinking it must not
  change the Engine API that the `servo-native` CI job builds against.

## Options

1. **Shapes and inline caches now** (OBJECT_MODEL.md slice 5 as written). This removes at most
   ~4% today, and it would add a shape table on top of 3 KB objects. Rejected as the next step;
   it stays as the last slice, with a new gate.
2. **One kind enum for the internal slots** (`Data { core, kind: Kind }`, one variant per
   exotic or builtin kind, as in SpiderMonkey's `JSClass` and V8's instance types). This is the
   most compact and type-safe. But some objects combine slots today (`arguments` +
   `parameter_map`, `array` + `iterator` state, `boxed` + `string_cursor`), and every one of
   the ~400 sites would change shape and need a per-kind audit. That is too much risk in one
   step.
3. **Cold slots behind one box (chosen first)**: `Data` keeps the hot core (`prototype`,
   `properties`, `extensible`, `array`, `arguments`, small flags) and moves every other
   internal slot into `ext: Option<Box<Ext>>`, where `Ext` has the same field names and a
   `Default`. Plain objects, arrays and closures never allocate `Ext`. The ~400 sites change
   mechanically through accessors (`d.ext()` returns `&Ext` or a static empty one,
   `d.ext_mut()` allocates on first write). `Data` goes from 2 976 to roughly 150-200 bytes.
   Option 2 can follow later, kind by kind, inside `Ext`.

## Design: slices, in order

Each slice gets a compile check, its smoke dirs, B0 before and after, and goes into the next
test262 checkpoint (CLAUDE.md "Large implementation lines").

### L1: cold internal slots behind `Option<Box<Ext>>`

- `Ext` holds every field of `Data` except the hot core. `Data::edges` traces `ext` if present.
  `replace_data` callers are unchanged, because they replace a whole `Data`.
- A single `Ext` is a trap for exotic objects that are hot (literature check #421): iterators,
  generator and async activations, promises and Map/Set would each pay a separately allocated,
  zeroed ~2.8 KB box. So before fixing the grouping, L1 reports `size_of::<Ext>()` and how many
  objects allocate it in B0 and in the smoke dirs. If one `Ext` regresses `exotic` (below), it is
  split into groups (iteration/activation, promise/async, collections, buffers/typed, rare),
  or the hottest kinds move to option 2 inside `Ext`.
- New B0 benchmark `exotic`: `for-of` over arrays and over a generator, `async`/`await` with
  promises, and a `Map` get/set loop. It is measured at eb06a7f before L1 starts.
- Size guards as tests: `size_of::<Object>() <= 256` and one bound per `Ext` group, so a later
  field addition cannot silently bring back the 3 KB objects or the 3 KB box.
- Prediction: memcpy falls by most of its 3-9%, and the allocator and object-store rows shrink
  (smaller store entries, cheaper growth). Kill line: **-15% instructions on props_class and on
  objects, and no more than +5% on exotic**. If L1 misses the first part, the cost model above is
  wrong and this note is revised before L2; if it misses the second, `Ext` is split first.

### L2: compact `Value`

- `Value::Native` becomes `Native(Rc<NativeRef>)` (the receiver and name behind one pointer),
  keeping the Engine API (constructors and matches go through helper functions).
- Target: `size_of::<Value>() <= 24` (`JsString` and `Rc` are one or two words). Measure
  `Value` clone/drop and dispatch before and after.

### L3: dense object store

- `objects: IdMap<Object>` becomes a slab per domain, indexed by the handle's index and checked
  by generation, so a stale `Id` misses instead of aliasing. The GC `records` map gets the same
  treatment if it is still visible in the profile after L1.
- This touches the GC boundary (DUAL_HEAP.md), so it starts with a short section in DUAL_HEAP.md
  and runs `tests/dual_heap*` plus the gc-stress probes.

### L4: interned property keys

- As designed in OBJECT_MODEL.md slice 3 (`JsAtom`, `Key::Index`) and deferred there. Chunk names
  become atoms at compile time, so `SetProp` no longer builds a `Key` from a `&str` on every
  execution and literals no longer allocate one `String` per key per object.
- Property storage becomes keys plus attributes in one vector and values in a parallel
  `Vec<Value>` (same index), replacing the 120-byte `Property`. That is the layout shapes share
  later (the key/attribute half moves into the shape). `size_of` per entry is recorded before
  and after.

### L5: shapes and property inline caches (re-gated)

- Shapes and ICs are one slice, gated together. Boa's shapes PR describes shapes alone as
  mostly a memory optimization, and its execution benchmarks regressed until ICs landed
  (Richards/DeltaBlue-style score 28.1, then 33.6 with shapes, then 42.9 with shapes and ICs).
- Design: compare against Boa PR #2723 first (shared and unique shapes, attributes and prototype
  in the shape, unique-shape conversion of long chains, IC entries `(name, weak shape, slot)`),
  with a dictionary fallback (V8 dictionary mode, Boa unique shapes) for delete-heavy objects.
- Gate, after L1-L4 and a B0 re-run:
  - (a) the cost an IC would remove: `ordinary_get`/`ordinary_set` inclusive minus the bodies of
    called getters and methods, so including the prototype-chain walk and the attribute and
    descriptor handling, not only `position`;
  - (b) bytes per object at steady state for `{x, y}`, a 6-field class instance and a tree node,
    with the per-object property storage that shapes would share;
  - (c) one OO benchmark of the Richards/DeltaBlue kind, added to B0 for this gate.
  Shapes and ICs go ahead if (a) is above 10% on at least two of `objects`, `props_class` and
  `tree_walk`, or if (b) shows per-object property storage is still the largest part of a plain
  object. Otherwise, the next stage is whichever bucket leads B0 at that point.

## Migration without losing passes

- Representation only: no algorithm, key order or edge multiset changes. Each slice is checked
  by `cargo test` (dual_heap and GC budget tests), vmprobe tree/vm/gc-stress (`s7_gate.sh`),
  the 2 500-case equivalence (`vmcompare.sh`), and the smoke dirs that exercise the moved slots:
  built-ins/{RegExp, Promise, Map, Set, WeakMap, ArrayBuffer, SharedArrayBuffer, DataView,
  TypedArray, Date, Proxy, Iterator, GeneratorPrototype, AsyncGeneratorPrototype},
  language/arguments-object, language/statements/class.
- The embedding default stays `Tree` for Servo until roadmap C. Both evaluators share the object
  model, so L1-L4 speed up the tree-walker too, and every slice is checked on both.
- No `Engine` API change in L1, L3 and L4. L2 changes `Value::Native`'s representation and is
  tagged `[servo]` so CI runs `servo-native`.

## Kill lines

- L1: -15% on props_class and objects, at most +5% on exotic (above). One method change allowed
  (`Ext` groups, or option 2 for the heaviest kinds) before revising the note.
- L2-L4: each must move its own row (Value clone/drop, object store, allocator) by at least a
  third on the benchmark where that row is largest, or it is reverted and recorded here.

## References

- C. Chambers, D. Ungar, E. Lee. An efficient implementation of SELF. OOPSLA 1989.
- SpiderMonkey: `JSClass` and `NativeObject` (fixed slots plus a separately allocated slot and
  element store; reserved slots per class), `JS::Value` (8-byte NaN-boxed).
- V8: instance types and in-object properties; `JSObject` header of map, properties and
  elements.
- Boa: `JsObject` (`ObjectData` kind enum, PR #2723 shapes and ICs), `JsValue` 40 to 24 bytes
  (0.9), `JsObject` size reduction (2024), NaN-boxed `JsValue` (v0.21).
- V8: dictionary-mode objects (the fallback for delete-heavy objects).
- QuickJS: per-class object layout.
- OBJECT_MODEL.md (stage 4), DUAL_HEAP.md, BYTECODE_VM.md.

## Second opinions

- Literature check (#82): request #421 (worker task #2569), sent 2026-11-15 before L1, on the
  layout-first order, L1's boxed-slots choice and the L5 gate. Verdict: adopt the order and
  L1's boxed cold slots, with two corrections, both folded in above: gate shapes and ICs
  together on the inclusive lookup cost and bytes per object (not on `position`), and size `Ext`
  per group before declaring L1 done. No source shows that deferring shapes is a known mistake.
  Layout-first is not novel: Boa reduced `JsValue` from 40 to 24 bytes (0.9), reduced `JsObject`
  (2024) and moved to NaN boxing (v0.21); SpiderMonkey and QuickJS size slots per class. Full
  report: task #2569 workspace,
  `docs/literature-check-object-layout-before-shapes-2026-11-15.md`.

## Results

(none yet)

## L1 result (2026-11-15)

Implemented: `Data` keeps the hot core inline (`prototype`, `properties`, `extensible`,
`raw_json`, `arguments`, `array`, `source`) and every other internal slot in
`ext: Option<Box<Ext>>`. Field access goes through `Deref`/`DerefMut` to `Ext` (reads of an
object without one see a per-thread empty `Ext`; the first mutable access allocates it), so the
~400 slot sites compile unchanged. `Data::edges` traces `Ext` only when present. A size guard
test (`layout_tests`) bounds `Object`, `Data` and `Ext`.

Sizes: `Object` 3 112 -> 352 B, `Data` 2 976 -> 192 B, `Ext` 2 792 B. `Object` is now bounded
by `Bound` (two 56-byte `Value`s) and `Environment`, not by `Data`.

B0 callgrind, eb06a7f -> L1 (Ir, millions):

| bench | before | after | delta |
|---|---:|---:|---:|
| decode_tiny | 224.1 | 224.7 | +0.3% |
| copywithin_small | 2519.0 | 2545.6 | +1.1% |
| exotic (new) | 1367.7 | 1387.0 | +1.4% |
| fib_small | 39.0 | 35.7 | -8.5% |
| loop_small | 366.4 | 356.2 | -2.8% |
| closures | 1683.8 | 1562.6 | -7.2% |
| objects | 1361.9 | 1365.3 | +0.2% |
| global_intrinsics | 520.9 | 517.2 | -0.7% |
| strings | 558.2 | 564.1 | +1.1% |
| props_class | 913.9 | 865.2 | -5.3% |
| props_poly | 452.9 | 465.3 | +2.7% |
| tree_walk | 630.2 | 621.9 | -1.3% |

Kill line: the -15% instruction target on props_class and objects is **missed**; exotic stays
within +5%. So the cost model above was wrong about instructions: a 3 KB move is few
instructions (callgrind counts a `rep movs` once), and the allocator row is dominated by the
number of allocations, not their size. L1 is kept for memory (9x less per object, which matters
for DOM-heavy pages in Servo), not for speed.

Decision (Santiago, 2026-11-15: "focus on completion"): slices L2-L5 are parked. Performance
work resumes only where a measured Servo or test262 case is blocked by it. Roadmap C (Servo
bindings) is next.
