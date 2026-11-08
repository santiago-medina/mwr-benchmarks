# Gap 2 design: async generators + for-await-of (2026-11-08)
Paths relative to repo/gc-lab/src/script. Target: ~3.0k "async generator" unsupported cases plus
~1.5k for-await-of / async-generator-declaration cases that fail parsing ("expected identifier/punctuation").

## Parser / AST
- `Expr::GeneratorFunction` gains `asynchronous: bool`; `Stmt::GeneratorFunction(name, params, body, asynchronous)`.
  (A bool field rather than a new variant: every match site must be revisited by the compiler.)
- Accept `async function*` (declaration, expression, module export) and `async *m(){}` in object literals and classes.
- `Stmt::ForOf` gains `asynchronous: bool`; `for await (` parses only the of-form.
- Validation: async generator bodies are both in_async and in_generator.

## Runtime
- FunctionKind::AsyncGenerator / AsyncGeneratorMethod; compiled to the resumable program like generators.
  Closure [[Prototype]] = %AsyncGeneratorFunction.prototype%; own `prototype` inherits %AsyncGeneratorPrototype%.
- Generator record reused: `asynchronous` flag, `queue: Vec<Request{mode, value, capability}>`, extra state AwaitingReturn.
- Activation Target::AsyncGenerator(id); a new `pending` field records why the frame awaits:
  YieldValue(out) (await before yield), ReturnValue (await of return operand / of a return resumption),
  DelegateResult(mode) (await of inner async iterator result in yield*).
- %AsyncGeneratorPrototype%.next/return/throw: new %Promise% capability, invalid receiver -> rejected promise,
  enqueue, then `async_generator_drain` (spec AsyncGeneratorResumeNext/AwaitReturn/CompleteStep as one loop, no recursion).
  Every AsyncResume of an async-generator activation also drains afterwards.
- yield: Await(v) then resolve front request {v,false}, state Yield. Return resumption at a yield awaits the value.
  `return v` (Op::Return or a Sync statement returning) awaits v, then routes the Return completion (finally blocks run).
- yield* in an async generator: GetIterator(async) (falls back to CreateAsyncFromSyncIterator); next/throw/return
  results are awaited; yields the inner value without re-awaiting.
- for await: Op::AsyncIterator (@@asyncIterator, else async-from-sync wrapper), next() result awaited, then checked.
- %AsyncFromSyncIteratorPrototype% next/return/throw: call the sync method, await the value, resolve {value, done}.
- %AsyncIteratorPrototype%[@@asyncIterator] returns this. AsyncGeneratorFunction constructor via dynamic_function_kind.

## Known simplification
- Abrupt exit from for-await (break/return/throw) calls iterator.return() but does not await its result.
