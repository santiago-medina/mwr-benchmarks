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

`catch` now supports omitted bindings and object/array binding patterns through the shared initializer. Catch parameter and body lexical environments are separate, and exceptions during destructuring become pending completions so `finally` can execute. Resumable catch scopes use the same environment split; suspension inside catch patterns is lowered since checkpoint 45. Corrected a stale parameter-record field in the parser left by the earlier binding-pattern refactor. Formatting only; no compilation or tests were run.

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
- `Error.prototype.stack` (error-stack-accessor proposal): a get/set accessor on %Error.prototype%; the getter returns an implementation-defined string (no stack traces are kept), the setter is SetterThatIgnoresPrototypeProperties (checkpoint 29).
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
- Open: multi-agent tests (109 Atomics cases, unsupported) need a thread-safe byte store and Worker agents. Immutable ArrayBuffer is implemented (checkpoint 29).

### Checkpoint 16: full test262 baseline (2026-11-09)

- 43818/47708 pass (91.8%), +769 vs checkpoint 15 (43049). Failed 708 (-148), unsupported 3182 (-621). Lost 0.
- Largest remaining buckets: "unsupported reserved expression" 596, module code 599 + import 129 + dynamic-import negative parse ~375, unpaired UTF-16 strings 442, createRealm 181, "reserved binding identifier" 115, test262 agents 109, runner timeouts 66, ShadowRealm 57.

### Modules: spec records, `import()` and `import.meta` (gap 17, 2026-11-09/10)

- Design: MODULE_RECORDS.md. `modules.rs` follows the Cyclic Module Record algorithms (16.2.1.5-16.2.1.6): LoadRequestedModules, Link (InnerModuleLinking over SCCs), Evaluate with `[[AsyncEvaluation]]` ordering for top-level await, ResolveExport with `export *` and ambiguity, module namespace exotic objects, and string export names.
- Pull host API: `take_module_requests` / `finish_module_request`, with referrers `Script { host_defined }`, `Module` and `Realm`. js_check answers requests from files relative to the referrer; run.py passes `--module` and `--negative-resolution`.
- `import()` (13.3.10) in both evaluators and the resumable VM: the promise is rejected on an abrupt ToString, requests for the same (referrer, specifier) are shared, and link/evaluate/namespace run as promise jobs. `import.meta` (13.3.12) is a lazy null-prototype object, one per module record. Fixed: AsyncModuleExecutionRejected now rejects the TopLevelCapability before the async parents.
- Still unsupported (parse error "unsupported ...", never parsed as something close): import attributes (`with {...}` and the second `import()` argument, 60 cases), import phases `import.source` / `import.defer` / `import defer` / `import source` (252 + 242 negative parse cases), `$262.AbstractModuleSource`.
- **Servo bridge (roadmap D):** `mwr.rs` still hands over the whole static graph up front. It must drain `take_module_requests`, or `import()` on a page stays pending forever.

### Checkpoint 17: full test262 baseline (2026-11-10)

- 44976/47708 pass (94.3%), +1158 vs checkpoint 16 (43818). Failed 713 (+5), unsupported 2019 (-1163). Lost 0. There are 12 new failures, from cases that were unsupported before and now run: 8 `built-ins/AbstractModuleSource` (source phase imports not implemented) and 4 `ShadowRealm.prototype.importValue` (ShadowRealm not defined).
- Largest remaining buckets: unpaired UTF-16 strings 447, import phase 252 + 242 negative parse, createRealm 181, "unsupported or missing expression" 158, "reserved binding identifier" 115 (`await` as an identifier in scripts), test262 agents 109, import attributes 60, runner timeouts 60, ShadowRealm 57, await expression shape 44, resumable loop binding expressions 37.

### Parser leftovers: contextual keywords, optional chaining, `new new X` (gap 18, 2026-11-10)

- `yield`, `await` and `let` are contextual (13.1.1): the parser tracks the [Yield]/[Await] parameters (`yield_kw`, `await_kw`) per function, arrow (ConciseBody is [~Yield]; async arrows are [+Await]), class static block ([~Yield, +Await]) and module. `yield` is an identifier only in sloppy code outside generators, `await` only outside async functions, modules and static blocks, `let` only in sloppy code and never as a lexically bound name (14.3.1.1, 14.7.5.1). In a for head, sloppy `let` that does not start a binding (`for (let in o)`, `for (let;;)`) is an identifier expression, and `for (let of x)` is a SyntaxError (14.7.5 lookahead).
- Optional chaining (13.3.9): `?.` token (no DecimalDigit lookahead, 12.8), `Expr::OptionalChain` boundary plus `Expr::Optional` links, short-circuit through a non-terminal `ScriptError::ShortCircuit` caught at the boundary. Calls keep `this`, `delete a?.b` is true on a nullish base, private names work (`o?.#x`). Early errors: tagged template in a chain, assignment/update target, `new a?.b()`.
- `new new X()` (13.3.5): the inner `new MemberExpression Arguments` binds first.
- Tokens that can never start an expression (`)`, `;`, binary operators, end of input) now give "unexpected token", a real early SyntaxError, instead of "unsupported or missing expression".
- Still unsupported: `await` inside some async shapes the resumable compiler does not lower ("await expression shape"), `with` in generators/async functions.

### Checkpoint 18: full test262 baseline (2026-11-10)

- 45309/47708 pass (95.0%), +333 vs checkpoint 17 (44976). Failed 714 (+1), unsupported 1685 (-334). The run lost 6: `class let {}` / `class yield {}` in sloppy scripts parsed. Fixed after the run (a class's BindingIdentifier is strict mode code, §11.2.2); the six pass again, and the recorded summary is the run as it was (45309).
- Largest remaining buckets: unpaired UTF-16 strings 452, import phases 252 + 242 negative parse, createRealm 181, test262 agents 109, import attributes 62, ShadowRealm 57, runner timeouts 55, await expression shape 46, "unsupported character" 70, "expected punctuation" 38, call depth limit 34 (tco-* cases).

### Gap 19: import phases (checkpoint 19, 2026-11-10)

- `import source`, `import.source()`, %AbstractModuleSource% (`$262.AbstractModuleSource`), `import defer`, `import.defer()` and deferred namespaces, from the Stage 3 proposals. Design: IMPORT_PHASES.md. `source-phase-imports`, `source-phase-imports-module-source` and `import-defer` are out of UNIMPLEMENTED_SYNTAX. Feature cases: source 253/253, defer 250/252 (2 also use import attributes).
- Side fixes: super property assignment targets (`super.x = v`, `super[k] op= v`, `++super.x`; PutValue throws on a refused [[Set]] only in strict code, §6.2.5.6), PrivateFieldAdd/PrivateMethodOrAccessorAdd on non-extensible objects throw TypeError (proxies exempt).
- Host API: `ModuleRequest.phase` and `ModuleLoad::ModuleSource { url, class_name, host_defined }` (additive).
- Full run: 45832/47708 pass (96.1%), +523 vs checkpoint 18. Failed 715 (+1), unsupported 1161 (-524). Lost 3, all runner timeouts under load (TypedArray set resizable-buffer, encodeURI/encodeURIComponent A2.5_T1); each passes alone in under 5 s.
- Largest remaining buckets: unpaired UTF-16 strings 442, createRealm 181, test262 agents 109, import attributes 64 + 19 strict + 15 negative parse, ShadowRealm 57 + 64 non-pass in its dir, runner timeouts 68, await expression shape 46, "unsupported character" 70, call depth limit 34.

### Gap 20: ill-formed UTF-16 strings + in-place property writes (checkpoint 20, 2026-11-10)

- Strings are `Utf8` or `Units` (ill-formed UTF-16). RegExp runs over code units, and lone surrogates flow through String methods, JSON, URI and keys. Design: STRINGS.md.
- Own properties: `PropertyMap` (insertion order + hash index above 8 keys), in-place [[DefineOwnProperty]] / delete / ArraySetLength. Writes and deletes are linear now (push x8000 8554 → 54-82 ms). Design and numbers: PROPERTIES.md.
- Full run: 45897/47708 pass (96.2%), +65 vs checkpoint 19. Failed 1104 (+389), unsupported 707 (-454). Lost 0. The new failures are mostly runner timeouts (457) of RegExp cases that were unsupported before: interpreter throughput.
- After the run: exact lone-surrogate handling in decodeURI/decodeURIComponent and parseInt/parseFloat (20 cases out of the 29 left).
- Largest remaining buckets: runner timeouts 457 (roadmap B trigger), createRealm 181, test262 agents 109, import attributes 64 + 19 strict, ShadowRealm 57, await expression shape 46, "unsupported character" 44 + 26, call depth limit 34.
- Next gap: roadmap B, the bytecode VM design note (resolved binding slots, dense elements, shapes).

### Roadmap B stage 1: incidental interpreter costs (checkpoint 21, 2026-11-10)

- Design note: BYTECODE_VM.md (staged plan: incidental costs, slot environments, register bytecode, shapes). Fixed benchmark set in `bench/vm/`.
- Stage 1: `Rc` closure bodies (no AST clone per call), lazy `arguments` (§10.2.11 argumentsObjectNeeded), one shared `for (let ...)` environment when nothing can capture the bindings (§14.7.4.4). Both predicates are conservative AST scans (`ast_scan.rs`).
- Callgrind: fib(16) 256.9 M → 114.8 M instructions (2.24x); 20k-iteration let loop 1,192 M → 870 M (1.37x).
- Full run: 45919/47708 pass (96.3%), +22 vs checkpoint 20 (the decodeURI/parseInt fixes made after that run). Failed 1102 (-2), unsupported 687 (-20). Lost 0. Runner timeouts 455 (-2).
- Next: stage 2, scope analysis with slot-indexed environments (BYTECODE_VM.md); the element store (44% of the loop) waits for stage 4.

### Roadmap B stage 2, part 1: hot-path costs (checkpoint 22, 2026-11-10)

- OrdinarySet fast path (`plain_set`, §10.1.9.2 with Receiver = O on all-ordinary chains), an FxHash-style seeded key hasher for property maps and environments (HashDoS trade-off in BYTECODE_VM.md), no block environment for blocks without lexical declarations (§14.2.2), and one object fetch per scope level in identifier lookup.
- Callgrind: 20k-iteration loop 869.0 M → 470.1 M instructions (1.85x), fib(16) 114.8 M → 111.0 M. Stage-2 kill line (≤ 15k per iteration) not met yet: 23.5k.
- Full run: 45919/47708 pass (96.3%), +0. Lost 0. Runner timeouts 455 (unchanged).
- Next: the per-iteration allocations (index keys, binding updates, ArraySetLength truncation), then the resolver with slot environments.

### Roadmap B stage 2, round 2: allocations and the reference walk (checkpoint 23, 2026-11-10)

- InitializeBinding in place, OrdinaryGet read in place on plain chains, ArraySetLength step 19 as one pass for long ranges, CreateListFromArrayLike with a reused key buffer, one-walk identifier resolution (`Engine::resolve_name`), borrowed reference names, and an integer fast path for Number::toString (§6.1.6.1.20).
- Callgrind: 20k-iteration loop 470.1 M → 319.1 M instructions (16.0k per iteration, kill line ≤ 15k, round 2 of 3), fib(16) 111.0 M → 105.6 M.
- Full run: 45918/47708 pass (96.2%), -1. Lost 1: built-ins/Function/prototype/toString/built-in-function-object.js timed out under foreign load (load average ~9.5) and passes alone in 2 s, so it is counted as a load timeout, not a regression. Runner timeouts 456.
- Next: round 3 (plain_set, lookup_with_base, initialize link/unlink, malloc/free), then the resolver with slot environments.

### Roadmap B stage 2, round 3: clones and calls on the store path (checkpoint 24, 2026-11-10)

- `has_direct_using` walks block statements without collecting a Vec, ToPrimitive §7.1.1 step 1 is an inline check, `assign_member` hands its key to OrdinarySet by value, and the `PropertyMap` index maps seeded key hashes to entry positions (no key clone per new property). BYTECODE_VM.md "Stage 2, round 3".
- Callgrind: 20k-iteration loop 319.1 M → 294.4 M instructions (14.7k per iteration: the stage-2 kill line ≤ 15k is met), fib(16) 105.6 M → 103.4 M.
- Full run: 46067/47708 pass (96.6%), +149. Lost 0. Runner timeouts 456 → 307 (the gains are slow cases now finishing under 10 s).
- Next: stage 3 (register bytecode, with the slot-environment resolver first) waits on the literature check (collab #336) and a second opinion; meanwhile roadmap A buckets.

### Import attributes and JSON modules (checkpoint 25, 2026-11-10)

- WithClause on static imports and exports (§16.2.2, duplicate keys are early errors), `import(spec, options)` (§13.3.10.2 step 7), ModuleRequest Records keyed by specifier plus sorted attributes (§16.2.1.3), AllImportAttributesSupported with « "type" », and JSON modules (ParseJSONModule, a default-export synthetic record). Design: JSON_MODULES.md. `import-attributes` is out of UNIMPLEMENTED_SYNTAX.
- Bucket: 89/100; the other 11 are import-text / import-bytes, Unsupported by design.
- Full run: 46177/47708 pass (96.8%), +110. Failed 933 (-21), unsupported 598 (-89). Lost 90, all RegExp property-escapes/generated runner timeouts under load (load average ~9.6); each passes alone in 4.3 s on average, 7.3 s at most, so they are counted as load timeouts.
- Next: realms (createRealm 181, ShadowRealm 64; needs a design note) or test262 agents (Atomics, 109).

### Realms: `$262.createRealm`, `$262.evalScript`, `Error.isError` (checkpoint 26, 2026-11-10)

- Realm Records (§9.3) in one Engine: the running realm's intrinsics and global environment stay in `intrinsics`/`global`, others wait in `realms` slots; objects allocated outside realm 0 are tagged in an allocation-time side table. Design and options: REALMS.md.
- Execution-context realm switch for built-ins and ECMAScript functions (§10.2.1.1, §10.3.1); proxies and bound functions push no context. GetFunctionRealm (§7.3.24), GetPrototypeFromConstructor's cross-realm fallback (§10.1.14) in every constructor (including CreateDynamicFunction), ArraySpeciesCreate step 5 (§10.4.2.3), per-realm [[TemplateMap]] (§13.2.8.4). [[Construct]] steps 10-12 and a non-constructor `new` throw in the caller's realm.
- `$262.evalScript` runs ParseScript + GlobalDeclarationInstantiation (§16.1.7, with CanDeclareGlobalFunction/Var and the lexical clash checks) + ScriptEvaluation and returns the completion value. Unimplemented syntax stays Unsupported.
- `Error.isError` (§20.5.2.1): 12/12 in its directory.
- Bucket: 194/194 (181 createRealm + 13 evalScript). Fix rounds: 3 of 3.
- Full run: 46330/47708 pass (97.1%), +153. Failed 974 (+41), unsupported 404 (-194). Lost 118, all runner timeouts in RegExp property-escapes / CharacterClassEscapes under foreign load (load average ~8); all 118 pass alone (6.1 s on average). They are counted as load timeouts.
- Not done: ShadowRealm (Stage 2.7 proposal, 57 cases), per-realm release (a roadmap C/D item), the embedding RealmId API (roadmap C note).

### String hardening and string performance (checkpoint 27, 2026-11-10)

- Follow-up of literature check #335 (STRINGS.md, last section): `debug_assert!` of the canonical-form invariant in the only `Repr::Units` constructor; a property test over surrogate-biased code-unit sequences (equality, hashing and `HashSet` agreement through `concat`, `Builder` and ropes); a JS test for U+FFFF > U+10000 (§7.2.13) and lone-surrogate Map/Set keys.
- Fix found by that test: the default SortCompare of `Array.prototype.sort` (§23.1.3.30.2) now compares code units of `ToString` results, so arrays holding lone surrogates sort, where before they reported Unsupported.
- Perf: `Utf8` strings cache their code-unit length and code units (O(1) `length`, `charCodeAt`, `s[i]`; `String.prototype` methods no longer copy the whole string). Concatenations of 256 bytes or more are lazy ropes, flattened once with no recursion. Benchmarks (release, `vm-bench/run.sh`): `+=` to 1M units 111.75 s → 3.58 s, `charCodeAt` over 100K non-ASCII units 69.39 s → 0.83 s, `strings` 2.19 s → 0.24 s, other cases unchanged or faster. Numbers in STRINGS.md.
- Full run: 46310/47708 pass (97.1%), -20 against full33. Failed 994 (+20), unsupported 404 (0). Gained 78 and lost 98, all on both sides RegExp property-escapes runner timeouts under foreign load (load average 7-10). All 98 lost pass alone (4.5 s on average, 7.6 s at most), so they are load timeouts, not regressions. Property-escapes throughput is regex-interpreter bound, not string bound: roadmap B.

### Hashbang, restricted function properties, `Math.sumPrecise`, `String.prototype.normalize` (checkpoint 28, 2026-11-10)

- Hashbang comments (§12.5): `#!` is a comment only at offset 0 of a Script or Module source (eval's Script too). Any other `#` that does not start a PrivateIdentifier is an early SyntaxError ("Invalid or unexpected token"); `#\` (escaped private names) stays Unsupported. `js_check` runs a case that opens with `#!` as its own Script after the harness globals. 29/29 in `language/comments/hashbang`.
- `%ThrowTypeError%` (§10.2.4.1): `length` 0 and `name` "" non-configurable, not extensible. `Function.prototype` gets the `caller`/`arguments` accessors from AddRestrictedFunctionProperties (§10.2.4). Non-strict ordinary functions get own read-only `caller`/`arguments` = undefined (§17.1 allows them; caller tracking is not implemented), so sloppy `f.caller` does not throw and strict functions inherit the thrower.
- `Math.sumPrecise` (ES2026): exact sum in a 36-limb two's complement fixed-point accumulator (unit 2^-1074), one round-half-even at the end; the -0/±Infinity/NaN states follow the spec. 10/10.
- `String.prototype.normalize` (§22.1.3.15) with UAX #15 NFC/NFD/NFKC/NFKD. The tables in `normalization_tables.rs` are generated by `gen_unicode_norm.py` from UCD 17.0.0 (UnicodeData.txt, DerivedNormalizationProps.txt); Hangul is algorithmic. Every line of NormalizationTest.txt 17.0.0 passes (unit test, `NORMALIZATION_TEST=<path>`). `localeCompare` compares the NFC forms, so canonically equivalent strings give 0 (§22.1.3.12 step 5). 27/27 across normalize and localeCompare.
- Full run (before the sloppy `caller` fix): 46367/47708 pass (97.2%), +57 against full34. Failed 992 (-2), unsupported 349 (-55). Lost 109: 23 `caller` cases (fixed in this checkpoint's fix pass: 1554/1559 in the Function, arguments-object and function directories, no new failure against full34) and 86 RegExp property-escapes runner timeouts under foreign load.

### JSON.parse source text, `Error.prototype.stack`, immutable ArrayBuffer (checkpoint 29, 2026-11-10)

- json-parse-with-source: `JSON.rawJSON`, `JSON.isRawJSON`, and the reviver's third `context` argument with `source` for primitive values, from JSON Parse Records built during the parse (`[[IsRawJSON]]` objects are serialized verbatim by SerializeJSONProperty). Two JSON.stringify fixes on the way: a Number object in a replacer array goes through ToString, and step 10's IsArray sees through proxies. 516/516 over built-ins/JSON, staging/sm/JSON and built-ins/Proxy.
- error-stack-accessor: see the Error entry above. 35/35 own cases.
- Immutable ArrayBuffer (proposal spec, §25.1 and §23.2 changes): `ArrayBuffer.prototype.immutable`, `transferToImmutable`, `sliceToImmutable`. An immutable buffer cannot be detached (`DetachArrayBuffer` and ArrayBufferCopyAndDetach throw TypeError) or resized. Writes go through ValidateTypedArray(…, write) / TypedArrayCreateFromConstructor(…, write) / TypedArraySpeciesCreate(…, write), so `set`, `fill`, `copyWithin`, `reverse`, `sort`, `slice`, `map`, `filter`, `from`, `of`, `setFromBase64`/`setFromHex` and Atomics `store`/`compareExchange`/RMW throw TypeError; DataView setters throw before ToIndex. Typed-array exotic objects over an immutable buffer report non-writable, non-configurable elements, [[Set]] returns false, and [[DefineOwnProperty]] runs ValidateAndApplyPropertyDescriptor against the current element. Smoke over 6355 ArrayBuffer/DataView/TypedArray/Atomics/Uint8Array/freeze cases: 0 regressions against full35. One case left: `sliceToImmutable/argument-coercion.js` passes alone in 31.6 s (a 42x42 loop of `Array.from(new Uint8Array(...))` + compareArray; each iterator-protocol step costs about 20-40 µs, linear in n). It is a roadmap B timeout, not a feature bug.
- Full run: 46501/47708 pass (97.5%), +134 against full35. Failed 862 (-130), unsupported 345 (-4). Lost 35, all RegExp property-escapes runner timeouts under load (each passes alone in under 7.3 s). 413 of the 428 timeouts are in `built-ins/RegExp/property-escapes`, which makes their cost the next gap (roadmap B).

### Strict early errors, ShadowRealm, proper tail calls (checkpoint 30, 2026-11-10)

- Strict early errors (§13.10, §14.7.4, §12.9.3, Annex B.3.5): the [In] parameter in `for` initializers, `#x in` only as a RelationalExpression left operand, arrows only at the start of an AssignmentExpression, parenthesized literals and a rest element with a trailing comma rejected as destructuring targets, `new.target` in scripts and global eval, `3in []`, `for (async of`, for-in heads that are assignments, and Annex B `for (var a = b in c)`. 30/30 target cases, 0 regressions over 8663 neighbours.
- ShadowRealm (proposal, Stage 2.7): constructor, `evaluate` (PerformShadowRealmEval), wrapped function exotic objects (GetWrappedValue, WrappedFunctionCreate, CopyNameAndLength). See REALMS.md. built-ins/ShadowRealm 58/64; the 6 `importValue` cases stay Unsupported (needs HostLoadImportedModule without a referrer).
- Proper tail calls (§15.10): a trampoline in `invoke_this` runs calls in tail position of strict ordinary bodies at constant call depth. See TAIL_CALLS.md for the positions kept as ordinary calls (for-in/of bodies, `using` blocks, optional calls). 35/35 `tail-call-optimization` cases (was 0), 0 regressions over 4049 neighbours.
- Runner: `run.py --retry-timeouts W` reruns runner timeouts afterwards with W workers and the same 10 s budget. In full37, 439 cases timed out at 10 workers and 379 of them passed on the 2-worker retry. The other 60 still time out (54 RegExp property-escapes). That is load noise removed, not a larger budget: the property-escapes cost stays the roadmap B gap.
- Full run: 47011/47708 pass (98.5%), +510 against full36 (about 144 from the features above, 366 from the timeout retry). Failed 351 (-511), unsupported 346 (+1). Lost 0.

### Private name scoping, `var` PutValue, generic Array methods on exotic receivers (checkpoint 31, 2026-11-10)

- Private names (§15.7.14 step 7, §19.2.1.1, §19.2.1.2, §10.2.3): one PrivateEnvironment per class evaluation, with lookup through the enclosing private environments. Direct eval sees the private names in scope, a field-initializer eval rejects `arguments`, and class constructors define `length` before `name`. Smoke 11824/11896, +69 against full37.
- `var x = v` (§14.3.2.1) resolves the binding first, then evaluates the initializer, then runs PutValue. This fixes the `with` cases where the initializer changes the object environment. Annex B.3.4 (a `var` that redeclares a catch parameter) stays Unsupported.
- Generic Array methods walk TypedArray, Proxy and module namespace receivers with HasProperty per index. Before, they skipped every TypedArray element. Smoke over Array.prototype, TypedArray and Proxy: 5283/5314, +27.
- Small fixes: ArraySpeciesCreate takes a u64 length and throws RangeError above 2^32-1 (slice, splice, map, and Array.from's ArrayCreate). ToIntegerOrInfinity maps -0 to +0. sort and toSorted check comparefn before ToObject. The lengths of includes and lastIndexOf are 1, and flat's is 0.
- Full run: 47196/47708 pass (98.9%), +185 against full37. Failed 175 (-176), unsupported 337 (-9). Gained 198, lost 13: all RegExp property-escapes runner timeouts under foreign load (load average ~10). All 13 pass alone (5.3 s on average, 10.9 s at most), so they count as load timeouts. 23 runner timeouts are left after the retry. Next: stage 3 of BYTECODE_VM.md (register VM).

### Register VM stage 3: try/catch/finally, VM default in js_check (checkpoint 32, 2026-11-10)

- Stage 3 of BYTECODE_VM.md, coverage steps 1-2: the register VM compiles expressions, control flow, calls and `try`/`catch`/`finally` (handler side table, innermost first; a completion-kind register for `finally`, `EndFinally`; `return` forwarded through nested `finally` blocks; §15.10.2 tail positions inside `try`). Function bodies that use something the VM does not compile fall back to the tree-walker per function. Fallbacks seen over 600 cases: `arguments`/direct eval 214, closures over locals 73, for-in/of 60, non-simple parameters 33, templates 33, switch 16; 89.7% of function bodies compile.
- Logical assignment (`&&=`, `||=`, `??=`) applies NamedEvaluation to anonymous functions (§13.15.2): +9 cases.
- `js_check` defaults to the VM (`--engine=tree` or `MWR_ENGINE=tree` selects the tree-walker). The embedding default (`EngineMode`) stays Tree. Equivalence: tree and VM give identical results on a 2500-case sample and on `statements/try`.
- pe-bench (10 fixed RegExp property-escape cases, run alone): tree 37.1 s, VM 10.3 s (3.6x).
- Full run on the VM: 47223/47708 pass (99.0%), +27 against full38. Failed 148 (-27), unsupported 337. Gained 27 (17 property-escape timeouts, 9 logical-assignment cases, 1 other), lost 0. The full run took ~10 min instead of ~39. Five timeouts remain on both engines: TypedArray copyWithin coerced-values-*-detached (3) and decodeURI/decodeURIComponent S15.1.3.x_A2.5_T1 (2); top-level script code still runs on the tree-walker.

### Structural AST walker for the compile-time scans (checkpoint 42, 2026-11-11)

- `ast_scan.rs` uses an exhaustive structural walker (`Walk`, `Node`, `Flow`) instead of searching the derived `Debug` text. The direct-eval, argumentsObjectNeeded, captured-name, for-let capture, elision, operand-reuse and switch-TDZ checks are node predicates (OBJECT_MODEL.md, "Structural AST walker").
- Bug fixed: operand reuse missed writes made by destructuring assignment, so `x + ([x] = [5], 0)` gave 5 on the VM instead of 1 (§13.15.5). New probe `vmprobe/operands.js`.
- Callgrind: `vm-bench/decode_tiny.js` 271.4M to 256.0M instructions (-5.7%). A differential build found no disagreement with the old scans over 7951 test262 files.

### Object model slice 3: global-name slot hints (checkpoint 41, 2026-11-11)

- Slice 3 (part a) of `OBJECT_MODEL.md`: each compiled chunk keeps one hint per name, the entry index of the name in the global object's `PropertyMap` (`script/global_names.rs`). The environment walk of GetIdentifierReference (§9.1.2.1) still runs on every access; the hint only replaces the global object record's HasBinding / GetBindingValue / SetMutableBinding (§9.1.1.4) for an ordinary global with an own data property (writable for sets). Hints are validated by key on every use, so no invalidation epoch is needed; accessors and read-only properties mark the hint failed. Interned keys (JsAtom, `Key::Index`) are deferred.
- Fix found by the new probes: an elided strict function called from sloppy code took strictness from the closure environment, so a write to a read-only global was silently ignored instead of throwing TypeError (PutValue, §6.2.5.6, uses the Reference's [[Strict]]). Assignment strictness now comes from the chunk.
- Callgrind: `vm-bench/decode_tiny.js` 339.2M to 271.4M instructions (-20.0%); slices 1-3 together -35.9% (kill line of 30% met). Next measured cost: the Debug-formatting `ast_scan::mentions` compile-time scans (~22M, ~8%).
- Full run on the VM (full48): 47226/47708 (99.0%), +0, lost 0; the five shared timeouts remain.
- Gate: cargo test 187/0; vmprobes (new `globals.js`, 9 invalidation probes) tree/vm/gc-stress; 2110 cases from identifier-resolution, global-code, with, eval-code, assignment, compound-assignment, postfix-increment, typeof, built-ins/global, call, return, function-code and variable identical on both engines; the 2500-case equivalence is identical.

### Object model slice 2: environment elision for compiled calls (checkpoint 40, 2026-11-10)

- Slice 2 of `OBJECT_MODEL.md`: a compiled function body whose parameters are simple and unique, that captures nothing, uses no `arguments`, direct eval, `this`, `new.target`, `super`, nested functions, classes or `with`, and has no hatched expression, runs without a function environment record of its own (FunctionDeclarationInstantiation, §10.2.11, has no observable effect for it). Parameters live only in registers and free names resolve through the closure's environment. Strictness comes from the body. The sloppy `this` coercion (OrdinaryCallBindThis, §10.2.1.2) is skipped because `this` is never read. Tail calls (§15.10.1) keep working in strict elided bodies.
- Callgrind: `vm-bench/decode_tiny.js` 421.9M to 339.2M instructions (-19.6%), 4162 of 4163 calls elided. Slices 1+2 together: -19.9%; slice 3 (global-name cache, interned keys) has to bring the rest of the 30% kill line.
- Full run on the VM (full47): 47226/47708 (99.0%), +0, lost 0. Gate: cargo test 187/0; vmprobes (new `elide.js`) tree/vm/gc-stress; 1650 cases from arguments-object, call, return, eval-code, function-code and function declarations/expressions identical on both engines; the 2500-case equivalence is identical. The five shared timeouts remain (decodeURI S15.1.3.1_A2.5_T1 now 17.7 s alone with no limit).

### Object model slice 4: dense elements (checkpoint 43, 2026-11-11)

- `PropertyMap` keeps array-index properties in a dense `elements` Vec, which holds full properties of any attributes. Its density bound is `2 * present + 16`, and a map that goes sparse stays sparse. `script/elements.rs` adds exact fast paths that address the slot by `u32` index: OrdinaryGet, CreateDataProperty, OrdinarySet and the Array LengthOfArrayLike. The VM `GetElem`/`SetElem`, `Array.from`, spread and the array iterator use them. `iterator_step_value` (IteratorStepValue, §7.4.8) runs `%ArrayIteratorPrototype%.next` (§23.1.5.1) directly, without allocating the result object, for for-of, destructuring, spread and `Array.from`. Details and the deviations from the design are in `OBJECT_MODEL.md`, under "Slice 4".
- Callgrind: `vm-bench/copywithin_small.js` drops from 5,094.4M to 2,530.8M instructions (-50.3%; the kill line was -10%). decode_tiny goes from 256.0M to 257.4M (+0.6%).
- Full run on the VM (full50): 47229/47708 (99.0%), +3, lost 0. The gained cases are the three copyWithin `coerced-values-*-detached` timeouts. The only timeouts left are decodeURI/decodeURIComponent S15.1.3.x_A2.5_T1. Gate: cargo test 189/0; vmprobes tree/vm/gc-stress; 8796 cases from Array, Object/{freeze,seal,defineProperty,keys,getOwnPropertyNames,entries}, expressions/{array,assignment}, TypedArray, TypedArrayConstructors, ArrayIteratorPrototype, Iterator, for-of and String split are identical on both engines; the 2500-case equivalence is identical.

### Resumable expressions: class keys, optional chains, destructuring (checkpoint 45, 2026-11-11)

- The resumable compiler (`script/asynchronous.rs`) now lowers three shapes it used to reject. Generators, async functions, async generators and top-level-await module bodies all use it.
- Class heritage and computed keys that suspend (§15.7.14). The heritage, then every computed key up to the last one that suspends, run as ops: the key's evaluation, then ToPropertyKey. The class then evaluates with those keys already canonical, so later keys and every other step keep the spec order. The class scope and its private environment don't exist yet while these ops run. An early expression that names the class binding or one of its private names is therefore reported as Unsupported ("resumable class element naming its own class scope"), never resolved against the wrong environment. Anonymous classes keep NamedEvaluation.
- Optional chains with a suspending link (§13.3.9.1). Each `?.` link branches to one exit of its own chain that yields undefined. `a.b?.()` keeps `a` as its this value. An `Optional` link is never evaluated outside its chain (`open_optional`).
- Destructuring patterns whose defaults, computed keys or targets suspend: assignment (§13.15.5), declarations (§8.6.2), for-in/of heads and catch parameters (`Op::CatchParam`). They are lowered step by step and mirror `Engine::pattern_value`: target reference, then IteratorStepValue or GetV, then default and store. Array patterns push a guard handler (`Op::Destructure`), which closes the iterator on an abrupt completion while [[Done]] is false, including a generator `return()` at a `yield` inside the pattern. `Op::EndDestructure` performs the final IteratorClose. Object rest uses `Op::CopyRest` (CopyDataProperties with the evaluated keys excluded). GetV uses the primitive itself as the receiver, as the spec says.
- Probe `vmprobe/resumable_dstr.js` covers close on return, no close after exhaustion, assignment order, computed key plus rest, NamedEvaluation, catch, for-of heads, and an optional chain plus class key. It passes on tree, vm and `--gc-stress`.
- Smoke: the 120 cases of the surveyed buckets pass (they were 46 "await expression shape", 37 loop binding, 24 assignment pattern, 12 binding pattern, 1 catch binding). `language/expressions/optional-chaining` passes 84/84. A 400-case sample of the neighbouring dstr and for-await-of directories has 1 failure, which already failed in full51.
- Full run t262-full52 (VM): 47344/47708 = 99.2%, +113 (gained 120, lost 7). All 7 lost cases are runner timeouts from foreign load (load average ~17); they pass alone and HEAD and new builds run them in the same CPU time (decodeURI, decodeURIComponent, RegExp character-class-escape-non-whitespace, 3 TypedArray copyWithin detached cases, tco-call-args). decodeURI S15.1.3.1_A2.5_T1 takes 9-11 s of CPU on both builds: a roadmap B item. Unsupported fell 337 -> 217; the largest remaining bucket is built-ins/Atomics (109, test262 agents).

### Object model slice 3 part b: root-record hints (checkpoint 44, 2026-11-11)

- Each per-chunk name hint (`NameHint`, `script/global_names.rs`) also remembers the root global Environment Record whose VarNames held the name. A later access that starts at that record skips the §9.1.2.1 environment walk. The global-object step is still validated by key on every use. `Engine::var_names_epoch`, bumped only by DeleteBinding, drops every root. The hinted get and set now do one map lookup instead of two or three. Details are in `OBJECT_MODEL.md`, under "Slice 3 (part b)".
- Bug fixed: DeleteBinding (§9.1.1.4.7) now removes the name from VarNames. A VarNames entry whose global property is gone counts as unresolvable (HasBinding is HasProperty, §9.1.1.2.1), for reads, strict and sloppy writes, and strict ResolveBinding. New probe: `vmprobe/delvar.js`.
- Callgrind: `vm-bench/decode_tiny.js` drops from 257.4M to 217.5M instructions (-15.5%; the kill line was -15%).
- Full run on the VM (full51): 47231/47708 (99.0%), +2, lost 0. The gained cases are decodeURI and decodeURIComponent S15.1.3.x_A2.5_T1, the last full-profile timeouts. Gate: cargo test 189/0; vmprobes tree/vm/gc-stress; 3128 cases from global-code, identifier-resolution, variable, with, eval-code, delete, typeof, assignment, compound-assignment, decodeURI(Component), global, annexB eval-code, function, let and const. The two engines differ only on the two decodeURI cases, which still time out on the tree-walker. The 2500-case equivalence is identical.

### Object model slice 1: in-place slots, cheap object creation (checkpoint 39, 2026-11-10)

- Stage 4 design note: `OBJECT_MODEL.md` (dense elements, shapes, global/property caches, cheaper function environments). Five slices in order, with migration gates and kill lines. The literature check (Literature Review #82, request #376) adopted the order with corrections to the global-cache epoch rule and its scope, which the note records.
- Slice 1: `new_object` links the prototype and stores the slot in place, without the clone-and-relink of `replace_data`. `iterator_result` (CreateIterResultObject, §7.4.14) pushes `value`/`done` straight into the fresh object. `%ArrayIteratorPrototype%.next` (§23.1.5.1) updates the index and target in place. Callgrind: `vm-bench/copywithin_small.js` 7,817.6M to 5,169.6M instructions (-33.9%, kill line met). decode_tiny 423.4M to 421.9M (its cost is environments and name lookup: slices 2 and 3).
- Full run on the VM (full46): 47226/47708 (99.0%), +4 against full45 (the tco-non-eval cases fixed after full45), lost 0. The five shared timeouts remain: copyWithin coerced-values-*-detached now runs in 9.4-11 s alone, decodeURI/decodeURIComponent S15.1.3.x_A2.5_T1 in 16.8 s. Gate: cargo test 187/0; vmprobes tree/vm/gc-stress; 7323 cases from Array/TypedArray/TypedArrayConstructors/Map/Set/ArrayIteratorPrototype/Iterator/for-of give 7281 passes on the tree-walker and 7282 on the VM; the 2500-case equivalence is identical.

### Register VM stage 3: top-level script code (checkpoint 38, 2026-11-10)

- Coverage step 13: top-level script code runs on the VM (ScriptEvaluation, §16.1.6). GlobalDeclarationInstantiation (§16.1.7) stays the tree-walker's, and the script's var, lexical and function names are environment-resident. An expression that touches no register binding now compiles natively first and is hatched only when native compilation fails (`Compiler::native` rolls back). On `vm-bench/decode_tiny.js` this takes 458M to 424M instructions (callgrind).
- Over the 600 sampled cases, 530 scripts compile with 5 script fallbacks (`with` 3, function declaration in a block 1, `using` 1). Function bodies compile at 99.9%.
- Fix: a call through the name `eval` in tail position whose callee is not %eval% is an ordinary tail call (§13.3.6.1 step 9). The hatch now keeps the tail flag, and `tco-non-eval-{function,function-dynamic,global,with}.js` pass again.
- Full run on the VM (full45, before that fix): 47222/47708. Gained 0, lost 4 (the tco-non-eval cases above, fixed and smoke-checked afterwards), so the expected count is 47226 (99.0%), unchanged. The five shared timeouts remain. decodeURI/decodeURIComponent S15.1.3.x_A2.5_T1 no longer hit the step limit and pass in 14-15 s alone, still over the 10 s runner timeout. The copyWithin cases are bound by the object model (see BYTECODE_VM.md, coverage step 13): stage 4.

### Register VM stage 3: destructuring and spread (checkpoint 37, 2026-11-10)

- Coverage step 12, destructuring: declarations (`var`/`let`/`const`), for-in/of heads, assignment patterns and catch parameters compile through BindingInitialization (§8.6.2), IteratorBindingInitialization (§8.6.3) and DestructuringAssignmentEvaluation (§13.15.5). New instructions are `IterStep`, `IterRest` and `ObjectRest` (CopyDataProperties, §7.3.25). Iterator close on abrupt completion goes through a handler over the element range. TDZ stays static, per name in pattern order.
- Spread: array literal spread (§13.2.4.1) and spread arguments in calls and `new` (§13.3.8.1) build the list in an array (`ArraySpread`, sentinel count `SPREAD_ARGC`).
- Over the 600 sampled cases, 100.0% of function bodies compile. Remaining fallbacks: `with` 1, expression kind 1.
- Full run on the VM (full44): 47226/47708 (99.0%), with 1 gained (the full43 load timeout `sliceToImmutable/argument-coercion.js`, now a raw pass) and 0 lost. Equivalence over 2500 cases is identical on both engines. The dstr directories (1631 cases) and expressions/{call,new,array} plus statements/try (404 cases) give identical results on both engines. The probe `vmprobe/destructure.js` passes on both engines and under `--gc-stress`.

### Register VM stage 3: derived default constructors, block environments (checkpoint 36, 2026-11-10)

- Coverage step 10, `%forward`: the derived default constructor (§15.7.14 step 10.a) compiles. Its SuperCall is hatched and forwards the argument list without the iterator protocol.
- Coverage step 11, captured block lexicals: the frame keeps a current environment. `PushEnv`, `SetEnv` and `CopyEnv` implement BlockDeclarationInstantiation (§14.2.3), CreatePerIterationEnvironment for `for (let …)` (§14.7.4.2, §14.7.4.4), the for-in/of TDZ and per-iteration environments (§14.7.5.6-7) and the catch parameter environment (§14.15.2). Break, continue, return through `finally` and throw restore the environment of their target.
- Over the 600 sampled cases, 99.8% of function bodies compile (99.5% at checkpoint 35). Remaining fallbacks: for-in/of destructuring 2, destructuring declaration 2, spread arguments 1, expression kind 1, `with` 1.
- Full run on the VM (full43): 47225/47708 raw, with 0 gained and 1 lost. The lost case, `sliceToImmutable/argument-coercion.js`, is a load timeout (it passes alone in 4.6 s on both engines), so it is recorded as passed: 47226/47708 (99.0%), unchanged from full42. The 2,500-case equivalence sample is identical on both engines. The probes `vmprobe/{forward,block}.js` pass on both engines and under `--gc-stress`.

### Register VM stage 3: direct eval in functions (checkpoint 35, 2026-11-10)

- Coverage step 9, direct eval: a body that mentions the name `eval` keeps every binding in its environment record, and the `eval(...)` call is hatched, so the tree-walker decides whether it is a direct eval (§13.3.6.1) and runs PerformEval against the function environment. A direct eval is never a tail call. `delete name` of a free name is hatched, and of a register binding it is the constant `false` (§9.1.1.1.7). Only `%forward` still rejects a whole body.
- Spec fix on both engines: a `var` or function that a sloppy eval creates is deletable (CreateMutableBinding(N, true), §19.2.1.3). Environment records keep a `deletable` set. `eval-code/direct/var-env-{func,var}-init-local-new-delete.js` now pass.
- Over the 600 sampled cases, 99.5% of function bodies compile (99.3% at checkpoint 34). Remaining fallbacks: `%forward` 10, captured block lexical 5, for-in/of destructuring 2, destructuring declaration 2, spread arguments 1, `with` 1, other 1.
- Full run on the VM (full42): 47226/47708 pass (99.0%), +2 against full41, with 2 gained and 0 lost. The 2,500-case equivalence sample is identical on both engines. The probe `vmprobe/eval.js` (31 checks) passes on both engines and under `--gc-stress`.

### Register VM stage 3: templates, non-simple parameters, switch (checkpoint 34, 2026-11-10)

- Coverage step 6, template literals: `TemplateConcat` performs one TemplateLiteral step (§13.2.8.6), and `TemplateObject` performs GetTemplateObject (§13.2.8.4) through the per-realm cache. A tagged template goes through the same EvaluateCall path as an ordinary call, so it keeps the member/`with` receiver and its tail-call marking.
- Coverage step 7, non-simple parameters: the tree-walker's FunctionDeclarationInstantiation binds defaults, patterns and rest elements before the chunk runs, and the chunk copies the bound names. Parameters that a closure in an initializer mentions stay in the environment.
- Coverage step 8, switch: §14.12.4 with one CaseBlock scope, StrictEq selectors in source order and the default clause last. An unlabeled `break` targets the switch. A lexical declared in one clause and mentioned in a later clause still falls back, because the VM checks TDZ at compile time.
- Parser fix on both engines: `new tag`x`` binds the tagged template inside `new` (§13.3 MemberExpression TemplateLiteral). `tagged-template/constructor-invocation.js` now passes.
- Over the 600 sampled cases, 99.3% of function bodies compile (97.3% at checkpoint 33). Remaining fallbacks: eval 18, captured block lexical 5, for-in/of destructuring 2, destructuring declaration 2, spread arguments 1, other 1.
- Full run on the VM (full41): 47224/47708 pass (99.0%), +1 against full40, with 1 gained and 0 lost. On the 2,500-case equivalence sample, the only difference is the load timeout in `sliceToImmutable/argument-coercion.js`. The probes `vmprobe/{template,params,switch}.js` pass on both engines and under `--gc-stress`.

### Register VM stage 3: closures, the arguments object, for-in/of (checkpoint 33, 2026-11-10)

- Coverage step 3, closures: function-level names that a nested function, class or accessor mentions live in the function environment, not in registers (`ast_scan::captured_names`). Closures are created through `Hatch`. Top-level function declarations are instantiated at entry (`Chunk::hoist`), and `InitName` initializes captured lexicals. A captured block-level lexical still falls back.
- Coverage step 4, the arguments object: only direct eval and `%forward` still reject a body. In sloppy functions with a mapped arguments object, the parameters live in the environment, so the map aliases them.
- Coverage step 5, for-in/of: `GetIterator`, `IterNext`, `IterClose` and `IterCloseThrow` cover §14.7.5.6-7. IteratorClose runs on break, labeled break, return (also through nested `finally` blocks) and throw, but not after a throw from `next`. Heads with a TDZ `let`/`const`, `var`, a name or a member assignment target compile. Destructuring heads, `using` and `for await` fall back.
- Spec fix on both engines: `Object.freeze` on a mapped arguments object goes through [[DefineOwnProperty]] (§10.4.4.2), so it unmaps the frozen indices.
- Over the 600 sampled cases, 97.3% of function bodies compile (89.7% at checkpoint 32). Remaining fallbacks: template 40, non-simple parameter 33, eval 18, switch 16, captured block lexical 5.
- Full run on the VM: 47223/47708 pass (99.0%), unchanged from full39, with 0 gained and 0 lost. The probes `vmprobe/{closure,arguments,forof}.js` pass on both engines and under `--gc-stress`.
