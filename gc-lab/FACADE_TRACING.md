# Facade tracing: freeing cells Servo no longer holds

Status: design note, 2026-11-16. Lifts the C1 keep-alive policy
(MOZJS_FACADE.md "Cell lifetime"). This is the GC side of the binding API
between the engine and Servo (roadmap C), so it goes through a literature
check before the code lands.

## Problem

Today every `ObjectCell` is leaked and every object that ever got a cell is
reported as a root (`FacadeHost::trace_roots` reports all of `Shared.cells`).
So no object Servo has ever seen is collected: every DOM reflector, every
event listener closure, every promise passed to a WebIDL method stays alive
until the process exits. C1 pages live a few seconds. C2 (real pages,
long-lived tabs) cannot.

The target is SpiderMonkey's contract, which Servo's bindings already follow:

- An object is alive if a root reaches it. The roots are the context's
  `JS::Rooted` stack lists, the extra roots tracers
  (`JS_AddExtraGCRootsTracer`), the raw value roots (`AddRawValueRoot`), and the
  engine's own roots (running frames, the job queue, realm intrinsics).
- A host object's class `trace` hook reports the edges its native data holds
  (`Heap<T>` members of DOM structs).
- A dead host object's class `finalize` hook runs once, then its `JSObject*` is
  invalid. Holding a `JSObject*` that is neither rooted nor traced is a bug in the
  embedder.

## What Servo uses (survey of servo-src at the C1 pin)

| API | Uses | Where |
|---|---|---|
| `JS::Rooted<T>` stack lists (`RootingContext::stackRoots_`) | everywhere | `rooted!` in mozjs and Servo |
| `JS_AddExtraGCRootsTracer` | 2 | `trace_rust_roots` (script_runtime.rs: Servo's `STACK_ROOTS`, the script thread, `RootedTraceableBox` set); mozjs `trace_traceables` (rust.rs) |
| `AddRawValueRoot` | 1 | `PermanentRoot` (callback objects) - implemented |
| `CallObjectTracer` | 1 | script_bindings/trace.rs (`Heap<*mut JSObject>`, reflectors) |
| `CallValueTracer`, `CallStringTracer`, `CallScriptTracer` | 1 each | script/dom/bindings/trace.rs |
| `JSClassOps::trace` | ~110 `trace:` sites | generated DOM classes, WindowProxy |
| `JSClassOps::finalize` | 9 | DOM classes (`finalize_hook`, drops the boxed DOM struct), WindowProxy |
| `JS_SetGCCallback` | 1 | debug builds only (`debug_gc_callback`, asserts no GC during layout) |
| `JS_GC` | 8 | tests, `window.gc()`, memory pressure |
| `JS_UpdateWeakPointerAfterGC`, `JS_AddFinalizeCallback`, `PersistentRooted` | 0 | - |

So the facade needs four tracer glue functions (all stubs today), a facade
`JSTracer`, a walk of the Rooted lists, and a finalize path. No weak-pointer
API is needed for Servo.

## What the current code fixes in place

- **Object ids are never reused.** The facade engine runs `Model::Indirect`:
  a handle is (token index, serial) with a per-heap monotonic serial, and the
  collector moves objects but never re-tokens them. A dead `ObjectId` makes
  every engine call fail with `InvalidHandle`. It never names a different
  object. So a stale id is a clean error, and the only memory hazard is the
  cell itself.
- **`JSObject*` is a cell address that Servo dereferences inline**
  (`shape->base->{clasp, realm}`). Freeing a cell while Servo still holds the
  pointer is a use-after-free in Servo's code and in the facade's
  `object_cell()`, not an engine error. Debug builds have to catch this.
- **The engine side is already there.** `Host::trace_roots` is read at the start
  of every collection, `Host::trace(object, class, payload)` runs during
  marking for host objects tagged `traced`, and `Host::finalize(class, payload)`
  runs after the sweep (`run_host_finalizers`). `Host::trace` "must not call
  into the engine", and Servo's trace hooks only call the four tracer glue
  functions, which need no engine.
- **Finalize hooks read reserved slots.** Servo's `finalize_hook` calls
  `JS_GetReservedSlot(obj, DOM_OBJECT_SLOT)` on the dying object to find the
  boxed DOM struct. It also needs the `JSObject*`, which `Host::finalize(class,
  payload)` does not give. Reserved slots live in the facade (`Shared.slots`,
  bit-exact), so they can outlive the engine object until the hook has run.
- **Collections happen inside the engine.** Under `GcPolicy::Threshold`, a
  collection can start at any allocation, while the engine is out of the
  context slot. The post-collection work therefore cannot borrow the engine. It
  has to run on data the engine hands to the host.
- **The engine already computes the dead set.** `prune_metadata` (vm.rs) runs
  `self.objects.retain(|id| live)` after every collection, which costs O(objects).
  Passing the removed ids on adds O(dead).

## Options

### Roots

1. **Status quo: every cell is a root.** Correct, but nothing is ever freed.
2. **Precise roots from Servo's root sets** (SpiderMonkey's model). The facade
   walks `stackRoots_[kind]` for the kinds Object, Value, String, Symbol, Id and
   Traceable (through the `RootedVFTable` trace pointer), calls every registered
   extra roots tracer with a facade `JSTracer`, and reads the raw value roots.
   The engine adds its own roots. Chosen.
3. **Conservative stack scanning.** Not possible without reading the native stack
   as raw words, which the engine's rules rule out, and not needed: Servo is
   precise by construction (its static rooting analysis exists for SpiderMonkey).

### Cell freeing

1. **Free on death.** The cell is owned by the map (`HashMap<ObjectId,
   Box<ObjectCell>>` instead of `Box::leak`). A Box's contents never move, so the
   `JSObject*` address is stable, and removal drops it in safe Rust. This matches
   SpiderMonkey. A stale pointer is then a use-after-free. Chosen for release
   builds.
2. **Recycle cells through a free list.** Rejected. A stale `JSObject*` would
   alias a live object of another class: silent wrong behaviour instead of a
   crash.
3. **Poison mode (debug and CI).** A dead cell is never freed. Its id becomes
   `ObjectId::DEAD` and its shape points to a static poison shape whose `clasp` is
   a `DeadObject` JSClass. `object_cell()` and every entry point that resolves a
   cell panic with "mozjs facade: stale JSObject* (object N died in collection
   M)". Each dead cell costs 24 bytes until exit, which is fine for tests.
   `MOZJS_FACADE_CELLS=poison` selects it. CI runs the facade tests and the C1
   probes with it, next to `MOZJS_FACADE_GC=stress`, so a missing root shows up
   as a named panic at the first collection after the pointer went stale.

### The dead-object notification

1. **The facade scans its cells after each collection.** It needs engine access,
   and the engine is out of the slot during a collection. Rejected.
2. **`Host::finalize` with the id.** It only covers host objects, but Servo also
   holds cells of plain functions, arrays and promises.
3. **New `Host::objects_swept(&self, dead: &[ObjectId])`.** It is called once per
   collection, after `prune_metadata`, with the ids it removed. The facade keeps
   the ids that have cells. It runs `finalize` for those whose class has a hook,
   then drops their cell, slots, proxy state, native entry and error report.
   Chosen. It is O(dead) per collection on top of the existing O(objects) prune.
   `Host::finalize` stays for engine-only embedders.

## Design

### Facade `JSTracer`

```rust
#[repr(C)]
struct FacadeTracer {
    base: JSTracer,              // kind_ = Marking; vtable_ null (Servo never calls it)
    out: RefCell<Vec<ObjectId>>, // edges reported so far
}
```

`CallObjectTracer`, `CallValueTracer`, `CallFunctionTracer` and `CallIdTracer`
(symbols are interned, so ids add no object edge) cast `trc` back to
`FacadeTracer` and push the cell's `ObjectId`. A non-facade tracer is a facade
bug, so they assert on a magic word in `base.context_`. `CallStringTracer` and
`CallScriptTracer` are no-ops: string cells are interned text, not engine
objects (see "Not in scope"). The cell pointers never move, so tracers never
write the `Heap<T>` back.

### Roots (`FacadeHost::trace_roots`)

In this order, with one `FacadeTracer`:

1. **Rooted lists.** For each kind in {Object, Value, Id, Traceable}, follow
   `stackRoots_[kind]` through `RootedBase::prev`. `data` sits right after the
   base. For Traceable, the vtable word sits before the base, and its `trace(this,
   trc, name)` is called. These raw reads are new `unsafe` sites in
   `sys/imp/trace.rs`, each with a `// SAFETY:` note: a linked `Rooted` is live
   until it unlinks itself in `Drop`.
2. **Extra roots tracers.** Every `(op, data)` in `State.extra_roots`, which covers
   Servo's `trace_rust_roots` and mozjs's `trace_traceables`. They run in reverse
   registration order, as SpiderMonkey's `traceEmbeddingBlackRoots` does. Order
   doesn't change the result.
3. **Raw value roots.** As today.
4. **Facade internals.** `State.pending_exception`, the globals of the entered
   realm and of every realm in `running_scripts`, and objects of dequeued
   `JSMicroTask*`s not yet run (`job_tasks`).
5. **Engine roots.** The engine adds these itself: frames, job queue,
   intrinsics.

A tracer can call back into a facade entry point. `trace_rust_roots` only
traces, but a debug assertion in `Context` (a "tracing" flag) turns any other
entry point into a named panic during marking, like SpiderMonkey's
`JS::AutoAssertNoGC`.

### Edges (`FacadeHost::trace`)

As today (object-valued reserved slots, the proxy private value), plus: if the
object's class has `cOps->trace`, call it with a `FacadeTracer`.
`attach_host` passes `traced = true` when the class has a trace hook, or object
slots, or is a proxy.

### Sweep (`FacadeHost::objects_swept`)

For each dead id with a cell, in id order:

1. If the class has `cOps->finalize`, call `finalize(gcx, cell_ptr)` with a
   static `JS::GCContext`. Slots are still in `Shared.slots`, so
   `JS_GetReservedSlot` in the hook works. The hook must not reach the engine,
   which is checked by the same "tracing" flag (SpiderMonkey's finalizers have
   the same restriction).
2. Drop the cell (or poison it), then the id's entries in `slots`, `proxies`,
   `native_of`, `user_input`, `job_tasks` and `State.error_reports`.
3. A dead global: clear `RealmCell.global`, so `CurrentGlobal` on that realm
   asserts instead of returning a dangling cell.

`JSGCCallback` gets `JSGC_BEGIN` before the roots are taken and `JSGC_END`
after the sweep, which is what Servo's `debug_gc_callback` checks. The callback
runs from the engine's collection entry; this needs a second small engine hook,
`Host::collection_started`, called before `trace_roots`.

### Engine changes (all safe Rust)

- `Host::objects_swept(&self, dead: &[ObjectId])`, default no-op, called from
  `prune_metadata` after `run_host_finalizers`.
- `Host::collection_started(&self)`, default no-op.
- No change to marking. `Host::trace` already exists and is called only for
  `traced` host objects.

## Migration without losing passes

Each step is one commit with the usual gate (fmt, build, engine and facade tests
under Threshold and Stress, gated test262 profiles, CI C0/C1).

1. **Shadow tracing.** Implement `FacadeTracer`, the four glue tracers, the Rooted
   list walk, the extra and internal roots, and the class trace hook calls.
   Keep "every cell is a root", and after each collection compute in debug the
   precise reachable set and log `cells kept / cells precise tracing would
   free`. Nothing changes behaviour. Gate: C1 8/8, and the counter is printed
   in the C1 probe log. A count of 0 on the script probe means the roots are
   too broad. A probe that crashes later in step 3 points to roots that are too
   narrow.
2. **The engine hooks** (`objects_swept`, `collection_started`), with an engine
   test that a host learns the exact dead set.
3. **Flip, with poison on.** Cells are owned by the map, and precise roots
   replace "every cell". The sweep runs finalize, then frees or poisons. CI runs
   the facade tests with `MOZJS_FACADE_CELLS=poison` under both GC policies,
   plus the C1 probes with poison. Gate: C1 8/8 with poison, and no stale-pointer
   panics.
4. **Measure.** A facade test or probe step that creates and drops 10,000 DOM-like
   host objects in a loop and reports live cells after `JS_GC`: today 10,000,
   target near 0. The numbers go in this note and the commit.

Kill line: three fix rounds on the same stale-pointer or missing-root panic
without progress, then a narrower probe (a single Servo binding class) or a
second look at the root set.

## Not in scope

- **String cells.** `Atoms` interns every string text forever. That is memory
  growth, not a correctness issue: strings hold no object edges. It is a later
  item ("string cell lifetime"): count interned bytes, and free strings that no
  traced `Heap<*mut JSString>` holds.
- **Shapes, realms, compartments.** One shape per (class, realm), bounded by the
  number of realms. Realm destruction comes with workers and iframes (roadmap
  D).
- **Weak pointers, `PreserveWrapperCallback`.** Servo doesn't call the weak
  pointer API. Wrapper preservation matters once a reflector can be a WeakMap
  key without a strong path, which is a follow-up test.
- **Incremental or generational GC.** The barriers stay no-ops. The engine
  collector is stop-the-world.

## Literature check

Requested from Literature Review (#82) before step 3 (skill
`request-literature-check`). The question: is precise embedder rooting with
post-sweep finalization and a debug poison mode the established design for a
binding layer on a tracing GC (SpiderMonkey's Rooted and trace hooks, V8's
`cppgc` and `TracedReference`, WebKit's `JSC::Weak` and opaque roots), and are
there known pitfalls in finalizer ordering for DOM wrappers?
