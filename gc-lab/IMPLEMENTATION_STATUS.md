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
