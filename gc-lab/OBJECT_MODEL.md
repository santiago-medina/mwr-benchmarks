# Object model and name access (roadmap B, stage 4)

Status: design note, 2026-11-10. No code yet. Follows BYTECODE_VM.md ("Stage 4 (later, own note)")
and PROPERTIES.md ("What is left").

## Problem (measured)

Since checkpoint 38 both function bodies and top-level script code run on the register VM. The
remaining cost is no longer the evaluator. It is the object model and name resolution under it.
Release `js_check` with line tables, `MWR_ENGINE=vm`, callgrind, HEAD ec82fd2 (`scratch/prof.sh`,
stage script `scratch/prof_s4.sh`). Percentages are inclusive.

| benchmark | instructions | where it goes |
|---|---|---|
| `vm-bench/decode_tiny.js` (decodeURI A2.5_T1, shrunk) | 423.4 M | `lookup_with_base` 18.4%, `function_environment` 18.1% (`new_environment` 12.6%), `reference_set` 12.4%, `set_member` 9.6%, `ordinary_get` 8.4%, `scan_level` 4.3%, `PropertyMap::position` 4.6% |
| `vm-bench/copywithin_small.js` (copyWithin coerced-values-*-detached, shrunk) | 7 817.6 M | `array_from`/`consume_iterator` 36-38%, `iterator_result` 28.0%, `replace_data` 24.9% (`Data::edges` 15.4%), `ordinary_get` 20.1%, `define` 18.0%, `new_object` 17.0%, `length_of` 5.0%, `PropertyMap::position` 4.0% |

Callers of `replace_data` in copywithin_small (1.94 G): `new_object` 0.76 G (one clone-and-relink
of an *empty* `Data` per created object), `%ArrayIteratorPrototype%.next` 1.18 G (it clones the
iterator's whole `Data` to bump `index` by one). `new_object`'s caller is `iterator_result`
(1.32 G of 1.33 G). So most of the copyWithin case is spent in the *harness's* `new TA(array)`
over a 1000-element array: `%TypedArray%` construction iterates the source (§23.2.5.1.1
InitializeTypedArrayFromObject, steps 5-6 IteratorToList), one CreateIterResultObject (§7.4.14)
per element.

decode_tiny is a loop over global `var`s (`errorCount`, `indexO`, ...) that calls
`decimalToHexString` and `decodeURI` per iteration. Each global read or write walks the
environment chain with `&str` keys (`scan_level`: nine string-keyed maps or sets per
environment), then reads the global object's `PropertyMap` with an owned `Key::String`, built
per access. Each call to a VM-compiled function still builds two hash-map environments
(`function_environment`: parameters, then body), even when every binding lives in a register.

These five shared test262 timeouts are the bucket: TypedArray copyWithin coerced-values-*-detached
x3, and decodeURI/decodeURIComponent S15.1.3.x_A2.5_T1 x2. The same costs show up on any Servo
page: global-variable loops, `for...of` over arrays, and small helper functions.

## What the current code constrains

- **`Data`** (objects.rs) is one struct with ~35 inline `Option` slots: promise, generator,
  regexp, buffer, typed, collection, Temporal, and so on. Every `clone()` copies all of them, and
  `Data::edges()` chains all of them. 108 call sites use clone + `replace_data` (builtins 13,
  promises 10, generators 10, objects 7, collections 7, ...). They are correct but O(size of Data)
  plus O(edges).
- **GC edge multiset** (dual.rs `Runtime::link`/`unlink`): every store must keep the per-object
  edge list equal to `Data::edges()`. `unlink` is a linear `position` + `remove` in
  `Record::links`, so edge churn on a large object is O(edges). Any new storage, such as
  dense elements or slots, must link and unlink per stored object value, like `define_in_place`
  does today.
- **Keys** are `Key::String(String) | Key::Symbol(u64)`. Array indices are decimal strings
  (`Key::index()` parses them back). `GetProp` builds `Key::from(&str)`, an allocation per
  execution. `GetElem` with a number key formats the number into a `String`.
- **[[OwnPropertyKeys]] order** (§10.1.11.1): integer indices ascending, then strings in
  creation order, then symbols in creation order. `PropertyMap` keeps insertion order, and
  the caller sorts the indices.
- **Exotic objects** are flags on `Data` (`array`, `typed`, `parameter_map`, `namespace`) plus
  `Object::Proxy`. `plain()` is the gate for the in-place fast paths, and it must stay the
  only place that decides "ordinary for this key".
- **Environments** are `Object::Environment` GC objects with `bindings: NameMap<Option<Value>>`
  and seven more name sets. The VM keeps captured names environment-resident and the
  others in registers (BYTECODE_VM.md stage 3). Hatches evaluate an AST expression against
  the frame's environment, and they still read the env chain for `this`, `new.target`,
  `super` and free names.
- **Global names** are environment-resident by design (checkpoint 38): TDZ, `const`, configurable
  global object properties and sloppy direct eval `var` injection (§19.2.1.3) all need run-time
  checks. A cache must be invalidated by every one of those, not bypass them.
- **No `unsafe`, no dependencies**: shapes cannot be raw pointers. They must be indices into an
  engine-owned table (or `Rc`), and an inline cache entry is plain data `(shape id, slot)`.

## Options

1. **Hidden classes (shapes) + inline caches first**, as in Self maps (Chambers, Ungar & Lee 1989),
   V8 maps and SpiderMonkey shapes, with monomorphic/polymorphic ICs per `GetProp`/`SetProp` site
   (Deutsch & Schiffman 1984; Hölzle, Chambers & Ungar 1991). This is the classic answer, but it
   is the largest change: `PropertyMap` becomes shape + slot vector, every one of the 108
   `replace_data` sites and all reflection paths must keep shape and slots consistent. Its
   payoff is on repeated property reads of same-shaped objects. In both target profiles,
   `PropertyMap::position` (the part shapes would remove) is only 4-5%. Rejected as the
   first step, kept as slice 5.
2. **Remove the incidental costs around the object model first (chosen)**: in-place internal
   slot updates and allocation-light object creation, environment elision for non-capturing
   compiled functions, a global-name cache, interned keys, then dense elements. Each slice is
   local, keeps `PropertyMap` semantics, and targets a measured hotspot. Shapes come last, on
   top of interned keys and dense elements, which they need anyway (V8 and SpiderMonkey both keep
   elements apart from named properties).
3. **Fast paths inside builtins** (for example a TypedArray-from-Array path that skips the
   iterator protocol when %ArrayIteratorPrototype%.next and Array.prototype[@@iterator] are
   untouched, V8's "protector" cells). This is legitimate only when the skipped steps are
   unobservable, and it fixes one builtin at a time. It is not taken as an architecture.
   A protector may be added later for `for...of` over arrays, with its own spec argument.

## Design: slices, in order

Each slice has a compile check, its own probe or smoke run, the fixed benchmark before and
after, and goes into the next checkpoint (CLAUDE.md "Large implementation lines").

### Slice 1: in-place internal slots and cheap object creation

- `new_object`: insert `Data::new(prototype)` directly and link only the prototype edge. No
  empty-`Data` + `replace_data` round trip.
- `Engine::slot_mut`-style helpers for internal slots that hold no GC edge, or exactly one
  known edge: array iterator `index`/`target` (§23.1.5.1 %ArrayIteratorPrototype%.next), string
  iterator cursor, collection cursor, enumeration cursor. Edge changes (for example
  `target = None`) unlink exactly that one edge. The iterator `next` paths stop cloning `Data`.
- CreateIterResultObject (§7.4.14): a fresh ordinary extensible object, so both
  CreateDataPropertyOrThrow steps succeed and have no observable side effects. Build the
  `PropertyMap` with two `push`es (keys `value`, `done` in that order), link the value edge, and
  insert in one step.
- Move the rarely used slots of `Data` into `extra: Option<Box<Slots>>`: promise, job, cell,
  generator, activation, response, request, Temporal, regexp, buffer, view, collection, weak,
  dispose, namespace and shadow realm. `Data::edges()` and the remaining clones get cheap.
  Accessors keep their names (`data.promise()` and so on), so call sites change mechanically.
- Expected: copywithin_small -30% or more (it removes most of `replace_data`'s 24.9% and much
  of `new_object` + `define` in `iterator_result`). **This is the kill-line slice** (see below).

### Slice 2: environment elision for compiled calls

- `bytecode::compile` computes `Chunk::needs_env`. It is false when the body has no captured
  names, no direct eval, no `arguments`, no hatch, no `this`/`new.target`/`super` read through
  the environment, and no non-simple parameters that need a separate parameter scope (§10.2.11
  step 28). A body with a hatch or a sloppy direct eval keeps its environment.
- With `needs_env == false`, `invoke_inner` skips `function_environment`. `run_chunk` gets the
  closure's [[Environment]] as `local`, and arguments go straight into parameter registers. The
  receiver (after OrdinaryCallBindThis §10.2.1.2, sloppy coercion included) goes into a frame
  register that the compiler reads for `this`.
- Narrow rule: elide only when the body has no hatch, direct eval, mapped arguments or `with`
  (and none of the other conditions above). This matches V8 context allocation, SpiderMonkey
  scope analysis and QuickJS var_refs. No precedent pairs elision with a tree-walker fallback,
  so the vm/tree equivalence run is the evidence; this is not claimed as novel.
- Spec argument: FunctionDeclarationInstantiation (§10.2.11) creates the environment records,
  but nothing can observe a record that no code resolves a name in. That is the same argument
  as stage 3's register bindings.
- Expected: decode_tiny up to -15% (most of `function_environment`'s 18.1%).

### Slice 3: global-name cache and interned keys

- **Interned keys**: `Key::String` holds a `JsAtom` (an `Rc<str>` interned in an engine-wide
  table, compared by pointer first, then by content). Chunk names become atoms once at compile
  time, so `GetProp`/`SetProp`/`GetName` no longer allocate per execution. `Key::Index(u32)`
  becomes a variant for canonical array indices (CanonicalNumericIndexString, §7.1.21), produced
  directly by `to_key` for integral numbers and kept ordered per §10.1.11.1.
- **Global-name cache** (SpiderMonkey GetGName with a per-global generation counter, Bug
  1799023; V8 PropertyCell): each `GetName`/`SetRef` site in a chunk gets a cache cell
  `(epoch, Location)`. Only positive hits are cached, and only two kinds: "global declarative
  record, binding n" and "own PropertyMap entry n of the global object, plain writable data".
  Anything resolved through the global's prototype chain (Object.prototype, and in Servo the
  WebIDL WindowProperties named-properties object, which changes as the DOM changes) always
  takes the slow path. WindowProperties is out of the epoch's scope by design.
- `names_epoch: u64` is bumped only on events that can change an existing resolution, not on
  every global add (SpiderMonkey moved away from that churn):
  - a new binding that shadows: a global lexical over a global-object property (§16.1.7), an
    eval `var` in an intermediate scope (§19.2.1.3);
  - delete of a global binding or global-object property;
  - a data<->accessor switch or any attribute change on a cached kind;
  - `PropertyMap` compaction or reindex of the global object (entry n moves);
  - a realm switch.
  Epoch bumps per run are reported in `--vm-stats`.
- Eligibility is decided statically per site at compile time: no enclosing `with`, no
  enclosing sloppy direct eval. Such sites never use the cache; the epoch does not try to cover
  them (CVE-2024-3852 was an IC attach bug on BindName/GetBoundName after `eval` in a function).
  For function code, the parent scopes' names are passed to `compile` as a list, and a site
  whose name a parent scope declares is not a global site. TDZ and `const` checks still run on
  the cached binding: the cache saves the lookup, not the check.
- Required probes: a `with` object that gains or loses the name mid-loop; `@@unscopables`
  toggled; `eval("var x")` in an enclosing function after the inner closure has run once;
  `delete globalThis.x`; a global redefined as an accessor; `let x` in a later script shadowing a
  global-object `x`; a mutable object inserted between the global and Object.prototype. Churn
  control: decode_tiny plus a script that defines N new globals inside the loop; the hit rate
  must stay high.
- Expected: decode_tiny -20% or more (`lookup_with_base` 18.4% + `reference_set` 12.4%, minus the
  value read and write that remain).

### Slice 4: dense elements

- `Data::elements: Vec<Value>` with a hole marker (an internal `Value::Hole` that never escapes
  to JS) for array indices whose property is a default data property (writable, enumerable,
  configurable). Indices with other attributes, accessors, or a hole ratio over a bound, such
  as writing index 10^6 into an empty array, go to the sparse `PropertyMap` ("dictionary
  elements", as in SpiderMonkey and V8). Once an object is sparse it stays sparse.
- [[OwnPropertyKeys]] puts the dense indices first. They are already ascending, which makes the
  caller's sort cheap. ArraySetLength (§10.4.2.4) truncates the Vec. Non-extensible, sealed or
  frozen objects (§10.1.4 and §20.1.2.6/.21) convert to sparse first, so attribute checks stay
  in one place.
- `GetElem`/`SetElem` with `Key::Index` on a `plain` object: Vec index, then the prototype chain
  for holes, which stays the spec's OrdinaryGet/OrdinarySet (§10.1.8.1, §10.1.9.2).
- Each stored object value links its edge. The O(n) `unlink` in `Record::links` becomes a
  problem for big arrays of objects: replace it with a counted map, but only if a benchmark
  shows it. That is a GC change, so it would get its own section in GC_SCHEDULING.md or DUAL_HEAP.md.
- Expected: copywithin (`makeArrayLike`/`copyIntoArrayBuffer` loops), buildString and
  property-escape cases; measured on copywithin_small and `pe-bench`.

### Slice 5: shapes and property inline caches

- An engine-owned shape table (`Vec<Shape>`, id `u32`). A shape is (parent, key, attributes,
  slot) with a transition map. `PropertyMap` becomes `(shape, Vec<Value-or-accessor>)` for
  ordinary objects and stays the dictionary representation for objects over a threshold or
  after deletes (V8's dictionary mode).
- Per-site monomorphic IC on `GetProp`/`SetProp`: `(shape id, holder depth, slot)`, checked against
  the receiver's shape (and the holder's for prototype hits, with prototype shapes as V8
  "prototype validity" via an epoch). Polymorphic up to 4 entries, then megamorphic, meaning the
  generic path.
- Only after slices 1-4 have been measured. If `PropertyMap::position` + `ordinary_get` are still
  above ~15% on the benchmark set by then, this slice gets its own literature check. Compare
  against Boa PR #2723 (shared/unique shapes, IC entry = name + weak shape + slot) before
  writing a new design.

## Migration without losing passes

- Every slice keeps the observable algorithms: the same OrdinaryGet/Set/DefineOwnProperty steps,
  the same key order, the same edge multiset. Only representation and lookup cost change.
- Gates per slice: `cargo test` (dual_heap and GC budget tests), vmprobe tree/vm/gc-stress, the
  2500-case equivalence (`vmcompare.sh`), and smoke dirs chosen for the slice:
  - slice 1: built-ins/{Array,TypedArray,TypedArrayConstructors,ArrayIteratorPrototype,
    Iterator,Map,Set};
  - slice 2: language/{arguments-object,expressions/call,statements/return,eval-code,
    function-code}, including the TCO cases (APRENDIZAJES 2026-11-10);
  - slice 3: language/{global-code,eval-code,statements/with,identifier-resolution},
    annexB/language;
  - slice 4: built-ins/Array, built-ins/Object/{freeze,seal,defineProperty,keys,
    getOwnPropertyNames}, language/expressions/{array,assignment};
  - slice 5: built-ins/Object, built-ins/Reflect, language/expressions/{object,class}.
- `MWR_ENGINE=tree` keeps working. The tree-walker shares the object model, so slices 1, 3
  (keys) and 4 speed it up too, and slices 2 and 3 (cache) are VM-only.
- Each slice is one commit (or a short queue) and goes through the next test262 checkpoint.

## Benchmarks and kill lines

- Fixed set: `vm-bench/{decode_tiny,decode_small,copywithin_small}.js` (callgrind instructions,
  `scratch/prof_s4.sh`), plus `vm-bench/run.sh` wall times and the `pe-bench` set. Baseline
  numbers are in the table at the top (HEAD ec82fd2).
- Kill line from ESTADO: the first slice must move decode_tiny or copywithin_small by at least
  30% in instructions. Slice 1 is the one predicted to do it, on copywithin_small. If slices
  1-3 together move neither benchmark by 30%, this note is revised before slice 4. The
  profile would then be showing a cost that this decomposition does not model.
- Each slice records its before/after numbers in a "Results" section below.

## References

- C. Chambers, D. Ungar, E. Lee. An efficient implementation of SELF, a dynamically-typed
  object-oriented language based on prototypes. OOPSLA 1989 (maps).
- L. P. Deutsch, A. M. Schiffman. Efficient implementation of the Smalltalk-80 system. POPL 1984
  (inline caches).
- U. Hölzle, C. Chambers, D. Ungar. Optimizing dynamically-typed object-oriented languages with
  polymorphic inline caches. ECOOP 1991.
- V8 design docs: hidden classes (maps), elements kinds and dictionary elements, PropertyCell for
  global properties, protector cells.
- SpiderMonkey: Shapes, NativeObject dense elements (independent of shape), GName/BindName ICs,
  Bug 1799023 (global generation counter), CVE-2024-3852 (BindName IC after eval).
- Boa PR #2723 (shapes and inline caches in a Rust engine).
- ECMA-262: §7.4.14, §9.1, §10.1.8-10.1.11, §10.2.11, §10.4.2.4, §16.1.7, §19.2.1.3, §23.1.5.1,
  §23.2.5.1.

## Second opinions

- Literature check (#82): request #376 (worker task #2486), answered 2026-11-10. Verdict:
  adopt the order, with corrections. Deferring shapes is not a known mistake: SpiderMonkey
  keeps dense elements independent of shape and guards GetGName with a per-global generation
  counter (Bug 1799023), not a shape guard. Two details of the first slice-3 draft matched
  documented failure modes and are fixed above: an epoch bump on every global add (churn), and
  the global's mutable prototype chain (Servo's WindowProperties), which no epoch sees. Slice 2
  is stated narrowly; slice 5 compares against Boa PR #2723 first. Full report:
  /home/santi/tasky_spaces/task-2486-solicitud-376-request-literature-check-r/docs/literature-check-object-model-slice-order-2026-11-10.md
- taskygpt: optional since 2026-11-10 (Santiago waived the gate after repeated HTTP 429). Tried
  once on 2026-11-10 22:40 CET: HTTP 429 `credit_balance_exhausted` again, so there is no second opinion.

## Results

### Slice 1 (part a): in-place slots in `new_object`, `iterator_result`, `%ArrayIteratorPrototype%.next`

Callgrind instructions, release `js_check`, `MWR_ENGINE=vm`, same inputs as the baseline:

| benchmark | before | after | delta |
|---|---:|---:|---:|
| copywithin_small | 7,817.6 M | 5,169.6 M | -33.9% |
| decode_tiny | 423.4 M | 421.9 M | -0.4% |

The kill line (>= 30% on one of the two) is met by copywithin_small. decode_tiny is
untouched as expected: its cost is environment creation and name lookup (slices 2 and 3).
Wall time of the shared timeout bucket after the slice (unloaded machine, 10 s runner
limit): copyWithin coerced-values-start-detached 9.4 s, coerced-values-end-detached
11.0 s; decodeURI / decodeURIComponent S15.1.3.x_A2.5_T1 16.8 s each. What remains in
copywithin_small: `ordinary_get` 30.8%, `consume_iterator` 32%, `put_data`/`define` ~12%,
`array_define` and `append_array` ~10% each. Those are slice 3 and 4 territory.

### Slice 2: environment elision for compiled calls

`bytecode::compile` sets `Chunk::elide` (`elidable` in bytecode.rs) under the narrow rule above.
The rule: a simple parameter list with no duplicates; no direct eval; no `arguments`; no captured
names; no hoisted declarations; no block environments; no `Insn::Hatch`. The body must also not
mention `this`, `new.target`, `super`, `with`, `import.meta`/`import()`, nested functions,
arrows or classes. `invoke_inner` then skips `function_environment` for Ordinary, Arrow and
Method kinds. It still consumes the [[Construct]] marker. `run_chunk_elided` runs the chunk in
the closure's [[Environment]] and writes the arguments straight into the parameter registers.
Strictness comes from the body (`Stmt::Sloppy`), not from the environment, so a strict function
in a sloppy script still throws on an unresolvable write. The receiver is never read, so the
sloppy `this` coercion (§10.2.1.2) is skipped. That coercion is unobservable: ToObject of a
primitive has no side effects. Tail calls keep the §15.10.1 rule: strict bodies only.

Callgrind instructions, release `js_check`, `MWR_ENGINE=vm`, decode_tiny with
`decimalToHexString.js` included, as in the baseline (`scratch/prof_s4.sh`, fixed to pass the
include):

| benchmark | before (slice 1) | after | delta |
|---|---:|---:|---:|
| decode_tiny | 421.9 M | 339.2 M | -19.6% |

On decode_tiny, 4162 of 4163 VM calls are elided (`--vm-stats` prints `elided=`). Slices 1+2
together take decode_tiny from 423.4 M to 339.2 M (-19.9%). The kill line asks slices 1-3 for
-30%, which leaves slice 3 (global-name cache) to find about 10 more points. Probe:
`vmprobe/elide.js` (tree, vm, gc-stress).

### Slice 3 (part a): global-name slot hints

What was implemented differs from the epoch design above, and the difference makes it narrower
and safer. `Chunk::hints` holds one `Cell<u32>` per chunk name: the index of the name's entry in
the global object's `PropertyMap` (`global_names.rs`). The environment walk of §9.1.2.1 still runs
on every access. The hint replaces only the final global object record step (HasBinding,
GetBindingValue, SetMutableBinding), and only for an ordinary global object with an own data
property. Every use validates the hint by key (`PropertyMap::named_at`). A delete, an accessor
redefinition or a reindex therefore makes the check fail, and the slow path runs. No epoch is
needed, and nothing has to bump one. Shadowing by `with`, an eval `var` or a later script's
`let` is found by the walk before the hint is consulted. Accessors and read-only properties
mark the hint `FAILED` and stay slow. The sites that use hints are `GetName`, `GetCallee`,
`TypeofName`, `ResolveName` and the `GetRef`/`SetRef` binding cases.

The interned keys (`JsAtom`, `Key::Index`) are deferred. With the hint, the per-access
`String` allocation of the global lookup is gone (the key compare is a short `memcmp`). The
remaining costs are now the walk itself (`scan_level` 6.2%) and the Debug-based `ast_scan::mentions`
compile-time scans (about 8%; see below).

Callgrind, same setup as slice 2:

| benchmark | before (slice 2) | after | delta |
|---|---:|---:|---:|
| decode_tiny | 339.2 M | 271.4 M | -20.0% |

Slices 1-3 together take decode_tiny from 423.4 M to 271.4 M (-35.9%), which meets the -30% kill
line. Slice 4 can go ahead without revising the note.

The probe is `vmprobe/globals.js` (tree, vm, gc-stress). It covers all the probes the note
requires: a `with` object that gains or loses the name mid-loop, a toggled `@@unscopables`,
an eval `var` after the inner closure ran, `delete globalThis.x`, accessor and read-only
redefinition, a later-script `let`, an object inserted into the global's prototype chain,
and a churn loop.

The probe also found a slice 2 bug, fixed here. An elided strict function in a sloppy script
read its strictness for an assignment from the closure environment. A write to a read-only
global therefore failed silently instead of throwing a TypeError. `assign_binding_hinted` now
takes the strictness of the assigning code (§6.2.5.6 PutValue, the Reference's [[Strict]]).

The next cheap win, found in this profile, is `ast_scan::mentions`. It scans the AST by
formatting it with `Debug` and searching the text: about 22 M instructions (about 8%) on
decode_tiny, all compile-time. A structural walker would remove it, and on real pages this
cost scales with script size.

### Structural AST walker (checkpoint 42)

`ast_scan.rs` now walks the AST with a `Walk` trait (`Node` = expression, statement,
pattern, property, class member or string; the visitor can continue, skip a subtree or
stop). Every match is exhaustive with no `..` rest pattern, so a new AST field fails to
compile until the walker covers it. Every query is now a predicate on nodes:

| query | before (Debug text) | now |
|---|---|---|
| direct eval, argumentsObjectNeeded | quoted needle | `Name`/`Pattern::Name` or exact string |
| `touches_locals`, switch TDZ, `named` | quoted name | exact string, `HashSet` of locals |
| `captured_names` | bracket-matched text regions | closure nodes (`is_closure`), strings inside |
| for-let capture, elision | variant names as substrings | node kinds |
| `writes` (operand reuse) | `Assign(Name(x)`, `Update(Name(x)`, `", Name(x), ` | `Assign`/`Compound`/`Update` on `x`, and `Destructure` naming `x` |

A temporary differential build ran both versions side by side over 7951 test262 files (every
sixth file of language/ and built-ins/, VM engine) and logged every disagreement. There were
none. On hand-written cases they disagree only where the text scan was wrong: identifiers
such as `Arrow` or `Function` used to count as closures. The old `writes` also missed one case,
which was a real bug: `x + ([x] = [5], 0)` evaluated to 5 instead of 1, because a destructuring
assignment (§13.15.5) overwrote the register the left operand still pointed at. The fix is in
this checkpoint, and `vmprobe/operands.js` covers it.

| benchmark | before (slice 3) | after | delta |
|---|---:|---:|---:|
| decode_tiny | 271.4 M | 256.0 M | -5.7% |

### Slice 4: dense elements (checkpoint 43)

What landed differs from the design above in one point. `PropertyMap` keeps a second store,
`elements: Vec<Option<(Key, Property)>>`, indexed by the array index. Each slot holds the full
property, so it can have any attributes and can be an accessor. The `Key` is kept too, so
iteration can still yield `&(Key, Property)`. Freeze, seal and non-standard attributes
therefore need no conversion to sparse. The descriptor checks stay in their single place
(`define`, ValidateAndApplyPropertyDescriptor), which was the reason for that conversion. The
remaining rules:

- An index key goes to `elements` if it already fits, or if the map is not sparse and the index
  is below `2 * present + 16`, where `present` counts the stored elements. With that bound, holes
  never outnumber the present elements by more than 16. A first version used the Vec length
  there, and `x[2^k - 2]` for k = 1..32 then grew the Vec toward 2^31 slots
  (built-ins/Array/S15.4_A1.1_T10.js timed out, caught by the slice smoke). Any other index goes to the ordinary entries and marks the map
  sparse. A sparse map never grows its `elements` again ("once sparse stays sparse"). An index
  below `elements.len()` is never in the entries, so a lookup checks one store only.
- Iteration yields the elements in ascending order, then the entries. Callers already sort the
  indices for [[OwnPropertyKeys]] (§10.1.11.1), and the sort now finds them presorted.
  Removing a slot trims trailing holes. ArraySetLength (§10.4.2.4) deletes through `retain`, which covers both stores.
- `script/elements.rs` holds the fast paths. Each one is an exact shortcut of a single spec
  operation for an index on an object whose element access is ordinary: no typed array,
  namespace, mapped arguments or proxy. String objects qualify, because their code-unit
  properties are stored data properties. Every precondition is checked before anything
  changes. On any other case the path reports "not handled", and the generic string-key path
  runs instead.
  - `get_element`: OrdinaryGet (§10.1.8.1), walking the chain through holes.
  - `put_element`: CreateDataProperty (§7.3.5) when it cannot fail, plus the `length` bump of
    §10.4.2.1.
  - `set_element_fast`: OrdinarySet (§10.1.9.2), for an own writable data property, or an index
    absent from the chain or inherited writable on an extensible receiver.
  - `array_length_fast`: LengthOfArrayLike (§7.3.18) on an Array, which is one read of its own
    `length`.
- VM `GetElem`/`SetElem` take these paths when the key register holds a Number that is already
  a canonical index, so no key string is built.
- `iterator_step_value` implements IteratorStepValue (§7.4.8) for for-of (VM and tree),
  destructuring, spread and `Array.from`. When [[NextMethod]] is `%ArrayIteratorPrototype%.next`
  and the iterator has [[IteratedArrayLike]], the call would create a fresh
  CreateIterResultObject (§7.4.14) and read back its own `done` and `value` data properties, so
  the object cannot escape. The steps of §23.1.5.1 then run directly and the object is never
  allocated. This holds only in a single-realm engine. With realms, the call would switch to the
  function's realm for its TypeErrors.
- No counted-map `unlink` was needed: no benchmark showed `Record::links` cost.

| benchmark | before (checkpoint 42) | after | delta |
|---|---:|---:|---:|
| copywithin_small | 5,094.4 M | 2,530.8 M | -50.3% |
| decode_tiny | 256.0 M | 257.4 M | +0.6% |

Micro scripts (`scratch/micro/*.js`, 1000 elements each; instructions above an empty script),
with dense storage only → with the fast paths: for-of over an array 19.9 M → 5.7 M, `Array.from` 24.1 M → 6.8 M, typed-array
construction from an array 29.4 M → 10.2 M, indexed loop read 6.7 M → 5.9 M, indexed loop
write 7.2 M → 6.5 M, `push` 23.0 M → 20.5 M. `push` still goes through `Set(O, "length")` and
the name-dispatched builtin call. Builtin dispatch by name string is the next cost on these
paths.
