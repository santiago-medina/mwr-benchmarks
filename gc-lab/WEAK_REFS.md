# Weak references: WeakMap, WeakSet, WeakRef, FinalizationRegistry

Status: design (gap 14). Spec: ECMA-262 §24.3 WeakMap Objects, §24.4 WeakSet Objects,
§26.1 WeakRef Objects, §26.2 FinalizationRegistry Objects, and the "Processing Model of
WeakRef and FinalizationRegistry Targets" clause (§9.9: Objectives, Liveness, Execution,
Host Hooks), with AddToKeptObjects, ClearKeptObjects, CleanupFinalizationRegistry and
CanBeHeldWeakly.

## What the current code constrains

- Tracing lives in `dual::Runtime`: every object has a `Record { payload, links }`.
  `Runtime::reachable` marks from the roots over `links` only. The two `Heap` arenas then
  sweep with the computed live set as their roots (`collect_with_roots`). Runtime is the
  only place that knows liveness, so weak semantics must live there too.
- The engine mirrors object state in `Engine::objects` (`Data`). `replace_data` keeps
  `links` equal to `Data::edges()` (unlink old, link new). Every value a `Data` holds
  through `Data::edges()` is strong.
- IDs carry an identity number: a dead `Id` never resolves again (`Runtime::value` errs),
  even when the slot is reused. So a stale weak `Id` cannot alias a new object. That makes
  the "clear after sweep" step safe even if it runs late.
- `prune_metadata` runs after every collection (explicit, threshold, budget, native).
  It is the single post-GC hook in the engine.
- Symbols are `Value::Symbol(u64)` and are never collected. A symbol used as a weak key
  (symbols-as-weakmap-keys) therefore never dies. That is allowed: liveness is
  implementation-approximated, and a key that is never observed dead is conforming.

## Options considered

1. **Strong edges plus a manual "clear" API.** This gives the wrong semantics (a WeakMap
   keeps its keys alive) and is observable through FinalizationRegistry and WeakRef.
   Rejected.
2. **Iterated fixpoint over all ephemeron tables** (the naive algorithm: re-scan every
   table until no new value gets marked). It is simple, but O(tables × rounds) in the worst
   case (a chain of maps keyed on each other's values is quadratic).
3. **Ephemerons keyed by key (chosen).** While marking, each ephemeron `(k, v)` of a
   marked container either pushes `v` (when `k` is already marked) or parks `v` in a
   `waiting[k]` list. Marking `k` later drains `waiting[k]`. Every ephemeron is processed
   at most twice, so marking stays linear in edges. Ephemerons come from Hayes 1997
   ("Ephemerons: a new finalization mechanism"). The key-indexed O(L+W) scheme is the one
   Felix described on es-discuss (2011, a `guardedby[k]` list per key), and SpiderMonkey's
   weak-key tables (`js/src/gc/WeakMap.h`) mark the same way. V8 does not: it runs a
   fixpoint with a full-rescan fallback.
   A single marking point drains `waiting[id]` (the `live.insert` branch), so it does not
   matter which route marks a key: a root, a KeptAlive or native root, another
   ephemeron's value, or a key reached only after its container. Skipping that drain on
   one route is the bug class of V8's CVE-2021-37975.

## Design

### Runtime (dual.rs)

- `Record` gains `weak: Vec<Id>` (targets that are not traced) and
  `ephemerons: Vec<(Id, Id)>` (key, value: the value is traced only when the container
  AND the key are live).
- `Runtime::set_weak(id, weak, ephemerons)` replaces both lists. It is not a safepoint and
  either succeeds or fails without mutation, so the budget invariant holds. Weak entries
  are charged like edges (`edge_charge` each, a pair counts as two), and
  `recount_charge` includes them.
- `reachable` implements option 3. It marks strong links, then ephemerons, through
  `waiting: IdMap<Vec<Id>>`.
- After the sweep, each live record keeps only the weak targets and ephemerons whose key
  is live, and the charge is adjusted. Dead keys are dropped in the Runtime first. The
  engine then scrubs its `Data` (below).

### Engine data

| Object | Data slot | Strong edges | Weak / ephemeron |
| --- | --- | --- | --- |
| WeakMap | `weak: WeakTable{set:false}` | values under Symbol keys | `(key, value.id())` per object key; key only if the value is primitive |
| WeakSet | `weak: WeakTable{set:true}` | none | each object key |
| WeakRef | `weak_ref: Option<WeakKey>` | none | target object |
| FinalizationRegistry | `registry: Registry{cleanup, cells}` | cleanup callback, each cell's holdings | each cell's target and unregister token |

`WeakKey` is `Object(Id) | Symbol(u64)`, the only values CanBeHeldWeakly accepts (an
object, or a symbol that `Symbol.keyFor` does not know). `Data::edges()` returns only the
strong column. A new `Data::weak_edges() -> (Vec<Id>, Vec<(Id, Id)>)` returns the rest,
and `replace_data` calls `runtime.set_weak` when it is not empty (or when it was before).
`Engine::weak_holders: IdSet` tracks every object with weak slots, so the post-GC scrub
does not scan the whole heap.

### After a collection (`prune_metadata`)

For each live holder in `weak_holders` (dead holders are dropped):
- WeakMap/WeakSet: remove the entries whose object key is dead. This mutates in place
  without relinking, because the strong edges do not change.
- WeakRef: when the target is dead, set it to empty. `deref` then returns undefined.
- FinalizationRegistry: when a cell's target is dead, set `cell.target = empty`. When its
  unregister token is dead, also set `token = empty`. §9.9.3 has no such step, but the
  token is held weakly (§26.2.3.2), and a dead token can never be passed to `unregister`
  again, so clearing it cannot be observed. It only releases memory. The holdings stay strong in the
  registry until cleanup. If any cell became empty, call
  HostEnqueueFinalizationRegistryCleanupJob: the registry is rooted in
  `Engine::pending_cleanup`.

### KeptAlive (AddToKeptObjects / ClearKeptObjects)

`new WeakRef(t)` and `WeakRef.prototype.deref` add the target to `[[KeptAlive]]`. The
engine roots it (`runtime.root`) in `Engine::kept_alive`. ClearKeptObjects unroots all of
them. It runs at the end of `microtask_checkpoint`, which is where HTML performs it
("perform ClearKeptObjects()" in the microtask checkpoint), and in `release_globals`.
So within one synchronous run, a deref'd target cannot vanish. Known and accepted: a
synchronous loop that creates WeakRefs without ever reaching a checkpoint keeps every
target alive until the loop ends (the spec requires this). js_check performs a checkpoint
after the script, so the set is cleared before the cleanup task runs.

Conformance invariants of post-sweep clearing: nothing runs between the sweep and the end
of `prune_metadata` that could observe a weak slot, so the dead set is cleared atomically;
the WeakMap value goes in the same pass as its key; `[[KeptAlive]]` targets are runtime
roots, so every collection kind sees them; and no weak read path (`deref`, `get`/`has`/
`delete`, `register`/`unregister`, cleanup) ever sees a dead Id. Under `--gc-stress` a
collection runs inside these built-ins, and the smoke run passes there too.

### Cleanup job (CleanupFinalizationRegistry)

`Engine::run_finalization_cleanup()` takes the pending registries. For each one, it
loops "choose one cell with an empty target, remove it, call `cleanup(holdings)`" and re-reads
the cell list on every round. It never iterates a snapshot, so a callback that unregisters
another pending cell suppresses that cell's callback, and cells the callback registers are
seen (LibJS had a use-after-free iterating a snapshot here). An abrupt completion is
reported like an uncaught task error (HTML reports the exception). The other registries
still run, and a registry with empty cells left is queued again. The embedder runs it as a task after the microtask
checkpoint: js_check does so after draining microtasks, and the Servo bridge does so in
its task loop (a follow-up for roadmap D). It is never a microtask: the spec requires a
separate job, and running it inside the checkpoint would be observable.

### Methods (step by step in code)

WeakMap: constructor (§24.3.1.1, through AddEntriesFromIterable with `set`), delete,
get, has, set, getOrInsert, getOrInsertComputed (upsert). WeakSet: constructor, add,
delete, has. WeakRef: constructor, deref, @@toStringTag. FinalizationRegistry:
constructor (callable check), register (CanBeHeldWeakly target; target ≠ holdings via
SameValue; token is CanBeHeldWeakly or undefined), unregister (TypeError when the token
cannot be held weakly), @@toStringTag. CanBeHeldWeakly uses the symbol registry.

## Migration and safety

- No pass can be lost through existing objects: holders are new `Data` slots, and objects
  without weak slots keep `weak`/`ephemerons` empty, so `reachable` does the same work.
- GC tests: add `dual_heap` tests for an ephemeron chain (k1→map→v1=k2→…; the whole chain
  dies once the root goes), a WeakMap cycle (value references its key: it must die), a
  WeakRef cleared after the target dies, budget charge equality (`recount_charge`
  debug assertion), and stress-mode runs of the weak test262 smoke (`js_check --gc-stress`).
- Not covered: `$262.gc`-driven test262 cases (host-gc-required, 15 staging files) stay
  unsupported until the runner exposes a GC hook.
