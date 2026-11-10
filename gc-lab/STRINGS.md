# String representation: ill-formed UTF-16 strings (gap 20)

Status: design note, before implementation (2026-11-10).

## Problem

ECMA-262 §6.1.4 defines the String type as a sequence of 16-bit code units. A
String need not be well-formed UTF-16: a lone surrogate (U+D800–U+DFFF that is
not part of a lead/trail pair) is a legal element. The engine stores strings as
Rust `String` (`Value::String(String)`, `Key::String(String)`), which holds Unicode
scalar values only. Every operation that would produce a lone surrogate
currently reports `Unsupported("unpaired UTF-16 string storage")` (or the lexer
error "surrogate strings unsupported").

Size of the gap (full26, 2026-11-10): **483 cases**.

| Bucket | Cases |
| --- | --- |
| `built-ins/RegExp/property-escapes` (harness `buildString` covers U+D800–U+DFFF with `String.fromCodePoint`) | 423 |
| other RegExp (`CharacterClassEscapes`, modifiers, dotall, literals, named groups, ...) | 22 |
| `String.prototype` (at, codePointAt, isWellFormed, toWellFormed, pad*, match/search) | 9 |
| encodeURI / encodeURIComponent (must throw URIError on a lone surrogate, §19.2.6.5 Encode) | 10 |
| module export names (`IsStringWellFormedUnicode` early error, §16.2.1.1) | 6 |
| relational operators, for-of, JSON.stringify, Array concat, String.length, StringIterator | 13 |

## What the current code constrains

- `Value` and `Key` are `pub(super)`: the public host API (`host.rs`,
  `wire.rs`, the Servo bridge) only sees `&str`/`String`. A representation
  change stays inside `src/script`.
- `Value::String` appears at ~259 sites in 42 files; `Engine::to_string`
  (returns `String`) has 76 callers; `Key::String` ~20 sites. Value derives
  `Clone, PartialEq, Eq, Hash`.
- Strings are deep-copied on every `Value::clone()`. The property-escapes
  harness builds strings of up to ~1.1M code points and passes them around, so
  each clone copies megabytes.
- The RegExp VM already matches over `&[u16]` (it copies the input to UTF-16 on
  each call). Relational comparison already orders by code units
  (`encode_utf16().cmp`, execute.rs, §7.2.13 IsLessThan step 3).
- Most string builtins already compute UTF-16 indices over a `String`
  (`encode_utf16().nth(..)`, `strings::string(&[u16])`).
- Error messages and host strings are UTF-8 and must stay so.

## Options

1. **UTF-16 everywhere** (`Rc<[u16]>`, as SpiderMonkey/V8 two-byte strings).
   Exact spec model, O(1) code-unit indexing. Cost: every `&str` use (property
   names compared to `"length"`, number parsing, eval source, error messages,
   host boundary) needs a conversion; all ~350 sites change semantics at once.
   Doubles memory for ASCII text. Hard to migrate without losing passes.
2. **WTF-8** (`Vec<u8>`, surrogates encoded as 3-byte sequences; Servo's
   `script` and Rust's `std::sys::os_str` use it). Well-formed strings are byte-
   identical to UTF-8. But turning the bytes into `&str` needs `from_utf8`
   validation (O(n)) every time: `from_utf8_unchecked` is `unsafe` and the crate
   is `#![forbid(unsafe_code)]`. Concatenation must join a trailing lead and a
   leading trail into one 4-byte sequence. Code-unit indexing stays O(n).
3. **Canonical hybrid** (chosen): a newtype `JsString` with
   `enum Repr { Utf8(Rc<str>), Units(Rc<[u16]>) }` and the invariant
   **`Units` iff the string is ill-formed**. Well-formed strings, i.e. nearly
   all strings, keep a zero-cost `&str` view. Ill-formed strings are stored as
   the spec's code-unit sequence, which is also the RegExp VM's input format.
   Because the form is canonical, derived `Eq`/`Hash` are correct (two equal
   strings always have the same variant). Every constructor that may create or
   remove a lone surrogate (concat, slice, fromCharCode, toWellFormed) passes
   through one normalising function.

Literature/engine precedent (corrected after literature check #335, Literature
Review #82, 2026-11-10): SpiderMonkey, V8 and Boa use dual representations
(Latin-1 / two-byte) **without** a canonical-choice rule: a two-byte string may
hold only Latin-1 characters, and equality and hashing compare code-unit content.
So engines are no precedent for derived `Eq`/`Hash`; option 3's soundness rests
entirely on the invariant, as in WTF-8 well-formedness (Nova is the live Rust
WTF-8 engine). Servo's DOMString question (WTF-16 vs UTF-8) is open and handled at
the binding boundary by conversion. Option 3 has no exact precedent: it is a
canonical-encoding design in the WTF-8 family, with UTF-16 units as the
ill-formed form.

## Design

`src/script/js_string.rs`:

- `JsString` (`Clone` is an `Rc` bump; this also removes the deep copies).
- Constructors: `From<String>`, `From<&str>`, `from_units(&[u16])` (normalises:
  `String::from_utf16` succeeds → `Utf8`, else `Units`), `concat(&a, &b)`
  (if either side is `Units`, or a lead/trail pair forms at the seam, go
  through `from_units`).
- Views: `as_str() -> Option<&str>`, `text() -> Result<&str, ScriptError>`
  (returns the existing `Unsupported("unpaired UTF-16 string storage")` for
  `Units`: the migration helper for sites that are not yet migrated),
  `units() -> Cow<[u16]>`, `len_units()`, `unit_at(i)`, `is_well_formed()`,
  `to_well_formed()` (§22.1.3.32, U+FFFD replacement), `lossy()` for
  error messages only.
- `Display` is lossy and used only for diagnostics, never for a JS-visible
  result.

Spec-sensitive points:

- Code-point iteration (§11.1.4 CodePointAt, String iterator §22.1.5) yields a
  lone surrogate as its own one-unit string.
- RegExp `u`/`v` mode reads a lone surrogate as a code point (§22.2.2
  "pattern characters ... input as code points"); the VM already does this
  over `&[u16]`. `exec` results (`match[i]`, `input`, groups) are built with
  `from_units`, so they can be ill-formed.
- Encode (§19.2.6.5 step 3.c): a lone surrogate throws URIError.
- JSON.stringify QuoteJSONString (§25.5.2.3): lone surrogates are escaped as
  `\udxxx`.
- ModuleExportName (§16.2.1.1 early errors): `IsStringWellFormedUnicode` false
  → SyntaxError. This is a parser check on the cooked literal.
- Property keys: a string key may be ill-formed (`o["\uD800"]`). `Key::String`
  moves to `JsString` in phase 3; until then ToPropertyKey of an ill-formed
  string reports Unsupported (it cannot occur today either).
- Host boundary: `ScriptEngine` APIs that return `String` convert with
  `text()` (error) or document their lossy policy. A Servo WebIDL `DOMString`
  can hold lone surrogates, `USVString` cannot; this is decided in roadmap C,
  not here.

## Migration with no lost passes

Phase 1 — **type swap, no behaviour change.** `Value::String(JsString)`; every
site that needs text uses `as_str()`/`text()?`. `Units` cannot be produced yet,
so `text()?` never fails and behaviour is identical. Gate: cargo test, the four
gated profiles, and a ~50 case smoke run from the string/RegExp/JSON
directories, all with no change. Also measure the effect of `Rc` clones on a
fixed benchmark (property-escapes generated/ ASCII, Script_-_Latin and
General_Category_-_Letter, plus `examples/`) before and after.

Phase 2 — **producers and consumers for the bucket.** `strings::string`,
fromCharCode/fromCodePoint, the lexer (string and template literals produce
`JsString`; the AST literal type changes), `+`/concat/templates/join, indexed
access and `length`, at/charAt/charCodeAt/codePointAt, the String iterator,
isWellFormed/toWellFormed, padStart/padEnd/repeat/slice/substring, RegExp
input and result construction, encodeURI(Component), JSON.stringify, module
export names. Each consumer that still calls `text()?` keeps reporting
Unsupported, so nothing is accepted that is not implemented.

Phase 3 — **property keys and the long tail.** `Key::String(JsString)`, the
remaining `text()?` sites (split, replace, case mapping, trim, localeCompare,
Symbol descriptions, eval of ill-formed source, which is a SyntaxError unless
the surrogate is in a string literal or comment).

`grep -c 'text()?'` measures the phase-3 remainder; the ledger records it.

Kill line: 3 fix rounds on a bucket without progress → narrower survey.

## Results

- Phase 1 (dual `Utf8`/`Units` representation), release `js_check`, wall time:
  big_concat 25.49 s → 20.92 s, small_strings 7.73 s → 5.81 s, regex_strings 9.28 s → 9.29 s.
- Checkpoint 20 (full test262, Temporal binary): 45897/47708 (96.2%), +65 vs checkpoint 19,
  0 lost. "unpaired UTF-16 string storage" unsupported: 442 → 29. Most of the 429 new failures
  are runner timeouts in RegExp `property-escapes` / `CharacterClassEscapes` cases that were
  unsupported before. They are interpreter throughput (PROPERTIES.md, "What is left").
- After the checkpoint: decodeURI/decodeURIComponent decode each well-formed run and copy lone
  surrogates through (§19.2.6.6 step 4.b; escapes are ASCII, so this is exact). parseInt/parseFloat
  read the scalar prefix before the first lone surrogate (§19.2.4, §19.2.5: a lone surrogate is
  never whitespace or numeric syntax). Smoke: built-ins/decodeURI, decodeURIComponent, parseInt,
  parseFloat 218/220. The 2 failures (`S15.1.3.x_A2.5_T1`) are timeouts that also fail in full27.
- Still unsupported (9): lone surrogates in source text (comments, RegExp literals, group
  names). They need the lexer to work on code points over UTF-16, not on `&str`.

## Literature check #335 (2026-11-10): verdict and follow-ups

Verdict: adopt with corrections (report: task #2438, docs/literature-check-js-dual-string-repr-2026-11-10.md).
- Done already: `Repr` is private to `js_string.rs`, there is no `Ord` derive, and relational
  comparison works on code units.
- To do, as one small hardening item: a `debug_assert!` of the invariant in every constructor;
  a property test over random u16 sequences biased to surrogates, with concat/slice/at seams,
  checking `a == b ⇔ units(a) == units(b)` and equal hashes (and Map/Set keys); a U+FFFF vs
  U+10000 relational test.
- Perf controls for the benchmark set: a `+=` builder loop to 1M units (O(n²) check) and an
  indexed `s[i]`/`charCodeAt` loop over a long non-ASCII string. Pre-decided mitigations if
  either regresses: a cached unit length / ASCII flag for O(1) indexing, and an in-place append
  for a uniquely owned string (or a lazy rope node) for `+=`.
- Fallback if the invariant proves fragile: Boa-style handwritten Eq/Hash over code units
  (correct without canonicality, at the cost of `encode_utf16` when hashing `Utf8`).
- Host boundary (roadmap C): decide throw vs U+FFFD per API, matching SpiderMonkey's lossy
  UTF-8 conversion and Stylo's replacement policy.
