# Sloppy mode (non-strict code)

Date: 2026-11-09. Baseline: t262-full8 (28157 pass). Survey of the 1740 `noStrict`
(non-module) cases run with `js_check --sloppy` on the strict-only engine
(survey_sloppy.py → scratch/sloppy_survey0.tsv): 426 pass, 323 fail, 991 unsupported.
Blockers: `eval` (~400, not implemented at all), `with` (~290), sloppy-only
identifiers/delete (~120), runtime semantics (implicit globals, `this` coercion,
silent failed writes, delete results: ~200 failures).

## Phase A — strictness + sloppy semantics + `with` (this gap)
Parser
- `Parser.strict`: scripts start sloppy; a directive prologue containing the raw
  text `"use strict"`/`'use strict'` makes the script/function strict; class bodies
  are always strict; nested functions inherit.
- Sloppy function bodies (and a sloppy script) carry a leading `Stmt::Sloppy`
  marker. Runtime strictness is read from it, so no AST variant needs a new field.
- Legacy octal numbers (`010`) and `08`/`09`, and legacy octal string escapes:
  the lexer accepts them and records the token offset; the parser rejects them in
  strict code (also retroactively in the prologue before `"use strict"`).
- Sloppy only: `with (e) s`, `delete identifier`, duplicate simple parameters,
  `let` as an identifier when not followed by an identifier, `[` or `{`.
Runtime
- `Environment.strict` (inherited from the parent; function environments take it
  from their body marker). `Environment.with_object: Option<Id>` is an object
  environment record; `in_with` (inherited) keeps the fast path when no `with`
  is on the chain.
- Sloppy effects: unresolvable assignment creates a global property; failed
  property writes (non-writable, no setter, non-extensible, primitive base) are
  ignored; `delete` returns false instead of throwing; `delete name` works on
  global-object properties and `with` objects; `this` is coerced (nullish → global
  object, primitives → wrapper) in non-arrow, non-class functions;
  `arguments.callee` is the function.
- `with`: lookup/assign/typeof/delete/call consult HasProperty + @@unscopables on
  the object; a call through a with-reference gets the object as `this`.
Runner: `noStrict` and `raw` cases run with `--sloppy` (no prefix). Cases with
no flag stay strict-only (test262 asks for both modes: a later variant).

## Phase B — `eval` (done)
Direct eval (`eval(...)` resolving to %eval%) parses the string with the caller's
strictness and evaluates in the caller's environment; sloppy direct eval declares
`var`/functions in the caller's var environment; strict eval gets its own var
scope. Indirect eval runs as a global sloppy script.

## Out of scope / later
Mapped `arguments` (parameter aliasing, exotic object), Annex B block-function
hoisting, `yield` as an identifier, labels (no label support at all yet),
running default cases in both modes.

## Implementation notes (phase A)
- Assignment to an immutable binding that is not a lexical `const` (a named
  function expression's own name, global `undefined`/`NaN`/`Infinity`) is ignored in
  sloppy code; `Environment.constants` keeps `const` assignment a TypeError in
  both modes.
- The Function/GeneratorFunction/AsyncFunction constructors drop the script-level
  marker from the synthesized declaration; the body's own prologue decides strictness.
- Phase-A survey after the change (scratch/sloppy_survey2.tsv): 881 pass, 127 fail,
  732 unsupported out of 1740 noStrict cases (it was 426 pass before). Remaining:
  eval (~440 unsupported), `yield` as an identifier (82), mapped arguments (~25
  failures), `with` reference timing (the binding is deleted or shadowed between
  resolution and PutValue), `arguments.callee`, `with` inside generators/async.

## Implementation notes (phase B)
- `script/eval.rs`: PerformEval + EvalDeclarationInstantiation. Direct eval is detected
  in `eval_call` (callee named `eval` that is %eval%, not `new`); indirect eval is the
  `eval` builtin, run against the global environment.
- `parser::parse_eval(source, strict, super_property, super_call)`: the caller's
  strictness and whether `super.x` / `super()` are allowed (the caller's environment
  binds `%home` / `%func`).
- Sloppy eval: vars and top-level functions go to the caller's variable environment
  (configurable properties on the global object at the top level, with the
  CanDeclareGlobalFunction/Var checks); a var that would hoist across a lexical
  declaration of the same name is a SyntaxError (environments record their lexical
  names in `lexical`). Strict eval keeps everything in its own scope.
- Completion values: `Engine::completion` is only tracked while eval code runs and is
  saved/cleared around function calls; expression statements set it, and loops,
  `if`, `switch`, `with` and `try` reset it to undefined first (UpdateEmpty).
- Known gaps: Annex B catch-parameter redeclaration by eval vars is flagged as a
  conflict; eval-declared vars in functions are not deletable; `eval(await x)` in
  async code runs as an indirect call; labels are still unsupported (the two
  for-of `cptn-*-abrupt-empty` cases are known failures until they exist).
