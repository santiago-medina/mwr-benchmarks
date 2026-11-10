# Bytecode VM and interpreter throughput (roadmap B)

Status: design note, 2026-11-10. Nothing in it is implemented yet.
Gates: literature check #336 (Literature Review (#82)) returned ADOPT. The `taskygpt` second
opinion was waived by Santiago on 2026-11-10, so stage 3 is unblocked.

## Problem (measured)

Checkpoint 20 (full27, commit 9acf139) had 457 runner timeouts. 441 of them are
`built-ins/RegExp/property-escapes/generated/*`. The others are the
CharacterClassEscapes cases, 4 URI `A2.x_T1` cases, 3 TypedArray `copyWithin` cases
and 2 `Iterator.zip` cases. Each property-escape test calls `buildString` from
`harness/regExpUtils.js` twice: once for the matching code points and once for the
complement. Together that covers about 1.1M code points, through a counted loop with
`let` bindings and an indexed array store, flushed in chunks with
`String.fromCodePoint.apply`. The RegExp half is cheap: `regex_scan.js` below runs
`\p{Script=Han}` over 20k code points in 0.23 s.

### Fixed benchmark set

The set is in `bench/vm/`. `run.sh LABEL [binary]` appends to `results.tsv`. These are
wall times from a release `js_check` at 9acf139 on the shared WSL2 machine, which was
idle:

| case | what it stresses | 9acf139 |
|---|---|---|
| `loop_let.js` | buildString shape, 200k iterations | 4.56 s |
| `fib.js` | `fib(22)`, 28,657 calls | 1.12 s |
| `objects.js` | 50k object literals plus named get and set | 1.06 s |
| `closures.js` | 20k closures and upvalue writes | 1.54 s |
| `strings.js` | 20k concatenations plus 20k `charCodeAt` | 1.29 s |
| `regex_scan.js` | `\p{Script=Han}` over 20k code points | 0.23 s |
| `Script_-_Manichaean.js` (test262) | one full property-escape case | 15.70 s (budget 10 s) |

For before/after claims, use callgrind instruction counts, which do not depend on load:
`scratch/prof.sh`, valgrind 3.18 installed user-local.

- `loop_let` at 20k iterations: **1.19 G instructions, about 60k per iteration.**
- `fib(16)`, 1,973 calls: **257 M instructions, about 130k per call.**

Node finishes each benchmark in a few milliseconds. At about 60k instructions per
iteration, one property-escape case is roughly 66 G instructions. To fit the 10 s budget
with margin, it has to drop below about 5k per iteration, at least 12x.

### Where the instructions go

Inclusive callgrind, excluding recursive frames:

**loop_let:**

| cost | share | cause |
|---|---|---|
| `assign_member` (`codePoints[length++] = cp`) | 32 % | about 19k instructions per store |
| `new_environment` | 9.4 % | per-iteration `let` copies (§14.7.4.4 CreatePerIterationEnvironment); each is a GC allocation and six `HashMap`/`HashSet`s |
| `allocate` | 8.3 % | |
| `lookup_with_base` | 7.2 % | SipHash string lookup at each scope level |
| `String::clone` | 7.0 % | names and keys are cloned everywhere: `Key::String(String)`, `Value::Native { name: String }`, binding names |
| `prune_metadata`, `collect` | about 3 % each | |

Array elements are `Key::String("123")` entries in the general `PropertyMap`. Every
index store formats a decimal string, hashes it and allocates.

**fib:**

| cost | share | cause |
|---|---|---|
| `function_environment` | 62 % | every call builds an `arguments` object (`parameters.rs:73-85`), mapped in sloppy mode, even when the body never names `arguments`; that is about 21 % in `define_in_place` and `replace_data` alone |
| `Object::clone` + `Stmt`/`Expr::clone` | about 13 % | `invoke_inner` (`execute.rs:1005`) does `self.objects.get(&id).cloned()`, which deep-copies the closure's parameter list and **whole body AST** on every call |
| `new_environment` | 12 % | |
| `lookup_with_base` | 4.6 % | |
| `prune_metadata`, `collect` | about 7 % | |

In short, about half of the cost is incidental: the AST clone per call, the eager
`arguments` object, string-keyed array elements and `String` clones. A tree-walker can
remove that without a new architecture. The rest is structural: name-keyed hashed
environments allocated on the GC heap, plus the cost of a recursive AST walk through
`Result` (call depth, no reuse of decoded operands). A tree-walker cannot remove that.

## What the current code constrains

1. **GC model** (`dual.rs`). The heap is non-moving and `Id`-indexed. Edges are explicit
   and maintained eagerly (`link`/`unlink` on every store that adds or drops a reference).
   Roots are `ExternalRoot` entries. Rust-stack temporaries are rooted with
   `Engine::protect` and released by `Frame::drop`. A collection runs only at an
   allocation safepoint (`maybe_collect`). Any VM register file must therefore
   (a) be scanned as a root set at collection time, or (b) root each value it holds.
   Option (b) is what makes the current code pay `Runtime::root` per temporary.
2. **Environments are GC objects** (`Object::Environment` with `HashMap<String, Option<Value>>`).
   They are reachable from closures, from mapped `arguments` (`parameter_map` holds
   `(env Id, name)`) and from `with` and sloppy direct `eval`, which add bindings at run
   time (§9.1.1.1, §19.2.1.3 EvalDeclarationInstantiation). Generators and async
   functions keep their state in `Activation`, whose "registers" are environment
   bindings named `"\0async{r}"` (`asynchronous.rs:87`).
3. **There is already a resumable op machine.** `asynchronous.rs` compiles generator and
   async bodies into `Program { ops: Vec<Op> }`, but `Op::Eval(Expr, r)` hands every
   await-free expression back to the tree-walker. It is the precedent for (and the
   eventual consumer of) a real compiler: a full bytecode makes suspension free, because
   a frame is plain data.
4. **Engine defaults reach every embedding.** These are `js_check`, `module_check`,
   `script_demo`, `servo_bridge` and `servo-native/mwr.rs`. The step limit
   (`Engine::tick`, 10,000 by default and 50M in `js_check`) counts AST statements. A
   bytecode VM changes what a "step" is, so the guard needs a new definition. It is a
   test guard and **must not reach the Servo path as a page limit** (flagged under
   CLAUDE.md "Interpreter budgets").
5. **Safe Rust only.** There is no computed goto, no NaN-boxing through transmute and no
   raw frame pointers. Dispatch is a `match` on an enum, which LLVM lowers to a jump
   table. Operands are indices into `Vec`s with bounds checks; hot loops can amortise
   those with slice patterns.
6. **About 46k lines of semantics live in `Engine` methods** that take `Value`s
   (`get_member`, `assign_member`, `invoke_this`, `binary_values`, `to_primitive`, ...).
   Those are the abstract operations, and a VM calls them unchanged. A rewrite only
   replaces **evaluation of syntax**: `execute.rs`, `loops.rs`, `patterns.rs`,
   `classes.rs` (evaluation part), `parameters.rs`, `asynchronous.rs` and
   `generators.rs`.

## Options

| Option | Expected gain | Cost | Risk |
|---|---|---|---|
| **O1. Tree-walker fixes only**: `Rc` bodies, lazy `arguments`, dense elements, interned names | about 2x (the incidental half) | small | low; still about 25k instructions per iteration, short of the 12x needed |
| **O2. O1 plus resolved bindings** (a scope-analysis pass annotates each `Name` with `(hops, slot)` or Dynamic; environments become `Vec<Value>` plus a fallback map) | about 4-6x | medium | medium; `with`, sloppy eval and mapped arguments need a dynamic path |
| **O3. O2 plus a register bytecode VM** (Lua 5 / Hermes style: three-address ops over a frame-local `Vec<Value>` register window) | 10-30x over today | large | high, but staged (below) |
| O3'. Stack bytecode (QuickJS, SpiderMonkey; Boa before v0.21) | about the same as O3 | large | more dispatches per operation. Shi et al. 2005 report a 1.15 speedup for registers with inline-threaded dispatch (the figure literature check #336 verified); the "47 % fewer executed VM instructions" figure cited here earlier could not be sourced by that check and is not load-bearing. No Rust or match-dispatch measurement exists: the stage-3 kill line is the real test |
| O4. Accumulator plus registers (V8 Ignition) | about the same as O3 | large | denser code, but every op has an implicit operand; no clear win without a JIT tier to feed |

**Choice: O3, reached in stages through O1 and O2.** Each stage ships on its own with
measured gains and no lost passes. O1 and O2 are needed by O3 anyway. The VM consumes
the scope analysis of O2, and dense elements and interned keys belong to the object
model, not to the evaluator.

## Design

### Stage 1: incidental costs (tree-walker, no new architecture)

- `Object::Closure { body: Rc<[Stmt]>, params: Rc<[Param]> }`. The call path borrows
  these instead of cloning them. A `Builtin` call also stops cloning `Data`: only the
  name and `internal` are needed.
- **Lazy `arguments`** (§10.2.11 FunctionDeclarationInstantiation step 15-22). The spec
  creates the object only when `argumentsObjectNeeded` is true. That flag is false for
  arrow functions, when a parameter is named `arguments`, and when there is no direct
  `eval` and no lexical `arguments` binding. So `argumentsObjectNeeded` should
  additionally be cleared when the body (excluding nested non-arrow functions) neither
  contains the identifier `arguments` nor a direct `eval`. This is an unobservable
  optimisation, the same one engines make. It is computed once per function at
  `make_function` time and cached next to the body.
- **Dense elements.** `Data.elements: Vec<Option<Value>>` for array-index keys
  (§6.1.7 array index, < 2^32 − 1) when every element is a writable, enumerable,
  configurable data property. The first non-default attribute, accessor or hole beyond
  a sparseness threshold falls back to the `PropertyMap`. OrdinaryOwnPropertyKeys
  (§10.1.11.1) already orders integer keys first, so the iteration order is unchanged.
  GC edges are linked per element, as today.
- **Interned names.** `Key::String(String)` → `Key::String(JsString)` (already `Rc`).
  `Value::Native { name: String }` → `&'static str` or an index into the builtin
  table.
- Measure each change on the benchmark set. Expected: fib loses the `function_environment`
  and clone share (about 2x), and the loop loses about a third.

### Stage 2: scope analysis and slot environments

- A resolver pass over the AST, after parsing and before evaluation, computes per
  function and block scope (§9.1 Environment Records):
  - the declared names and their slot numbers;
  - whether the scope is **dynamic**: it contains a sloppy direct `eval`, it is inside
    a `with` body, or it is the global scope. Global bindings stay property-based
    (§9.1.1.4 Global Environment Records), behind a per-name cache.
  - for each `Name` reference: `Local(slot)`, `Scoped(hops, slot)`, `Global(name)` or
    `Dynamic(name)`.
- `Object::Environment` gets `slots: Vec<Option<Value>>` (`None` = TDZ) beside the
  existing maps. Static scopes use only slots; dynamic scopes keep today's maps. The
  `(hops, slot)` resolution is valid only when no dynamic scope lies between the use and
  the declaration. The resolver checks that, and otherwise emits `Dynamic`.
- Per-iteration `let` copies (§14.7.4.4) become a `Vec` clone of the loop scope's slots,
  with no hashing. Closures capture the environment `Id` as today, so their semantics do
  not change.
- Mapped arguments alias `(env, slot)` instead of `(env, name)`.

### Stage 3: register bytecode

- `Chunk { code: Vec<Insn>, constants: Vec<Value>, names: Vec<JsString>,
  functions: Vec<Rc<FunctionTemplate>>, handlers: Vec<Handler>, registers: u16 }`,
  compiled lazily per function on its first call and cached in the closure's template.
- `enum Insn` uses three-address `u16` register operands, for example
  `Add { dst, a, b }`, `GetProp { dst, obj, key: NameIdx }`,
  `Call { dst, callee, this, args: Range }`, `Jump`, `JumpIfFalse`, `GetSlot { dst, hops, slot }`.
  Exception handlers live in a side table (pc range → handler pc, register depth), as in
  Lua and SpiderMonkey's try notes, so the fast path does not check for them.
- **Frames are data**: `Frame { chunk: Rc<Chunk>, pc, base, env: Id, this, new_target }`
  on a `Vec<Frame>`, with one `Vec<Value>` register stack shared by all frames.
  JS-to-JS calls do not recurse on the Rust stack. That also replaces the `call depth
  limit` guard (34 failures today, `tco` tests) with a heap-sized limit. Generators and
  async functions suspend by moving their frame and register window into the
  generator object, which retires `Activation`/`Op` and the `"\0async{r}"` bindings.
- **GC rooting**: at a safepoint, `maybe_collect` passes the live register stack, plus
  every frame's `env` and `this`, as extra seeds to `collect_joint` (the same entry the
  DOM uses for native pins). No `ExternalRoot` per temporary. Correctness argument: a
  collection happens only inside `allocate`, and every VM value live at that moment is
  in the register stack or a frame. Native code called from the VM keeps today's
  `protect` discipline. `GcPolicy::Stress` keeps exercising every safepoint.
- **Step guard**: tick on backward jumps and calls, which bounds every loop and
  recursion. The test limit gets recalibrated so that the gated profiles keep their
  result.
- **Abstract operations are reused**: `Insn` handlers call the existing `Engine` methods.
  Hot paths get fast cases before the generic call: Number + Number, dense-element
  get/put, own data property on an ordinary object.

### Stage 3 requirements from literature check #336 (2026-11-10)

Verdict: adopt the register VM and the staged plan. Requirements added to stage 3:
- Register allocation with reuse from day one (a freelist or live ranges); log the maximum
  registers per function over test262 as a metric.
- GC seeds scan only the live window `[0, top)`; registers are cleared at block and loop exit.
  Tests: a Stress-mode case per opcode family where the handler's abstract operation allocates
  while its operands are live, and a dead register must not keep a WeakRef target alive.
- A suspended generator's register window is traced as explicit edges of the generator object.
- Resolver: "maybe direct eval" is decided by name with a run-time check; the dynamic marking
  covers the containing function's var environment and every scope between it and the eval.
  Gate: test262 `language/eval-code`, `language/statements/with`, `language/arguments-object`,
  `annexB`.
- Mapped arguments on `(env, slot)`: explicit cases for freeze/defineProperty breaking the
  mapping, aliasing after return, and captured formals.
- Differential gate: besides `auto`, run `--engine=vm` with fallback as an error, and report
  the fallback count at each checkpoint.
- Optional: a stack-vs-register micro-comparison on `loop_let` at the first coverage step
  (expressions only), so the choice is measured in this engine.

### Stage 4 (later, own note): shapes and inline caches

Hidden classes and per-instruction inline caches for `GetProp`/`SetProp` (Deutsch &
Schiffman 1984; Hölzle et al. 1991). They need the stable instruction sites that stage 3
creates. They are out of scope here.

## Migration without losing passes

1. **Per-function fallback.** The compiler returns `Unsupported` for any construct it
   does not handle yet, and that function keeps running on the tree-walker. Both
   evaluators share environments, closures and the abstract operations, so they can call
   each other. Coverage grows construct by construct. This never parses or evaluates
   something as something close, because the fallback is the existing correct path.
2. **Differential gate.** `js_check --engine=tree|vm|auto` (auto = VM with fallback). For
   each stage: the gated profiles in both modes, plus a full test262 run in `auto` mode
   compared against the previous full run with `compare_runs.py`. Zero lost cases is the
   bar, as at every checkpoint.
3. **Order of stage-3 coverage**, chosen by benchmark weight: expressions and
   operators, `var`/`let`/`const` slots, `if`/`for`/`while`, calls and returns, object
   and array literals, member get/set, `try`/`throw`, closures, then classes, destructuring
   and spread, generators and async (retiring `asynchronous.rs`), then `with` and eval
   (dynamic path).
4. When `auto` reports zero fallbacks on test262 and on the Servo smoke suite, the
   tree-walker evaluator is deleted. The parser and AST stay, as the compiler input.

## Kill lines

- Stage 1: if the benchmark set gains less than 1.5x on fib or on loop_let, stop and
  re-profile before stage 2.
- Stage 2: target ≤ 15k instructions per loop_let iteration. After 3 fix rounds without
  reaching it, write a literature-check request on the environment representation.
- Stage 3: target ≤ 5k instructions per iteration and the property-escape bucket under
  the 10 s budget (≥ 400 of 441). If the first 3 coverage steps show < 3x over stage 2 on
  loop_let, stop and reconsider O4 or a stack design with Literature Review (#82).

## References

- ECMA-262 2026: §9.1 Environment Records, §10.2.11 FunctionDeclarationInstantiation,
  §10.4.4 Arguments exotic objects, §14.7.4.4 CreatePerIterationEnvironment,
  §19.2.1.3 EvalDeclarationInstantiation, §10.1.11.1 OrdinaryOwnPropertyKeys.
- Y. Shi, D. Gregg, A. Beatty, M. A. Ertl, "Virtual Machine Showdown: Stack Versus
  Registers", VEE 2005.
- R. Ierusalimschy, L. H. de Figueiredo, W. Celes, "The Implementation of Lua 5.0",
  J. UCS 11(7), 2005 (register VM, upvalues).
- V8 Ignition design (accumulator register machine), 2016. Hermes bytecode (register
  VM, AOT). QuickJS and SpiderMonkey (stack bytecode). Boa (Rust) moved from a stack VM
  to a register VM in v0.21; Kiesel (register VM over a live-ranges IR) and LibJS (from an
  accumulator format to explicit src/dst registers) converged on registers too
  (literature check #336, Literature Review #82, 2026-11-10).
- L. P. Deutsch, A. M. Schiffman, "Efficient Implementation of the Smalltalk-80 System",
  POPL 1984 (inline caches). U. Hölzle, C. Chambers, D. Ungar, "Optimizing
  Dynamically-Typed Object-Oriented Languages With Polymorphic Inline Caches",
  ECOOP 1991.

## Results

Per stage: commit, instruction counts (callgrind, load-independent) and test262 delta.
Wall times on this machine are unreliable while other jobs run (load average 3+), so the
kill lines are judged on instruction counts.

### Stage 1 (checkpoint 21)

Done:
- Closure `params`/`body` are `Rc<Vec<_>>`; `invoke_inner` copies only the callee fields
  a call needs, in an out-of-line helper (`Engine::callee`), so debug stack frames on the
  recursive call path do not grow (the `recursive_closure_is_bounded` test overflowed
  its 2 MB test-thread stack when the temporaries sat in `invoke_inner`).
- Lazy `arguments`: `Object::Closure::needs_arguments`, computed once in
  `make_function`, skips the object in `function_environment`.
- Shared loop environment: `for (let ...)` skips CreatePerIterationEnvironment
  (§14.7.4.4) when init, test, update and body contain nothing that can capture a
  binding (function, arrow, class, method, accessor, direct `eval`).
- Both predicates are conservative scans of the AST's derived `Debug` text
  (`src/script/ast_scan.rs`, stops at the first match). A false positive only keeps the
  spec-literal path. The stage-2 resolver replaces them with real scope analysis.

| case (small variant) | baseline 9acf139 | stage 1 | ratio |
|---|---:|---:|---:|
| `fib_small.js` (fib(16)) | 256.9 M instr | 114.8 M | 2.24x |
| `loop_small.js` (20k iterations) | 1,192.0 M | 870.2 M | 1.37x |

Kill line (≥ 1.5x on fib or loop_let): met on fib. The loop is now dominated by the
element store (`assign_member` → `ordinary_set` → `array_define`, 44% of instructions,
about 19k per store): a new index walks the prototype chain through the generic
[[GetOwnProperty]] (proxy, namespace and typed-array checks per level) and hashes
string keys with SipHash. Dense elements and interned keys were left for stage 4
(shapes and element kinds), where the "no indexed properties on the prototype chain"
invariant they need belongs. test262 delta: see checkpoint 21 in ESTADO.md.

### Stage 2, part 1 (checkpoint 22): hot-path costs before the resolver

The stage-1 profile put the element store and hashing first, so these came before the
resolver. Each step was measured on its own (callgrind, `loop_small.js`, 20k
iterations):

| step | loop_small | per iteration |
|---|---:|---:|
| stage 1 (checkpoint 21) | 869.0 M | 43.4 k |
| `plain_set`: OrdinarySet fast path | 667.5 M | 33.4 k |
| seeded key hasher in `PropertyMap` | 617.3 M | 30.9 k |
| no block environment for blocks without lexical declarations | 516.0 M | 25.8 k |
| `NameMap`/`NameSet` (same hasher) in environments | 473.5 M | 23.7 k |
| `scan_level`: one object fetch per scope level in lookup | 470.1 M | 23.5 k |

`fib_small.js`: 114.8 M → 111.0 M. Overall 1.85x on loop_small over stage 1.

- `plain_set` (`objects.rs`) implements OrdinarySet §10.1.9.2 and
  OrdinarySetWithOwnDescriptor §10.1.9.2.1 for the case where the receiver is the
  target and every object on the prototype chain is ordinary data (no proxy, typed
  array, module namespace, mapped arguments, String exotic or accessor on the path).
  An own writable data property is replaced in place. An absent or inherited writable
  data property is created on an extensible receiver. Array indices follow
  §10.4.2.1 (index ≥ length needs a writable `length`, which is then bumped). Anything
  else returns `None` and the generic path runs, so the fast path never decides an
  edge case by itself.
- Key hasher (`props.rs`): an FxHash-style word hasher with a murmur finaliser
  replaces SipHash. **HashDoS trade-off:** the seed comes from `RandomState` once per
  process, so keys that collide cannot be precomputed offline, but the hasher is not
  keyed-cryptographic like SipHash. A page that can measure timing could in principle
  search for collisions in its own process. That is acceptable for the lab. Before the
  Servo path relies on it (roadmap D), shapes (stage 4) remove most string hashing, and
  the remaining dictionary-mode maps can switch back to SipHash at a measured cost.
- Block environments (`execute.rs`, `scoped`): Block evaluation §14.2.2 creates a
  declarative environment and runs BlockDeclarationInstantiation §14.2.3. With no
  lexically scoped declarations in the block's direct statements, that environment is
  empty and unobservable, so it is skipped. The check is a whitelist of statement
  kinds: anything else keeps the environment.
- `scan_level` (`vm.rs`): GetIdentifierReference §9.1.2.1 walks outward, one level per
  step. The walk now reads each environment object once and skips the empty alias and
  object-binding maps.

Kill line (≤ 15k per iteration): not met yet (23.5k). The remaining profile:
malloc/free about 20% (23 allocations per iteration: key strings for array indices in
`ordinary_get`, `to_string`, the ArraySetLength truncation loop and `argument_list`),
`ordinary_set` 6.5k (half of it `codePoints.length = 0` truncation),
`eval_reference` 6.3k and `lookup_with_base` 2.9k. Next: index keys without string
allocation, then the resolver (slot environments) as designed above. This counts as
fix round 1 of 3 toward the stage-2 kill line.

### Stage 2, round 2 (checkpoint 23): allocations and the reference walk

Same benchmark and method (callgrind, `loop_small.js`, 20k iterations):

| step | loop_small | per iteration |
|---|---:|---:|
| part 1 (checkpoint 22) | 470.1 M | 23.5 k |
| round 2, all steps below | 319.1 M | 16.0 k |

`fib_small.js`: 111.0 M → 105.6 M.

- `initialize` updates an existing binding in place, with no name allocation
  (InitializeBinding §9.1.1.1.4 only sets the value and the initialized flag).
- `ordinary_get` reads `plain` objects level by level in place (OrdinaryGet §10.1.8.1,
  same proto-chain eligibility rule as `plain_set`).
- `plain_set` takes the key by value; the `"length"` key is a thread-local constant.
- ArraySetLength §10.4.2.4 step 19 deletes the indices ≥ newLen in one pass over the
  own keys when more than 8 would be deleted, instead of one `[[Delete]]` per index.
  The result is the same as the descending loop: the highest non-configurable index
  ≥ newLen stops the deletion, everything above it goes, and `length` becomes that
  index + 1. Deleting a configurable property of an ordinary array runs no code.
- CreateListFromArrayLike §7.3.18 (`argument_list`) reuses one key buffer and reads
  own data properties directly when the object is plain.
- `eval_reference` resolves a name in one walk (`Engine::resolve_name`), replacing the
  "unresolvable?" check followed by `with_base`. Aliases and object bindings are the
  outermost levels, so the walk order of GetIdentifierReference §9.1.2.1 is kept.
- `Reference<'a>` borrows the AST name instead of cloning it.
- Number::toString §6.1.6.1.20: integers in the safe range take a digit fast path,
  and `to_string(Number)` skips the `JsString` round trip.

Kill line (≤ 15k per iteration): not met yet (16.0k). This is fix round 2 of 3.
Remaining hot spots: `plain_set` 2.3k, `lookup_with_base` 2.0k, `initialize` 1.1k
(link/unlink and two lookups), malloc/free about 10%. If round 3 misses 15k, the next
step is a literature check on the environment representation, then the resolver
(slot environments) as designed above.

### Stage 2, round 3 (checkpoint 24): clones and calls on the store path

Same benchmark and method (callgrind, `loop_small.js`, 20k iterations):

| step | loop_small | per iteration |
|---|---:|---:|
| round 2 (checkpoint 23) | 319.1 M | 16.0 k |
| `has_direct_using` without a Vec, ToPrimitive step 1 inline, owned key in `assign_member` | 303.9 M | 15.2 k |
| property index keyed by hash (no key clone per new property) | 294.4 M | 14.7 k |

`fib_small.js`: 105.6 M → 103.4 M.

- `has_direct_using` ran on every block entry and collected `direct_statements` into a
  `Vec` first (≈ 200 instructions per iteration). It now walks the same statements
  without collecting.
- ToPrimitive §7.1.1 step 1 (a non-object is returned unchanged) is an inline check;
  the object path (`@@toPrimitive`, OrdinaryToPrimitive) stays out of line. `to_number`
  and the binary operators called it about 9 times per iteration.
- `assign_member` (PutValue §6.2.5.6 on a property reference) owns its key and passes it
  by value to OrdinarySet, so a new property no longer clones the key string.
- `PropertyMap`'s index (objects with more than 8 live keys) maps the seeded key hash to
  entry positions, `One(at)` or `Many(Vec)` on a collision, and compares against the
  stored entry. Appending a property no longer clones its key into the index. Insertion
  order, tombstones and compaction are unchanged (PROPERTIES.md).

Kill line (≤ 15k per iteration): **met** at 14.7k in fix round 3. The property-escape
timeouts need more than another 1.1x: they need stage 3 (register bytecode, target ≤ 5k per
iteration), which waits on the literature check and the second opinion. The resolver with
slot environments (above) stays the first stage 3 prerequisite: bytecode operands need
`(hops, slot)` resolution.

### Property-escape timeouts re-measured (2026-11-10, after checkpoint 30)

Question: is the RegExp matcher or the interpreter what keeps 54 `property-escapes/generated`
cases at the 10 s timeout under 2 workers (t262-full37)?

- Split on `\P{ASCII}` (2,162,560 code units): `buildString` from `regExpUtils.js` takes
  **8,639 ms**, the `^\P{ASCII}+$` `test` takes **35 ms**. The matcher's `Star` loop is not the
  bottleneck; the 1.1 M-iteration JS loop in `buildString` is (99.6 %).
- Loop shapes (wall, 1.1 M iterations, machine load ~13): empty `let` loop 3.3 s, `var` loop
  3.0 s, the `buildString` body (`a[l++] = i` plus the chunk reset) 7.4 s.
  `String.fromCodePoint.apply` on 10,000 args ×110 is 0.5 s; 110 rope appends are 0.1 s.
- Callgrind, `function f() { let a = []; for (let l = 0, i = 0; i < 100000; i++) { a[l++] = i; … } }`:
  1,259 M Ir, **12.6 k per iteration**. Self cost is spread out: `scan_level` 9.5 %,
  `eval_inner` 7.5 %, `reference_set` 6.6 %, per-iteration environment `initialize` 4.4 % and
  `Frame::drop` 3.8 %, `PropertyMap::position` 4.3 %, String-keyed hashbrown lookups ~6 %,
  and `Expr::Number` re-parsing its literal text on every evaluation (`dec2flt` 1.6 %,
  `execute.rs` `Expr::Number(n) => n.parse()`). No single function goes above 10 %.
- Fixed set of 10 timed-out cases (pe-bench in the task workspace): 76.8 s wall alone, all
  passing, 3.6 to 11.5 s each.

Conclusion: no regex-matcher work is warranted. The cases need roughly 3x to 5x on the plain loop to be
stable under parallel load, which is the stage 3 target (≤ 5 k per iteration) and not another
round of stage 2 fixes (whose kill line was met at checkpoint 24). Number literals should be
parsed once at parse time as part of stage 3's constant pool. Stage 3 was gated on a
second opinion (taskygpt HTTP 429, no credits); Santiago waived that gate on 2026-11-10.

### Stage 3, coverage step 1 (2026-11-10, checkpoint 32 queue)

What landed (`src/script/bytecode.rs` compiler, `src/script/interp.rs` executor):
- Register bytecode for **leaf functions** with simple parameters that do not need an
  `arguments` object. The compiler covers expressions, `var`/`let`/`const` with static TDZ
  checks, `if`, `while`, `do`/`while`, `for`, labels, `break`/`continue`, `return` (with
  §15.10 tail calls through the existing `defer_tail_call` trampoline), `throw`, array and
  plain object literals, member get/set/delete, calls and `new`.
- Anything else is rejected with a reason and the function runs on the tree-walker.
  **Deviation from the requirement above:** the fallback is silent (counted per reason in
  `VmStats`, printed by `js_check --vm-stats`) instead of an error under `--engine=vm`,
  because the harness itself (`assert.sameValue`, `assert.throws`) uses `try`. The fallback
  stays silent until `try` is compiled.
- **Hatch:** a subexpression that mentions none of the function's own bindings runs on the
  tree-walker against the function environment (one `Insn::Hatch`). That keeps rare
  syntax (templates, regex literals, class expressions) from rejecting a whole function.
- **Registers** are allocated with reuse (each statement frees its temporaries) and are
  cleared at loop back-edges and block exit. The register window is mirrored into
  `Runtime::vm_seeds`, which `root_ids()` returns as GC seeds, so a collection inside any
  handler sees every live register.
- Chunks are cached by the body's `Rc` pointer with a `Weak` check (the cache is pruned at
  4096 entries). Known limit: closures created per instance share the body, so they compile once.
- `ToPropertyKey` on a primitive has no side effects (§7.1.19), so `PropKey` keeps the raw
  primitive and lets `GetElem`/`SetElem` take the existing fast index path.
- Number literals are parsed once into the constant pool.
- The engine mode is selected with `js_check --engine=vm` or `MWR_ENGINE=vm`. The default stays
  `Tree` until the checkpoint gate below passes with the VM.

Spec fix found by the VM work: the tree-walker's logical assignment (`a ??= function(){}`)
did not apply NamedEvaluation (§13.15.2 steps 5.a / 6.a / 7.a). It now does in
`references.rs`, and the VM matches it. `language/expressions/logical-assignment` is 81/81.

Measurements (prof build, `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`):

| loop2.js, 100 k iterations, `a[l++] = i` body | Ir total | Ir / iteration | wall |
|---|---|---|---|
| empty script | 20.0 M | | |
| tree-walker | 1,261.9 M | 12.42 k | 0.18 s |
| VM, first build | 689.2 M | 6.69 k (1.86x) | 0.10 s |
| VM, raw primitive `PropKey` | 615.9 M | 5.96 k (2.08x) | 0.07 s |
| VM, `finish` pass + register updates | 475.2 M | **4.55 k (2.73x)** | |

After the first build the profile was half VM overhead: 27 dispatches per iteration of
`loop2`, of which 3 were per-statement `Clear`s, 6 were `Move`s and 2 were `Const`s.
Two compiler changes brought that down to 13 dispatches:
- A `finish` pass after compilation computes, flow-insensitively, which registers may ever hold
  an Object or Native value (parameters plus every instruction that can write one;
  operators, `ToNumeric`, `Step`, `PropKey`, `typeof` and `delete` write primitives). It drops
  each `Clear` whose range has none, because clearing exists only to end GC liveness. It also
  drops the final copy into a discarded statement result, and remaps jump targets.
- `x++` / `--x` on a ready mutable register binding steps the register in place
  (§13.4.2.1, §13.4.4.1). As an expression statement or a `for` update, a postfix update
  becomes a prefix one (same effects), and `Step` applies ToNumeric itself.
  `js_check` prints a chunk's code with `MWR_VM_DUMP=1`.

The rest of the per-iteration cost is the shared object model. An index key goes
through `to_key` (number → string), `PropertyMap::position` (a hash lookup) and `plain_set`.
`assign_member` is 2.6 k Ir per iteration inclusive (57 % of the total, amortising the
`a.length = 0` truncation every 10 k stores). Dense element storage is the stage 4 object-model note, not a
VM item.

Equivalence: the same 2,500-case smoke sample over `language/` and `built-ins/` gives
`passed=2235 failed=251 unsupported=14` on both engines, with no case whose verdict differs.
`vmprobe/basic.js` (task workspace) passes under `--gc-stress`. Max registers on that sample: 9.

Next coverage steps: (1) `try`/`catch`/`finally` through a handler side table, which removes the
largest fallback reason and lets the fallback become an error under `--engine=vm`; (2) closures
(non-leaf functions) via the stage 2 slot environments; (3) dense-element get/put fast paths.
Kill line (unchanged): under 3x on loop2 after three coverage steps sends the design back to
Literature Review (#82).

### Stage 3, coverage step 2: try/catch/finally (2026-11-10, checkpoint 32 queue)

TryStatement (§14.15.3) compiles to straight-line code plus a handler side table in the chunk
(`Chunk::handlers`, innermost first). A `ScriptError::Thrown` raised at `pc` inside
`start <= pc < end` stores the thrown value in the handler's `exc` register. For a finally, it
also sets the completion-kind register to 1, then jumps to `target`. Limits, Unsupported,
Memory and host errors are not catchable, the same as in the tree-walker. The dispatch loop is
now `execute_from(.., &mut pc)` inside an outer loop that looks up the handler.

Layout: `try-block; Jump normal; catch-block; normal: kind = 0; fin: finally-block;
EndFinally{kind, val, to: end}; return-path; end:`. A `return` inside a try or catch that has a
finally stores the value in `val`, sets `kind = 2` and jumps to `fin`. `EndFinally` continues
for 0, rethrows `val` for 1, and for 2 falls through to the return path. That path is a
`Return` or, through a nested finally, the same forwarding to the outer one. The catch parameter
is a scope binding whose register is `exc`, so its own scope is separate from the catch block's
(§14.15.2). Calls in a try block, or in a catch that has a finally, are not tail calls (§15.10.2):
the compiler keeps a `non_tail` counter. `finish` remaps the handler ranges and the `EndFinally`
targets, and marks `exc` registers as possible references.

Still on the tree-walker (fallback reasons): break/continue that crosses a finally, destructuring
catch parameters, and a catch block that declares `var <param>` (Annex B.3.4).

Gate: `vmprobe/try.js` (21 cases: return through nested finally, finally overriding return or
throw, rethrow, a catch in a loop with `continue`, catch scope shadowing, optional catch binding)
passes on both engines and under `--gc-stress`. Equivalence is identical: the 2,500-case sample
gives `passed=2235 failed=251 unsupported=14` on both engines, and `language/statements/try`
gives 200/201 on both. Fallback mix over 600 sampled cases (`vmstats.sh`, task workspace):
89.7 % of function bodies compiled. The remaining reasons are `arguments or eval` 214,
`closure over locals` 73, `for-in/of` 60, `non-simple parameter` 33, `template` 33, and
`switch` 16; `vm: try` no longer appears.

### Stage 3, coverage steps 3-4: closures and the arguments object (2026-11-10)

**Closures (step 3).** The compiler splits the function's own bindings into two classes:
- *Register-resident*: the default, as before.
- *Environment-resident* (`Compiler::captured`): every declared name (parameter, `var`, top-level
  `let`/`const`) mentioned inside a closure subtree of the body, plus every top-level function
  declaration's name. `ast_scan::captured_names` finds the closure subtrees (arrow, function, async,
  generator, class, accessor and class-init variants) in the body's `Debug` text, skipping string
  literals, and looks for each name quoted inside them. A false positive only moves a binding to its
  environment record; it cannot change semantics.

An environment-resident name gets no register. Reads and writes compile as free names
(`GetName` / `ResolveName` + `SetRef`) against the function environment that
FunctionDeclarationInstantiation (§10.2.11) has already populated. `var` bindings start as
undefined, lexical ones are in their TDZ, and parameters live in the parameter environment. A
top-level `let`/`const` initializer ends in `InitName` (InitializeBinding, §9.1.1.1.4). TDZ and `const`
errors come from the environment record itself. A closure expression then mentions no register
binding, so it runs through the existing `Hatch` (the tree-walker creates the function object over
the function environment, and NamedEvaluation still applies through the hatch's name). Top-level
function declarations are kept in `Chunk::hoist` and instantiated at entry with `hoist_functions`,
before parameters are copied into registers (a declaration can shadow a parameter).

Still on the tree-walker: a captured binding declared in a nested block, a `for (let …)` head or a
catch parameter (`vm: captured block lexical`). Those need a fresh environment per block
evaluation or per iteration (CreatePerIterationEnvironment, §14.7.4.4), which is a separate step.
Function declarations in nested blocks (Annex B.3.3) also stay on the tree-walker.

**Arguments object (step 4).** `arguments` alone no longer rejects a body: only a direct `eval` or
the derived default constructor's `%forward` do (`vm: eval`). The `arguments` object already
exists in the environment (§10.2.11 steps 15-22), so the name is environment-resident. In a sloppy
function with simple parameters the object is mapped (§10.4.4.7) and aliases the parameter bindings,
so all its parameters become environment-resident as well. A strict function's unmapped object
leaves its parameters in registers.

Spec fix found by the probes (both engines): `Object.freeze` mutated the property map directly and
skipped the mapped-arguments [[DefineOwnProperty]] (§10.4.4.2), so a frozen `arguments` stayed linked
to its parameters. SetIntegrityLevel (§7.3.15) now defines each mapped index through `define`, which
takes the current mapped value and removes the mapping.

Gate: `vmprobe/closure.js` (22 cases: counters, parameter written after capture, hoisting order,
function vs param/var, captured TDZ and const, class and accessor captures, NamedEvaluation) and
`vmprobe/arguments.js` (13 cases: mapping, short argc, strict unmapped, arrow `arguments`,
`var arguments`, defineProperty/freeze breaking the map, captured formal) pass on both engines and
under `--gc-stress`. The closure-heavy directories (function-code, function and arrow
expressions and statements, class definition, block-scope) give 400/400 on both engines.
`language/arguments-object` and `built-ins/Object/{freeze,seal,isFrozen}` give 469/469 on both
engines. The 2,500-case equivalence sample is still identical (`passed=2235 failed=251
unsupported=14` on both). Fallback mix over the same 600 sampled cases: **96.1 %** of bodies compiled
(89.7 % after step 2, 91.6 % after closures alone). The remaining reasons are `for-in/of` 60,
`non-simple parameter` 33, `template` 33, `eval` 18, `switch` 16 and `captured block lexical` 5.

### Stage 3, coverage step 5: for-in and for-of (2026-11-10)

ForInOfStatement (§14.7.5.5-§14.7.5.7) compiles to four instructions:

```
  iterable -> src; GetIterator iter, next <- src (enumeration?)
top:
  IterNext value <- iter.next() ; done -> exit          (outside the handler)
  [handler start] bind value; body [handler end]
  Loop top
  handler: IterCloseThrow iter, exc                     (for-of only)
exit:
```

- `GetIterator` is §14.7.5.6 steps 6-7: EnumerateObjectProperties (§14.7.5.9, the engine's
  `%ForInIteratorPrototype%` cursor, `null`/`undefined` give an empty one) or GetIterator (§7.4.3).
- `IterNext` is §14.7.5.7 steps 6.a-6.d, with the same "iterator result is not object" TypeError as
  the tree-walker.
- The handler covers only the binding step and the body. An abrupt binding or body closes the
  iterator (step 6.i and 6.l), and a throw from `next()` does not. Only for-of has a handler:
  for-in returns the completion without IteratorClose.
- `IterCloseThrow` is IteratorClose (§7.4.11) with a throw completion. Errors from `return` are
  swallowed and the original value is rethrown.
- `break` out of a for-of goes through `exit_to`, and so does a labeled break to an outer statement.
  Either way it emits `IterClose` for every for-of loop it leaves, innermost first. `continue` keeps
  its own loop's iterator. `IterClose` clears the iterator register before calling `return`. So
  when `return` throws, the loop's own handler finds the register empty and only rethrows. Each
  outer for-of still closes its iterator, which matches the spec's chain of completions.
- `return` emits `IterClose` for the for-of loops between it and the innermost pending finally
  (`finally_depth == finally.len()`), then goes on as before. The finally's return path calls
  `return_value` again once the finally is popped, and that closes the loops outside it. So
  `try { for (x of it) try { return } finally { A } } finally { B }` runs A, closes `it`, then
  runs B, as the tree-walker does.
- Bindings: a `let`/`const` name gets a register scope pushed before the iterable is compiled, so
  `for (let x of x)` reads hit `Tdz` (§14.7.5.6 step 2). Each iteration writes the register, which
  is the per-iteration copy, because a captured name is rejected (`captured block lexical`).
  - `var` and assignment names go through `store_name`: a register Move, or ResolveName/SetRef
    after the value, as in step 6.h.i.
  - Member and computed assignment targets evaluate their reference after IteratorValue.
  - Destructuring heads (`vm: for-in/of destructuring`), `using` heads and `for await` stay on
    the tree-walker.
- Like the tree-walker, neither loop's body is a tail position (`non_tail`), because the iterator
  still closes after a `return`.

Probe `vmprobe/forof.js` (28 cases) passes on both engines and under `--gc-stress`, and all 65
of its bodies compile. It covers:
- close order on break, labeled break, return and throw;
- `return` throwing after break;
- a non-object `return` result;
- a `next` throw that must not close;
- `continue outer`;
- return through nested finally blocks;
- TDZ and const assignment in the head;
- member and computed targets;
- the Annex B initializer;
- deletion during for-in.

Gate results:
- `language/statements/for-of` and `for-in`: 870 cases, identical on both engines (843 passed, 27
  unsupported).
- 2,500-case equivalence sample: identical except
  `ArrayBuffer/prototype/sliceToImmutable/argument-coercion.js`. That case is load-sensitive: it
  takes 7-15 s on either engine under the machine's foreign load, and in full39 it passed only on
  retry.
- Fallback mix over the 600 sampled cases: **97.3 %** of bodies compiled (up from 96.1 %).
  Remaining reasons: `template` 40, `non-simple parameter` 33, `eval` 18, `switch` 16,
  `captured block lexical` 5, `for-in/of destructuring` 2, `destructuring declaration` 2.
