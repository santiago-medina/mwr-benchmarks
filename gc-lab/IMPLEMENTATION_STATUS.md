# Implementation-only work

User direction: implement without creating or executing tests. Checkpoints use [skip ci]. No new implementation is verified or included in previous conformance/native proof claims. Historical evidence remains tied to its recorded commit.

Added in this batch: Map and Set constructors, SameValueZero lookup, mutation, size, callbacks, iterable ingestion, iterator closing on abrupt entry processing, ordered mutable iteration, species and prototype descriptors, and traced collection slots.

This is not 100% ECMAScript. Outstanding major areas include the full grammar, classes/private fields, generators, proxies, weak references/ephemerons, regular expressions, BigInt, typed arrays/buffers, date/time, internationalization, dynamic compilation, complete module surface, and the remaining intrinsic methods and edge semantics.

Array additions (unverified): Array.from/of, slice/splice, shift/unshift, reverse, fill/copyWithin, at/includes/lastIndexOf, find/findIndex/findLast/findLastIndex, reduce/reduceRight, and stable sorting. Generic operations reuse property descriptors, species construction and GC rooting.

JSON additions (unverified): syntax parsing, materialization, reviver traversal, stringify with toJSON/replacer/property lists, indentation, cycle detection and array/null omission rules. Current scalar-value String storage cannot represent lone UTF-16 surrogates; JSON reports that as Unsupported. Recursion is bounded by host limits.

Operator additions (unverified): conditional expressions with resumable async branches, bitwise operators and shifts, bitwise NOT, and right-associative exponentiation with unary-left syntax restriction. Numeric conversion helpers are shared with intrinsics.

Numeric additions (unverified): Math constants and 36 numeric methods, including compensated/scaled hypot, signed-zero round/min/max, f16round, fround, integer multiplication, and a per-thread randomly seeded hash stream for Math.random. Added parseFloat and global/Number predicates, with Number parsing aliases.
