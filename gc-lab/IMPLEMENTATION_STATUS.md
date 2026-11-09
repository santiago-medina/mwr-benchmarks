# Implementation-only work

User direction: implement without creating or executing tests. Checkpoints use [skip ci]. No new implementation is verified or included in previous conformance/native proof claims. Historical evidence remains tied to its recorded commit.

Added in this batch: Map and Set constructors, SameValueZero lookup, mutation, size, callbacks, iterable ingestion, iterator closing on abrupt entry processing, ordered mutable iteration, species and prototype descriptors, and traced collection slots.

This is not 100% ECMAScript. Outstanding major areas include the full grammar, classes/private fields, generators, proxies, weak references/ephemerons, regular expressions, BigInt, typed arrays/buffers, date/time, internationalization, dynamic compilation, complete module surface, and the remaining intrinsic methods and edge semantics.

Array additions (unverified): Array.from/of, slice/splice, shift/unshift, reverse, fill/copyWithin, at/includes/lastIndexOf, find/findIndex/findLast/findLastIndex, reduce/reduceRight, and stable sorting. Generic operations reuse property descriptors, species construction and GC rooting.

JSON additions (unverified): syntax parsing, materialization, reviver traversal, stringify with toJSON/replacer/property lists, indentation, cycle detection and array/null omission rules. Current scalar-value String storage cannot represent lone UTF-16 surrogates; JSON reports that as Unsupported. Recursion is bounded by host limits.

Operator additions (unverified): conditional expressions with resumable async branches, bitwise operators and shifts, bitwise NOT, and right-associative exponentiation with unary-left syntax restriction. Numeric conversion helpers are shared with intrinsics.

Numeric additions (unverified): Math constants and 36 numeric methods, including compensated/scaled hypot, signed-zero round/min/max, f16round, fround, integer multiplication, and a per-thread randomly seeded hash stream for Math.random. Added parseFloat and global/Number predicates, with Number parsing aliases.

String additions (unverified): primitive prototype property lookup without eager boxing, Unicode code-point iteration, fromCharCode/fromCodePoint, code-unit/code-point access, concatenation, searches, slice/substring, padding/repetition, casing and exact ECMAScript whitespace trimming. Operations requiring an isolated surrogate still report Unsupported because the runtime String representation has not been replaced with UTF-16 storage.

Promise/error additions (unverified): all/race/allSettled/any, withResolvers and try, traced aggregate state and per-element once guards, AggregateError, Error.prototype.toString, cause options, and Error newTarget prototypes. These reuse the existing job queue and iterator closing; no tests have been added or run.

Reference/operator additions (unverified): shared once-evaluated assignment references, prefix/postfix increment/decrement, arithmetic/bitwise/logical compound assignment, nullish coalescing with parenthesization restrictions, and async lowering that stores canonical property keys before suspension. Existing plain assignments now use the same reference path.

Control-flow additions (unverified): while, do/while, classic for, empty/debugger statements and comma expressions. Let loop environments are copied per iteration, and resumable loop handlers route break/continue through finally blocks. Await inside for-of remains a separate missing path at this checkpoint.

Resumable iteration additions (unverified): for-of now lowers into the async instruction stream, preserving per-iteration lexical scopes and closing the iterator for body break/return/throw after finally processing. Iterator-step failures bypass closing. Iterable constructors, synchronous for-of and resumable for-of share iterator opening. This adds await in ordinary for-of bodies; it does not yet add for-await-of or the async iterator protocol.

Symbol additions (unverified): descriptions, Symbol.for/keyFor registry, boxing and prototype methods/accessors, well-known identities, Symbol.toPrimitive coercion hooks, custom Symbol.hasInstance dispatch and Symbol.toStringTag branding. Registry and descriptions survive host rebinding/suspension with the engine. Cross-engine shared-agent symbol registries are not yet provided.

Object/array additions (unverified): Object.hasOwn/values/entries/fromEntries/getOwnPropertyDescriptors/groupBy, Map.groupBy, species-aware concat/flat/flatMap, dense copying toReversed/toSorted/toSpliced/with, and Symbol.isConcatSpreadable. Flattening uses an explicit host depth limit. All additions remain uncompiled and untested under the current implementation-only direction.

Dynamic-function/URI additions (unverified): Function and AsyncFunction constructors reuse the existing strict subset parser, validate parameter text independently, capture the global environment, honor newTarget prototypes and retain generated source text. Existing closures without retained source use the host source-unavailable native representation. Sloppy-mode dynamic-function semantics and broader parameter grammar remain missing. Added encodeURI/encodeURIComponent/decodeURI/decodeURIComponent with malformed-percent and UTF-8 rejection. Lone-surrogate input still depends on the pending UTF-16 representation change.

Set/index additions (unverified): union/intersection/difference/symmetricDifference, subset/superset/disjoint predicates, set-like operand records, live versus copied traversal and iterator closing on early exits. Map/Set now use a canonical SameValueZero hash index alongside insertion-order tombstones; mutation still copies traced object data and rebuilds graph edges, so this is not an overall constant-time mutation claim. Algorithm reference: https://tc39.es/ecma262/multipage/keyed-collections.html#sec-set-objects .

For-in additions (unverified): declaration-based enumeration loops, prototype-level key snapshots, non-enumerable shadowing, deletion checks, nullish empty enumeration, per-iteration lexical bindings and async bodies. Internal enumeration captures its own next function and bypasses user iterator/return hooks. For-in/of assignment targets and destructuring remain pending.

Switch additions (unverified): case/default parsing and fallthrough, shared lexical case scope and hoisting, strict-equality selection, separate break/continue validation contexts, and resumable case expressions/bodies. Switch breaks and loop continues route through existing finally handlers.

Spread additions (unverified): array/call/new spread, sparse array literal accumulation, object spread with symbol/enumerability handling, trailing call commas, and resumable spread expressions. Array and argument accumulation share rooted iterable ingestion; object spread shares a descriptor-based CopyDataProperties path suitable for future object rest bindings.

Parameter additions (unverified): explicit parameter records, defaults, rest arrays, single-parameter arrows, generalized parenthesized arrow lookahead, function length stopping at first default/rest, parameter TDZ and a separate initializer environment, shared ordinary/async setup, and async rejection on initializer exceptions. Body var bindings copy parameter/arguments values while defaults remain isolated from body declarations. Destructuring parameters are still pending. Also updated module validation for the separate switch context.

BigInt arithmetic groundwork (unverified, not yet connected to JavaScript at this checkpoint): immutable signed-magnitude limb operations, arithmetic/bitwise shifts, division/remainder, radix parsing/formatting and exact binary64 integer conversions. A 65,536-bit host size limit is defined; VM work-budget charging and language/coercion integration follow separately. This is not a completed BigInt implementation claim.

BigInt language integration (unverified): BigInt literals and intrinsic/prototype, arithmetic/bitwise/shift/exponentiation operators, updates, boxing, typeof/truthiness, Number conversion, integer-width truncation, radix formatting, numeric comparison/equality and JSON rejection/toJSON handling. Arithmetic/formatting/parsing are charged to the VM work budget and bounded by a 65,536-bit host limit. The lexer now accepts integer radix prefixes and digit separators; StringNumericLiteral decimal validation is separate from parseFloat. BigInt locale formatting and typed-array integration remain pending.

Binary storage additions (unverified): ArrayBuffer fixed/resizable stores, byteLength/maxByteLength/resizable/detached accessors, resize, species slice, transfer/fixed transfer, DataView construction and traced buffer retention, bounds revalidation after coercions, endian-aware integer/BigInt/float16/32/64 reads and writes, and isView. Byte stores use fallible allocation with per-store 16 MiB and per-engine 64 MiB host quotas. Algorithms consulted against the current TC39 structured-data draft; typed arrays, shared buffers and Atomics remain pending.

Typed-array core (unverified): integer/float16/32/64/clamped/BigInt concrete constructors, buffer/typed/iterable/array-like overloads, length-tracking resizable views, integer-indexed get/has/define/set/delete/ownKeys, size/buffer/tag getters, iteration and extension/integrity constraints. Numeric element writes revalidate bounds after coercion. Most typed-array prototype/static methods remain pending at this checkpoint. Also rooted custom prototypes across allocation in the common object path, typed allocation and dynamic Function construction.

Typed-array algorithms (unverified): at, join, includes/indexOf/lastIndexOf, callbacks/find variants/map/filter, reduce/reduceRight, sort/toSorted, content-type-checked species creation and numeric/BigInt/signed-zero/NaN comparison. Ordinary arrays and typed arrays now share one stable fallible merge sorter. Typed-array static from/of and copying/mutating methods still follow separately.

Typed-array copy/static additions (unverified): from/of, set with overlap snapshots and same-type bit preservation, species slice/subarray including length-tracking subviews, reverse, fill, copyWithin, with and toReversed. Slice preserves sequential raw-copy behavior for aliasing species results. Locale formatting, shared buffers and Atomics remain separate gaps.

Generator continuation groundwork (unverified, not yet exposed by parsing at this checkpoint): generalized activation targets, traced generator states, next/throw/return injection, yield output, completion cleanup and reentrancy guards. Promise activations keep their existing external API. Generator grammar and bootstrap wiring follow separately; yield delegation and async generators remain pending.

Generator frontend (unverified): function* declarations/expressions and generator methods, yield expressions and grammar context, hoisting and function prototypes, lazy generator object creation, and next/throw/return resumption through existing loop/finally handlers. Yield-star delegation, dynamic GeneratorFunction and async generators are not yet exposed at this checkpoint.

Generator delegation additions (unverified): yield-star caches next, forwards exact non-done iterator result objects, routes next/throw/return and missing-throw close behavior, and propagates delegate exceptions through the shared completion handlers. Added dynamic GeneratorFunction through the strict subset parser and default/named generator module exports. Async generators remain pending.

Date additions (unverified): Date construction/copy/call, now/parse/UTC, ISO and engine-format round trips, getters/setters, legacy year/GMT aliases, generic toJSON and toPrimitive. Gregorian component arithmetic reuses exact integer operations before TimeClip. The current host time zone is explicitly UTC, and non-ECMA-402 locale methods use host-defined UTC formats; named-zone/DST integration and ECMA-402 localization remain pending.

Binding-pattern additions (unverified): array/object destructuring declarations and parameters, defaults, elisions, computed property keys, array/object rest, bound-name/TDZ validation, module exports and iterator closing. Ordinary/async parameter setup shares the same binder. Async initializers are lowered before binding; suspension inside pattern defaults/keys is explicitly still unsupported. Assignment patterns and loop-header patterns follow separately.

### Destructuring loop headers (unverified)

Classic `for` declarations and `for…in`/`for…of` declaration headers now share binding patterns with declarations and parameters. Lexical loop environments declare every bound name; per-iteration `let` copies include destructured names. Ordinary iteration preserves the enclosing lexical scope for `var`, and binding failures use the existing iterator-close path. Async lowering accepts non-suspending patterns; suspension inside pattern defaults/computed keys remains unsupported. No compilation or tests were run.

### Catch binding forms (unverified)

`catch` now supports omitted bindings and object/array binding patterns through the shared initializer. Catch parameter and body lexical environments are separate, and exceptions during destructuring become pending completions so `finally` can execute. Resumable catch scopes use the same environment split; suspension inside catch patterns is explicitly unsupported. Corrected a stale parameter-record field in the parser left by the earlier binding-pattern refactor. Formatting only; no compilation or tests were run.

### Literal string search/replacement protocols (unverified)

Implemented string-pattern `replace`, `replaceAll`, and `split`, with object `Symbol.replace`/`Symbol.split` dispatch before receiver string conversion, callable replacers, UTF-16 match positions, empty-pattern advancement, split limits, and literal replacement tokens (`$$`, `$&`, prefix, suffix). `replaceAll` observes `Symbol.match` and global flags, and includes/startsWith/endsWith reject regexp-like objects through that marker. Shared bounds cap output and charge search comparisons. There is no RegExp internal implementation yet; symbol hooks work for user objects. Results needing isolated UTF-16 surrogates remain explicitly unsupported. Formatting only; no compilation or tests were run.

### Multiple declarators (unverified)

Added ordered declaration lists for `const`, `let`, and `var`, including destructuring, classic `for` initializers, and named module exports. A shared direct-statement expansion exposes each declaration to binding instantiation, lexical conflict checks, var hoisting and export collection without creating an artificial scope. Ordinary and resumable execution initialize declarators in source order. Per-iteration lexical copying includes all `let` declarators. No compilation or tests were run.

### Computed object property names (unverified)

Object literals now accept computed data, method, generator-method, async-method and accessor names, including symbol keys. Parser property handling is consolidated into one path; computed `__proto__` is an ordinary property. Ordinary execution converts keys before values. Resumable lowering captures canonical keys before evaluating suspending values and retains function creation at property definition. Anonymous functions receive inferred property names, including symbol descriptions; accessors receive get/set name prefixes. Async generator methods and `super` semantics remain unsupported. No compilation or tests were run.

### Destructuring assignments (unverified)

Assignment expressions now convert array/object cover forms to shared patterns, including nested defaults, computed keys, array/object rest and member-expression targets. Initialized shorthand cover forms are accepted for assignment conversion and rejected as ordinary object expressions. The shared initializer switches between binding initialization and reference assignment; element/property destination references are evaluated before reading the corresponding iterator/property value. Existing iterator closing and object rest copying are reused. Awaiting the RHS is lowered; suspension inside targets/defaults/keys remains explicitly unsupported. Cover-grammar restrictions, including trailing commas after assignment rest and duplicate prototype-named properties, still need completion. No compilation or tests were run.

### Existing iteration destinations (unverified)

`for…of` and `for…in` now accept existing identifier/member destinations and destructuring assignment patterns, in ordinary and resumable bodies. Header recognition respects nested delimiters and parses candidate targets independently; iteration reuses the surrounding environment and the common reference-assignment path instead of creating declaration bindings. Abrupt assignments retain existing iterator closing. Existing declaration-header `NoIn` grammar limitations remain pending. No compilation or tests were run.

### Template literals and tags (unverified)

Added nested template lexing, cooked/raw segments, substitutions, tagged templates and `String.raw`. Template escapes support Unicode/code-point and hex forms, continuations, raw line-ending normalization, and undefined cooked entries for malformed tagged escapes. Untagged malformed escapes are syntax errors; valid results requiring isolated UTF-16 surrogates remain explicitly unsupported. Each AST site has stable identity; cooked/raw arrays are frozen and cached through traced edges from the owning realm environment, with stale metadata pruned by GC. Tags share call-target evaluation with ordinary calls, preserving method receivers. Async/generator lowering captures the tag and template object before substitutions and coerces each untagged substitution before the next suspension. New-expression/tag grammar edge cases and the existing UTF-16 representation remain incomplete. Formatting only; no compilation or tests were run.

### Async generators and for-await-of (2026-11-08)

`async function*` declarations, expressions, object/class methods, module exports and the dynamic `AsyncGeneratorFunction` constructor run on the resumable instruction stream. Each async generator keeps a request queue (next/throw/return, each with its own promise), and `yield` awaits its operand. A `return` operand and a return resumption are awaited before finally blocks run. `yield*` uses `@@asyncIterator` with an async-from-sync fallback. `for await (... of ...)` awaits each `next()` result. Also added `%AsyncIteratorPrototype%[@@asyncIterator]`, `%AsyncGeneratorPrototype%` and the AsyncGeneratorFunction prototype chain. Design: ASYNC_GENERATORS.md. Known simplification: on an abrupt exit from for-await, `return()` is called but its result is not awaited.
Harness and engine fixes surfaced by the newly running cases: native `assert.compareArray` (test262 moved it into assert.js), and the `arguments` object is now iterable.
Full test262 (strict script variant): 25010/47708 pass (52.4%), up from 20647; no previously passing case lost. Baseline in conformance/results-full/summary.json. `Array.fromAsync` is still missing.

### Lexer: Unicode identifiers, string escapes and RegExp literals (2026-11-08)

Identifiers follow Unicode 17 ID_Start/ID_Continue (generated tables in unicode_id.rs, from gen_unicode_id.py), with `\uXXXX`/`\u{...}` escapes in identifiers and private names; an escaped reserved word is never a keyword. String literals are cooked by the shared template escape cooker (line continuations, legacy octal rejected in strict code). RegExp literals are tokenized with goal disambiguation (a brace stack tells block `}` from expression `}`, including function-expression bodies), but evaluating one is still reported as unsupported. Source and token caps raised to 8 MiB and 1 Mi tokens. Design: LEXER.md.
Full test262 (strict script variant): 26538/47708 pass (55.6%), up from 25010; no previously passing case lost. Baseline in conformance/results-full/summary.json. Next blocker for Temporal and Array.fromAsync: a RegExp engine ("RegExp literal", 2845 cases).

### RegExp engine (2026-11-09)

Own safe-Rust RegExp with no external crate (design: REGEXP.md). The pieces:
- regexp.rs: the full pattern parser (u/v modes, Annex B, named and duplicate groups, modifiers, class set operations, `\p` from Unicode 17 tables generated by gen_unicode_props.py) and a bytecode backtracking VM over UTF-16 with a 50M step budget.
- regexp_builtins.rs: the RegExp constructor, species and escape; exec, test, toString, compile and the flag/source getters; the @@match, @@matchAll, @@replace (`$<name>`, `$1`–`$99`), @@search and @@split methods; %RegExpStringIterator%; `d`-flag indices; String match, matchAll and search.

Invalid literals are early SyntaxErrors.

Full test262 (strict script variant): 28157/47708 pass (59.0%), up from 26538; no previously passing case lost. Most of the 1694 newly running cases that fail need Temporal (1343, not implemented). Another 372 are property-escape tests that hit the script execution step limit.

### Sloppy mode and `with` (2026-11-09)

Non-strict scripts (design: SLOPPY_MODE.md). Directive prologues set strictness per script and function; legacy octal literals and escapes are accepted only in sloppy code. Environments carry strictness, `with` object records and immutable bindings. Sloppy assignment creates globals, sloppy `delete` of identifiers works, and `this` is coerced for sloppy functions. In strict code an unresolvable name now throws before the right-hand side is evaluated (`Reference::Unresolvable`). The runner runs noStrict and raw cases as sloppy scripts (`--sloppy`).

Full test262 (strict prefix, or sloppy for noStrict/raw): 29100/47708 pass (61.0%), up from 28157; no previously passing case lost. Mapped `arguments` objects (aliasing with parameters) are not implemented yet: the two gated cases that now run and need them are listed as known failures in the arrays and async profiles. eval is next.

### eval, labels, mapped arguments, negative parse tests (2026-11-09)

- Direct and indirect eval (src/script/eval.rs; PerformEval and EvalDeclarationInstantiation, notes in SLOPPY_MODE.md). Full test262: 30032.
- Labelled statements with labelled break/continue, and their early errors. Full test262: 30117.
- Mapped `arguments` objects for sloppy functions with simple parameters (10.4.4). Full test262: 30160.
- The runner now runs parse-phase negative tests (`js_check --negative-parse`). They pass only on a real early SyntaxError; negatives for unimplemented syntax stay unsupported. The parser gained the strict-mode and statement-position early errors. Full test262: 33930.

### Array copy methods, assert.js globals, step limit (2026-11-09)

The Array copy-method dispatch matched the prefixed builtin name, so concat, flat, flatMap, with, toSorted, toReversed and toSpliced were all reported as unsupported. js_check now defines the assert.js globals (compareArray, isPrimitive, isNegativeZero) and raises the step limit to 50M; the runner's 10 s timeout is the guard. The engine default (10k steps) is a test guard and far too low for real pages.

Full test262: 35782/47708 pass (75.0%), up from 33930; no previously passing case lost. Largest remaining buckets: Temporal not defined (4105 failures, to come from a vetted crate), standard intrinsics not implemented (1793 unsupported: TypedArray/DataView, WeakMap/WeakSet, SharedArrayBuffer/Atomics, Proxy, FinalizationRegistry), Iterator helpers (438 + 133), modules (~780 unsupported), and 246 runner timeouts.

### Proxy and Reflect (2026-11-09)

Proxy exotic objects (design: PROXY.md): `Object::Proxy`, the `Proxy` constructor and `Proxy.revocable`, all thirteen traps with their §10.5 invariant checks, and proxy dispatch in the essential internal methods and their prototype walks. Callers that read ordinary slots now go through the internal methods: `Object.*`/`Reflect.*`, SetIntegrityLevel/TestIntegrityLevel, OrdinaryHasInstance, for-in, IsArray (JSON, concat, ArraySpeciesCreate, `Object.prototype.toString`), BoundFunctionCreate and GetFunctionRealm. Proxy target and handler are rooted for the duration of an internal method, because a trap may revoke its own proxy. Also fixed while there:
- `Reflect[Symbol.toStringTag]`.
- `instanceof` with a primitive left operand no longer reads `prototype` (§7.3.21 step 3).
- `with` object records resolve a name once (`Reference::WithBinding`) and re-check HasProperty in GetBindingValue/SetMutableBinding, so proxy environments see the spec's trap sequence.

Smoke: built-ins/Proxy passes everything except the 37 cross-realm cases (`$262.createRealm`); 2039 cases across with/assignment/Proxy/Reflect and the 382 cases that use `new Proxy` elsewhere lost nothing against the gap-9 baseline. Still missing: private elements on proxies (no storage for them on `Object::Proxy`), and the Annex B `__proto__`/`__defineGetter__`/`__lookupGetter__` family (not implemented at all, ~69 cases).

### Temporal, step 1: infrastructure and Temporal.Duration (2026-11-09)

Temporal comes from `temporal_rs =0.2.6` (audited by Rust Unsafe Security Auditor (#92), approved by Santiago), built by a second manifest `temporal/Cargo.toml` so the main crate stays dependency-free and `--offline` (design: TEMPORAL.md). Implemented: the `Temporal` namespace, `Temporal.Duration` (constructor, `from`, `compare`, getters, `with`, `negated`, `abs`, `add`, `subtract`, `round`, `total`, `toString`/`toJSON`/`toLocaleString`, `valueOf`), the options readers in spec order, and catch_unwind around every crate call. New CI step and run.py profile `temporal` (with `--binary temporal/target/release/js_check`): 421/4613 pass; Duration 373/540. Still missing in Duration: `relativeTo` (72 unsupported) and the cases that need PlainDate/ZonedDateTime (~90). Next: PlainDate, PlainTime, PlainDateTime.

### Temporal, step 2: Temporal.PlainDate (2026-11-09)

Implemented `Temporal.PlainDate` on `temporal_rs::PlainDate`: the constructor (ToIntegerWithTruncation on the ISO fields, calendar identifier canonicalised), `from` (ToTemporalDate with the property bag read in spec order: `day`, `era`, `eraYear`, `month`, `monthCode`, `year`, era fields only for calendars that have eras), `compare`, all 16 getters, `add`, `subtract`, `with`, `withCalendar`, `until`, `since` (largestUnit/smallestUnit/roundingIncrement/roundingMode, time units rejected), `equals`, `toString` (`calendarName`), `toJSON`, `toLocaleString`, and `valueOf` (TypeError). `Temporal.Duration` now accepts a PlainDate as `relativeTo`. Still unsupported: `toPlainYearMonth`, `toPlainMonthDay`, `toPlainDateTime` and `toZonedDateTime`, which wait for those types. Profile `temporal`: 1028/4613 pass (was 421). PlainDate is 561/654 and Duration is 426/540. Next: PlainTime and PlainDateTime.

### GC scheduling: threshold policy (2026-11-09)
- `Engine::new` still defaults to stress (a GC at every allocation) for the rooting tests. Embeddings (`js_check`, `module_check`, `servo_bridge`, `servo-native/mwr.rs`) select `GcPolicy::Threshold`: collect when live objects reach max(4096, 2 × live after the last GC). `js_check --gc-stress` restores stress.
- `Runtime::charged_bytes()` is O(1), metadata pruning runs only after a collection, and `Id` maps use a deterministic hasher.
- s1.js: 31.9 G → 0.375 G instructions. Several full16 runner timeouts now finish in under 1 s. Details and the benchmark table are in GC_SCHEDULING.md.

### Checkpoint 10: full test262 baseline (2026-11-09)
- Full profile with the Temporal binary (`gc-lab/temporal`): **37451/47708 (78.5%)**, +1669 vs the gap-9 baseline (35782), 0 lost. Failed 5733, unsupported 4524. Runner timeouts: 246 → 53, thanks to GC scheduling. The full run now takes about 10 min, down from about 25-30 min.
- Largest remaining buckets: Temporal 3585 non-pass (ZonedDateTime, PlainDateTime, PlainTime, PlainYearMonth, Instant are still mostly "not a constructor"), Iterator helpers 666, module code 599 + import 129, RegExp 483 (unpaired UTF-16 strings 446 across dirs), Atomics 389, WeakMap/WeakSet/WeakRef/FinalizationRegistry ~250, DataView/TypedArray ~450.

### Temporal complete, Number formatting, toLocaleString (2026-11-09)
- Temporal: PlainTime, PlainDateTime, Instant, ZonedDateTime, PlainYearMonth, PlainMonthDay and `Temporal.Now` are implemented on `temporal_rs` (TEMPORAL.md). The `built-ins/Temporal` smoke run passes 4613/4613. `intl402/Temporal` is not started.
- `Number.prototype.toFixed`, `toExponential` and `toPrecision` use exact digit generation with integer arithmetic (§21.1.3.2/.3/.5). `toString(radix)` uses shortest round-trip digits and is no longer unsupported. `toLocaleString` is ToString, since there is no ECMA-402.
- `Object.prototype.toLocaleString` (§20.1.3.5). `Array.prototype.toLocaleString` and `%TypedArray%.prototype.toLocaleString` (§23.1.3.32, §23.2.3.31) invoke each element's `toLocaleString`.

### Checkpoint 11: full test262 baseline (2026-11-09)
- Full profile with the Temporal binary: **41200/47708 (86.4%)**, +3749 vs checkpoint 10 (37451). Failed 2200, unsupported 4308.
- 4 lost, all with verdicts:
  - Two runner timeouts under load (`TypedArray/prototype/set/typedarray-arg-src-backed-by-resizable-buffer`, `identifiers/start-unicode-8.0.0-escaped`) pass alone in 1.5 s and 2.5 s.
  - Two `TypedArray/prototype/toLocaleString/this-is-not-*` cases had been false passes, because the method was undefined. They now pass through the real implementation, which was added in the fix pass after the run.
- Largest remaining buckets: Iterator helpers (442 "Iterator is not defined", 666 in the directory), module code 599 + import 129, RegExp 482 (unpaired UTF-16 strings 449), Atomics 389, DisposableStack/AsyncDisposableStack 187, WeakMap 141, ShadowRealm 57.

### Iterator helpers and joint iteration (2026-11-09)

- iterator_helpers.rs: the %Iterator% constructor (abstract, subclassable); Iterator.from with %WrapForValidIteratorPrototype%; Iterator.concat; Iterator.zip and Iterator.zipKeyed (shortest, longest with padding, and strict modes); the %Iterator.prototype% methods map, filter, take, drop, flatMap, reduce, toArray, forEach, some, every and find; chunks, windows and includes; join (stage 3); the `constructor` and @@toStringTag accessors (SetterThatIgnoresPrototypeProperties); and @@dispose. Spec: ECMA-262 §27.1.2-§27.1.4 and the joint-iteration, chunking, includes and join texts.
- Design: the spec builds each helper with CreateIteratorFromClosure. With no native coroutines in this interpreter, each closure is a resumable state machine in the object's `iter_slot` (state SuspendedStart, SuspendedYield, Executing or Completed, plus the closure's captures, which are traced). `return` on a suspended-yield helper replays the closure's handling of a return completion at its Yield: IfAbruptCloseIterator, flatMap's inner-then-outer close, IteratorCloseAll in reverse for zip, and no close after the final partial chunk or window (`? Yield`).
- Checkpoint 12: full19 = 41896/47708 (87.8%), +696 vs full18, no pass lost. built-ins/Iterator: 18 → 679/684. Remaining: 3 unsupported (cross-realm `proto-from-ctor-realm` and two `wellKnownIntrinsicObjects.js` cases); zip/zipKeyed `basic-longest` pass alone in 11-14 s, so they time out at 10 s on a loaded machine.
- Performance finding (roadmap B): `arr.push(k)` in a loop takes 2.7 s for 2500 elements and 15 s for 10000, which is superlinear. Plain index stores are slow as well (2.1 s for 5000). This probably drives a share of the runner timeouts.

### Small built-ins bundle (gap 13, 2026-11-09)

- `Array.fromAsync` (from_async.rs, §23.1.2.2 / the array-from-async text) is a native async state machine: a job object carries the iterator or array-like record, the phase (Next, Mapped, Value, Close) and the pending error, and every Await is `PromiseResolve` plus a reaction that resumes the job. Sync iterables go through an inline AsyncFromSyncIterator continuation, which closes the sync iterator when an awaited value rejects. A mapper throw goes through AsyncIteratorClose. Smoke: 95/95.
- `Promise.allKeyed` / `Promise.allSettledKeyed` (promise_combinators.rs): own enumerable keys from `[[OwnPropertyKeys]]`, and the result is a null-prototype object (CreateKeyedPromiseCombinatorResultObject). Smoke: 89/89. Also fixed: `PromiseResolve`'s IsPromise no longer reflects on a Proxy (§27.2.4.7).
- Annex B: the `Object.prototype.__proto__` accessor (§B.2.2.1), `__defineGetter__`, `__defineSetter__`, `__lookupGetter__` and `__lookupSetter__` (§B.2.2.2-5). %Object.prototype% is an immutable-prototype exotic object (§10.4.7).
- String: `isWellFormed` and `toWellFormed` are trivial, because strings are stored as UTF-8 and lone surrogates are unsupported (§22.1.3.10/.32). `toLocaleLowerCase` and `toLocaleUpperCase` are the root-locale case mappings. `localeCompare` compares UTF-16 code units. **Gap:** `String.prototype.normalize` (9 cases), and the canonical equivalence that `localeCompare` needs, require Unicode normalization data. Per the engine rules this is a vetted-crate proposal (audit by Rust Unsafe Security Auditor (#92), then Santiago's OK), not hand-written tables.
- `Map.prototype.getOrInsert` / `getOrInsertComputed` (upsert, step by step; the callback runs before the second lookup, and an entry it inserts is overwritten). Smoke: 31/33; the other 2 needed WeakMap (gap 14).
- Uint8Array base64/hex (base64.rs): `Uint8Array.fromBase64`/`fromHex` and `prototype.toBase64`/`toHex`/`setFromBase64`/`setFromHex`, with FromBase64 (all three lastChunkHandling modes, maxLength for set*), FromHex, DecodeBase64Chunk and SkipAsciiWhitespace transcribed from the spec. Smoke: 76/84; the other 8 need `$262.detachArrayBuffer` (6) or immutable ArrayBuffers (2).
- `Array.prototype[@@unscopables]` (§23.1.3.41).
- Not done: `Error.prototype.stack` (32 cases). It is the stage-2 error-stack-accessor proposal, and the engine keeps no stack traces yet, so it is deferred.
- Checkpoint 13: full20 = **42321/47708 (88.7%)**, +425 vs full19 (41896). Failed 1081, unsupported 4306. 2 lost, both with verdicts:
  - `language/identifiers/start-unicode-5.2.0.js` timed out under load; it passes alone in 5.6 s.
  - `Map/prototype/getOrInsert/does-not-have-mapdata-internal-slot-weakmap.js` was a false pass, because the method was undefined. It was honestly unsupported until WeakMap existed; it passes since gap 14.

### Weak references (gap 14, 2026-11-09)

- Design: WEAK_REFS.md. The runtime (`dual::Record`) now carries weak targets and `(key, value)` ephemerons next to its strong links. `Runtime::reachable` marks ephemerons indexed by key (a value waits in `waiting[key]` until its key is marked), so marking stays linear. After the sweep, weak entries with dead targets are dropped. `set_weak`/`add_weak` fail without mutation on the byte budget, and weak entries are charged as edges.
- Engine: WeakMap/WeakSet reuse the collection `Table` with `weak: true` (object keys are ephemerons; symbol keys, which never die, stay strong). WeakRef and FinalizationRegistry live in `script/weak.rs`. `prune_metadata` scrubs every weak holder after each collection: dead keys are removed, WeakRef targets and registry cells become empty, and registries with emptied cells are queued once for cleanup.
- `[[KeptAlive]]`: `new WeakRef` and `deref` root the target until ClearKeptObjects, which runs at the end of `microtask_checkpoint` (HTML) and in `release_globals`. CleanupFinalizationRegistry runs as a separate host task (`Engine::run_finalization_cleanup`), never as a microtask. js_check runs it after the checkpoint. **Servo bridge follow-up:** the bridge's task loop must call it (roadmap D).
- CanBeHeldWeakly: objects and non-registered symbols (symbols-as-weakmap-keys).
- WeakMap/WeakSet `set`/`add` of a new object key appends in place (O(1)), with no `replace_data` clone. 100k chained `WeakMap.set` calls take 17.8 s, about linear (1.5 s at 10k); the constant is interpreter cost (roadmap B).
- Smoke: built-ins/WeakMap, WeakSet, WeakRef, FinalizationRegistry 298/302. The 4 left are `proto-from-ctor-realm` (cross-realm). Of the 51 other files that use weak collections: 44 pass, 4 are unsupported (`$262.gc` / `createRealm`), and 3 fail (Error stack, ShadowRealm, and one staging deep-WeakMap case that times out and needs `$262.gc`).
- Tests: tests/weak_refs.rs (an ephemeron chain parked in reverse order, a value-to-key cycle dying, WeakRef clearing, FinalizationRegistry cleanup including a dead unregister token, and a zero-heap release).
- Literature check (Literature Review (#82), request #325): verdict "adopt". Its follow-ups are done: citations fixed (Felix's es-discuss scheme and SpiderMonkey `WeakMap.h`; V8 uses a fixpoint instead), cleanup re-reads the cells on every round (no snapshot), a registry whose callback throws is queued again, and there are tests for each key-marking route plus a random-heap comparison with the naive fixpoint. The `taskygpt` second opinion was not available (HTTP 429, no credits).

### Checkpoint 14: full test262 baseline (2026-11-09)

- 42656/47708 pass (89.4%), +335 vs checkpoint 13 (42321). Lost 1: `TypedArray/prototype/set/typedarray-arg-src-backed-by-resizable-buffer.js`, a load timeout (passes alone in 5 s).
- Largest remaining buckets: Atomics 389 (126 "not defined"), DisposableStack/AsyncDisposableStack 187 + SuppressedError 18 + 62 negative parse (explicit-resource-management), module code 599 + import 129, RegExp 482 (unpaired UTF-16 strings 447), ShadowRealm 57-64, runner timeouts 60.

### Explicit resource management (gap 15, 2026-11-09)

- Design: RESOURCE_MANAGEMENT.md. Built-ins in `script/disposal.rs`: DisposableStack, AsyncDisposableStack, SuppressedError, `%AsyncIteratorPrototype%[@@asyncDispose]`, with DisposeCapability / AddDisposableResource / DisposeResources following the spec records.
- Syntax: `using` and `await using` declarations are their own declaration kinds (never lowered to `const`), with the early errors (script and eval top level, case clauses, `for (using x in ...)`, `await using` outside async code, the name `let`, binding patterns, missing initialiser). Both the tree-walker (`Engine::using_scope`) and the VM (`DisposeStart` / `DisposeSync` / `DisposeAsync` inside a `Try{finally}`) dispose at scope exit; for-of disposes each iteration before IteratorClose.
- Known deviation: async disposal awaits one promise from `dispose_async` and not one inline Await per resource, so microtask tick counts can differ from the spec.
- Also fixed: sloppy `let = 1` as an expression statement, `x[]` as a real SyntaxError, and NamedEvaluation for VM declaration initialisers (`Op::EvalNamed`).
- Smoke: language/statements/{using,await-using} 165/178 (0 failed; 10 module cases and 3 parser gaps). for-of, for-await-of, block and the three built-in dirs: 2143/2225, the 7 failures already failed before.
- Open parser gaps found here: `await` as a binding identifier in scripts (`var await`), and `var let` in sloppy mode (blocks the `*-declaring-let-split-across-two-lines` cases).

### Checkpoint 15: full test262 baseline (2026-11-09)

- 43049/47708 pass (90.2%), +393 vs checkpoint 14 (42656). Failed 856 (-219), unsupported 3803 (-174). Lost 2: `language/identifiers/start-unicode-5.2.0.js` and `start-unicode-5.2.0-escaped.js`, both load timeouts (each passes alone in 3.7 s).
- Largest remaining buckets: Atomics 389 (+ SharedArrayBuffer 104), module code 599 + import 129 + dynamic-import negative parse ~375, "unsupported reserved expression" 596, RegExp 482 (unpaired UTF-16 strings 442), "reserved binding identifier" 115 (includes `await` as an identifier in scripts), ShadowRealm 57-64, runner timeouts 67.

### SharedArrayBuffer and Atomics (gap 16, 2026-11-09)

- Design: SHARED_MEMORY.md. A SharedArrayBuffer is a `Buffer` with `shared: true` (constructor, byteLength, growable, maxByteLength, grow, slice, @@species). ArrayBuffer and SharedArrayBuffer methods reject receivers of the other kind.
- Atomics (`script/atomics.rs`): load, store, compareExchange, add, sub, and, or, xor, exchange, isLockFree, pause, wait, waitAsync and notify, following ValidateIntegerTypedArray / ValidateAtomicAccess / RevalidateAtomicAccess. There is a single agent, so every access is sequentially consistent.
- [[CanBlock]] is false by default, so a synchronous `Atomics.wait` throws a TypeError. `js_check --can-block` sets it, and run.py maps `CanBlockIsTrue` to that flag.
- waitAsync timers and notify resolutions are host jobs, drained by the embedder through `next_host_job()` / `run_host_jobs()`. js_check does this; **the Servo bridge must too** (roadmap D).
- Harness-only `$262` (detachArrayBuffer, gc, global, agent.sleep/monotonicNow; createRealm, evalScript and agent.start are Unsupported) and `setTimeout`.
- Fixed: the ArrayBuffer and SharedArrayBuffer constructors now get `prototype` from newTarget before the host allocation-limit RangeError.
- Open: multi-agent tests (109 Atomics cases, unsupported) need a thread-safe byte store and Worker agents. Immutable ArrayBuffer (`immutable`, `sliceToImmutable`, about 58 cases) is not implemented.

### Checkpoint 16: full test262 baseline (2026-11-09)

- 43818/47708 pass (91.8%), +769 vs checkpoint 15 (43049). Failed 708 (-148), unsupported 3182 (-621). Lost 0.
- Largest remaining buckets: "unsupported reserved expression" 596, module code 599 + import 129 + dynamic-import negative parse ~375, unpaired UTF-16 strings 442, createRealm 181, "reserved binding identifier" 115, test262 agents 109, runner timeouts 66, ShadowRealm 57.
