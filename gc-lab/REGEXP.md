# RegExp engine

The engine has its own RegExp implementation in safe Rust, with no external regex crate. It has three layers.

## 1. Unicode tables (`src/script/regexp_tables.rs`)

`gen_unicode_props.py` generates this file from the Unicode 17 UCD. Run it as `python3 -I gen_unicode_props.py <ucd dir> src/script/regexp_tables.rs`. It contains:

- the General_Category, Script and Script_Extensions values, and the binary properties that `\p{..}` accepts, together with their aliases;
- the simple case-folding table (`FOLD`), used by `i` with `u`/`v`;
- the emoji string properties (`RGI_Emoji`, …), used by `\p{..}` in `v` mode.

## 2. Matcher (`src/script/regexp.rs`)

This layer is pure: it does no engine or GC work.

- **Parser**:
  - the full ES2025 pattern grammar, with Annex B extensions when neither `u` nor `v` is set;
  - named groups, including duplicate names in different alternatives;
  - modifiers `(?i:)`;
  - `v`-mode class set operations (`--`, `&&`, nested classes, `\q{}`);
  - `\p`/`\P`;
  - lookbehind.

  Syntax errors come back as `Err(String)`.
- **Compiler**: turns the parse tree into a small bytecode program (`Program`, with `groups` and `names`).
- **VM**: a backtracking interpreter over UTF-16 code units. In Unicode mode it reads code points instead. It has a step budget of 50M steps; when that runs out it returns `Exhausted`, which the engine reports as the runtime error "regular expression step limit". There is no recursion on the Rust stack, so deep patterns cannot overflow it.

## 3. Builtins (`src/script/regexp_builtins.rs`)

This layer follows the ECMAScript specification.

- **Object slots**: `Data::regexp` holds an `Rc<Matcher>` with the source, the flags and the compiled program. Compiled programs are shared through a per-thread cache keyed by pattern and flags. `Data::regexp_cursor` holds the %RegExpStringIterator% state; its regexp `Id` is a GC edge.
- **Constructor**: `RegExp(pattern, flags)` implements IsRegExp, returns the pattern unchanged when RegExp is called without new, and applies RegExpAlloc and RegExpInitialize. `lastIndex` is defined at allocation as writable, non-enumerable and non-configurable. The constructor also has `RegExp[@@species]` and `RegExp.escape`.
- **Prototype**:
  - `exec`, `test`, `toString` and `compile` (Annex B);
  - the getters `flags`, `source` (EscapeRegExpPattern), `hasIndices`, `global`, `ignoreCase`, `multiline`, `dotAll`, `unicode`, `unicodeSets` and `sticky`. The getters return undefined (or `"(?:)"` for `source`) when the receiver is `RegExp.prototype` itself.
- **Exec**: RegExpExec calls a user-defined `exec` when one exists; RegExpBuiltinExec does the matching itself. A match result has `index`, `input`, a null-prototype `groups` object and, with the `d` flag, `indices` (with its own `groups`).
- **Symbol methods**: `@@match`, `@@matchAll`, `@@replace` (GetSubstitution with `$1`–`$99` and `$<name>`, plus function replacers that receive the groups argument), `@@search` and `@@split` (SpeciesConstructor, sticky splitter). Their names and lengths follow the specification.
- **String methods**: `String.prototype.match`, `matchAll` and `search` delegate through the symbol methods. `replace`, `replaceAll` and `split` already did that (`string_search.rs`), and IsRegExp now also recognises the internal slot.
- **Literals**: the parser compiles each literal once and reports an invalid pattern as an early SyntaxError. Each evaluation of a literal creates a new object; the compiled program comes from the cache.

## Limits and known gaps

- A match runs on a UTF-16 copy of the input string, made on every call.
- The matcher backtracks, so matching takes exponential time on pathological patterns. The step budget limits it to a runtime error and never lets it hang.
- The legacy static properties (`RegExp.$1`, `RegExp.lastMatch`, …) are not implemented.
