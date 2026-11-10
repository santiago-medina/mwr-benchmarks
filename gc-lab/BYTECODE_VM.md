# Bytecode VM and interpreter throughput (roadmap B)

Status: design note, 2026-11-10. Nothing in it is implemented yet.
Before implementation starts it needs a literature check (Literature Review (#82)) and a
second opinion (`taskygpt`).

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
| O3'. Stack bytecode (QuickJS, SpiderMonkey, Boa) | about the same as O3 | large | more dispatches per operation; Shi et al. 2005 measured about 47 % fewer executed VM instructions for registers. Match-based dispatch makes the dispatch count matter more |
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
  VM, AOT). QuickJS, SpiderMonkey and Boa (stack bytecode; Boa is a Rust engine).
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
