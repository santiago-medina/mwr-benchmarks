# Explicit resource management (`using`, DisposableStack)

Design note for gap 15. Spec: ECMA-262 2026 (explicit resource management
merged from the TC39 proposal), sections cited below as in the proposal text
(§ numbers follow the merged spec draft).

## Scope

| Area | test262 dirs | Cases |
|---|---|---|
| `SuppressedError` | built-ins/SuppressedError | 21 |
| `DisposableStack` | built-ins/DisposableStack | 93 |
| `AsyncDisposableStack` | built-ins/AsyncDisposableStack | 104 |
| `%AsyncIteratorPrototype%[@@asyncDispose]` | built-ins/AsyncIteratorPrototype/Symbol.asyncDispose | 9 |
| `using` / `await using` | language/statements/{using,await-using}, for-of, for-await-of | ~190 |

## Built-ins (new module `src/script/disposal.rs`)

* **DisposeCapability** (§ 9.11 "DisposableResource Records"): a stack of
  `(value, hint, method)` records, stored in a new `Data.dispose` slot of the
  stack object (or of a hidden scope object for `using` blocks). Edges are
  traced like any other `Data` slot, so the GC sees the resources and methods.
* **AddDisposableResource(cap, V, hint, method?)** (§ 9.11.2): sync hint and
  null/undefined → nothing recorded; await hint and null/undefined → record
  with undefined method (it still forces an `Await`). Non-object → TypeError;
  `GetDisposeMethod` (§ 9.11.4) gets `@@asyncDispose`, falling back to
  `@@dispose` wrapped in a closure that calls it and returns a promise
  resolved to undefined (closure = new `Internal` variant). Missing method →
  TypeError.
* **DisposeResources(cap, completion)** (§ 9.11.6): pop records in reverse;
  a throw from a disposer, when the completion is already a throw, becomes
  `SuppressedError { error: new, suppressed: old }`; otherwise it replaces
  the completion. The async form awaits each async result and, per the
  `needsAwait`/`hasAwaited` steps, issues one `Await(undefined)` for
  null/undefined `await using` resources.
* **DisposableStack** (§ 27.3): constructor requires `new`, uses
  OrdinaryCreateFromConstructor. `use`/`adopt`/`defer` throw ReferenceError
  once disposed, TypeError on non-callables. `dispose` and `[@@dispose]` are
  the same function object. `move` transfers the capability to a new stack and
  marks the old one disposed. `disposed` is an accessor.
* **AsyncDisposableStack** (§ 27.4): same shape; `disposeAsync` (=
  `[@@asyncDispose]`) is a native state machine in the `from_async.rs` style
  (job object + `promise_resolve` + reactions); a bad receiver rejects the
  returned promise instead of throwing.
* **SuppressedError(error, suppressed, message)** (§ 20.5.8): message is
  argument 2, installed only when not undefined; then `error`, `suppressed`
  as non-enumerable own data properties; length 3; no `cause` option.

## Syntax (parser.rs)

`using x = e` / `await using x = e` parse to
`Stmt::Bind { kind: "using" | "await using", pattern: Name, init }`. They are
never lowered to `const`: semantics differ, and accepting them as `const`
would hide missing disposal.

Early errors (§ 14.3.1.1, § 14.7.5.1):
* `using` must be unescaped and followed on the same line by a binding
  identifier (`using[x]`, `using\nx` stay expressions). In a for head
  `using of` is a declaration only when `=` follows (so `for (using of of y)`
  stays an identifier for-of).
* Initializer required outside for-of heads; no binding patterns; bound name
  `let` is an error.
* Not allowed: at the top level of a Script or eval code, directly in a
  `case`/`default` clause, in statement position, in `for (using x in y)`,
  or with an initializer in a for-of head.
* `await using` only where `await` is allowed (async functions, async
  generators; not class static blocks).

Every place that treats `const` as immutable / lexical (environment
constants, redeclaration checks, the VM's per-iteration `lexical` test) uses
a helper `is_lexical(kind)` that includes the using kinds.

## Execution

### Tree-walker (`execute.rs`)

A block, switch, `for` head or for-of iteration whose statements contain a
using declaration gets a hidden scope object (`Data.dispose`) bound as
`%dispose` in its environment and rooted with `protect`. The body runs, then
DisposeResources runs natively on the outcome:
`Err(Thrown)` is a throw completion; `Ok`, break, continue, label break and
return are normal completions and are kept unless disposal throws. Engine
errors (Unsupported, limits) propagate without disposal. Completion values
are untouched, which `cptn-value.js` checks (`eval('4; {using x=null;}')`
is 4); that is why this is not a `try/finally` desugaring.

For-of disposes the iteration's resources before IteratorClose. An
`await using` reaching the tree-walker returns Unsupported (the async VM owns
it; `stmt_await` routes any `await using` there).

### Async VM (`asynchronous.rs`)

The block compiles as `Enter; DisposeStart; Try{finally}; body; EndTry;
finally: DisposeFinally; [async loop]; DisposeFinish; EndFinally; Exit`.
`DisposeFinally` moves the handler's pending Throw into the scope's
completion (keeping Return/Break as pending). For sync-only scopes disposal
runs natively. For scopes with `await using`, a loop of
`DisposeNext` → `Await` (inside a nested catch that records the rejection
into the completion) repeats until the stack is empty; `DisposeFinish`
rethrows a throw completion.

## Migration / risk

No pass can be lost by the parser change: today every case with this syntax
is filtered as unsupported (`UNIMPLEMENTED_SYNTAX` in `conformance/run.py`),
which is removed once both executors handle it. Blocks without `using` take
the existing path (a cheap `has_using` scan), so other tests do not change.
