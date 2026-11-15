# Function source text (§20.2.3.5 [[SourceText]])

`Function.prototype.toString` returns the source text of every script-defined
function, as ECMA-262 §20.2.3.5 step 2 requires. Built-ins and callable proxies keep
the NativeFunction form (`function () { [native code] }`, steps 3-4).

## Design

- **Token spans.** `tokenize_legacy` records `(start, end)` byte offsets per token
  (end = the lexer position after the token, before trailing whitespace or comments).
  The parser keeps them in `spans: Rc<Vec<(usize, usize)>>`, sorted by start, and
  looks a token's end up by its start offset. Lookup by start (not token index)
  keeps sub-parsers working: the for-head sub-parser parses a slice of the token
  vector and shares the same table.
- **Shared text.** `SourceText { text: Arc<str>, start, end }` points into one
  `Arc<str>` of the whole source, created on first use per parse. A closure created
  in a loop shares the span; no text is copied per function object.
- **AST marker.** `Stmt::SourceText(SourceText)` is appended as the **last**
  statement of a function body. It cannot go first: `body.first()` is the
  `Stmt::Sloppy` strictness marker, and the directive-prologue scan in `validate`
  stops at the first non-string statement. Every statement executor (tree
  interpreter, bytecode compiler, resumable Op compiler, `ast_scan`, `validate`)
  treats it as a no-op. Putting it in the body (not in each `Expr`/`Stmt` function
  variant) keeps the AST shape of the ~10 function variants and their many
  constructors unchanged.
- **Where spans start** (§15.1-§15.9 productions):
  - function declarations and expressions: `function` or `async`;
  - methods, getters and setters: the MethodDefinition, which includes `get`, `set`,
    `async`, `*` and computed `[...]` brackets, and excludes `static` and `accessor`
    (§15.7: `static` belongs to the ClassElement, not the MethodDefinition);
  - arrows: the first token of the ArrowParameters or of `async`;
  - classes: the `class` keyword. The span is stored on `Class.source` and
    replaces the class constructor's [[SourceText]] (§15.7.14 step 1).
    DecoratorLists before `class` are not part of the span.

  Functions that `function_rest` parses without a start token (class constructor
  bodies, synthesized field and static-block functions) get no marker.
- **Runtime.** `make_function` copies a trailing marker into `Data.source`.
  `eval_class` overwrites it with the class text. Dynamic functions
  (`Function(...)`, §20.2.1.1.1 step 29) store their synthesized text with
  `SourceText::owned`. `function_source` returns `Data.source` and falls back to the
  NativeFunction form.

## Equality

`SourceText` implements `PartialEq` on the text slice, so two ASTs parsed from the
same text compare equal regardless of which allocation they point into.

## Verification

- `built-ins/Function/prototype/toString`: 80/80 (previously 79/80; the failing case
  `method-computed-property-name.js` uses method source text as a property key).
- `vmprobe/fn_source.js`: about 25 shapes (comments inside functions, computed
  methods, `static`, concise arrows, template and regex literals, non-ASCII text,
  dynamic functions) on the tree engine, the VM and with `--gc-stress`.
