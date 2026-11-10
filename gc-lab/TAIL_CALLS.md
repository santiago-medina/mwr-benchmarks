# Proper tail calls (ECMA-262 §15.10)

Status: implemented in the tree-walker (checkpoint 30), `src/script/tail_calls.rs`.

## Problem

test262 `features: [tail-call-optimization]` (35 cases) recurse `$MAX_ITERATIONS`
(100000) times through a call in tail position. The tree-walker evaluates each call by
recursing on the Rust stack, and `invoke_this` stops at a call depth of 32 with
`Runtime("call depth limit")`, so all of them failed.

## Spec

- §15.10.1 IsInTailPosition: the call must be in strict code, inside a FunctionBody or
  ConciseBody that is not a generator, async or async generator body.
- §15.10.2 HasCallInTailPosition: only a `return` statement produces a tail call. Tail
  position passes through StatementList, Block, `if`, the iteration statements, `switch`,
  labelled statements, the Catch of `try … catch` and the Finally of `try … finally` /
  `try … catch … finally`. Inside an expression it passes through `?:` (both branches),
  the right operand of `&&`, `||`, `??`, the last operand of `,`, parentheses, calls and
  tagged templates.
- §15.10.3 PrepareForTailCall: the running execution context is discarded before the call
  (§13.3.6.1 EvaluateCall step 6), so the callee's result becomes the caller's result.

## Design: trampoline in `invoke_this`

- `Engine::tail_ok` is set in `invoke_inner` right before an ordinary closure body runs,
  when the body is strict and the kind is not a derived constructor (its result check
  runs inside that frame). Generators and async functions return earlier through
  `start_generator` / `start_async`, so they never see it. `invoke_this` clears it for
  every callee and restores the caller's value afterwards.
- `Engine::non_tail` counts enclosing statements whose body is *not* tail position here:
  the `try` Block, a Catch that has a Finally, for-in/for-of bodies and blocks with `using`
  declarations (`non_tail_scope`).
- `return e` with `tail_ok && non_tail == 0` evaluates `e` with `eval_tail`, which follows
  the §15.10.2 expression shapes. At a call (or tagged template) it evaluates the callee,
  `this` and the arguments as usual, then stores them in `Engine::tail_call` (rooted with
  `ExternalRoot`s, since the statement frames that protected them unwind) and returns a
  placeholder. A direct `eval(...)` is never deferred (§13.3.6.1: it is not a call).
- `invoke_this` runs the callee in a loop: when the body returns `Ok` with a deferred
  call, it releases the roots, ticks the step guard and calls the new target at the same
  call depth. An error discards the deferred call.

Constructors: `new F()` for a strict base function or class constructor goes through
`invoke_this`, so a tail call there yields the callee's result to the [[Construct]] result
check, as the spec's popped context does.

## Conservative deviations

Two places the spec lists as tail position keep ordinary calls, because the tree-walker
would run the call after work the spec runs after it:

- for-in/for-of bodies: the loop closes the iterator after the `return` completion
  (§14.7.5.7). Deferring the call would call `return()` on the iterator before the tail
  call instead of after. (No test262 case covers tail calls in for-in/of.)
- Blocks with `using` declarations: DisposeResources runs after the body's completion.

Optional-chain calls (`a?.b()`) are also evaluated as ordinary calls for now. All three
only cost stack depth; they change no observable result except the call depth limit.

## Bytecode VM (roadmap B)

Stage 3 of BYTECODE_VM.md keeps frames on a heap `Vec<Frame>`. A `TailCall` instruction
there replaces the trampoline: it pops the frame and reuses its register window. The
`non_tail` rules become a compile-time property of the `return` site.

## Results

See IMPLEMENTATION_STATUS.md (checkpoint 30).
