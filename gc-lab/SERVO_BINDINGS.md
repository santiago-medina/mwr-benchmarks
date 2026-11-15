# Roadmap C: Servo's bindings on this engine

Status: design note, 2026-11-15. Nothing implemented yet. The literature check
(#82) is pending. Roadmap D (switching subsystems over, dropping mozjs) gets
its own note once milestone C2 holds.

## Goal

Servo v0.7.0 (aac43a3) should run page scripts on this engine instead of
SpiderMonkey. The WebIDL bindings, reflectors, rooting and tracing then target
this engine's GC. The engine stays `#![forbid(unsafe_code)]`. Any `unsafe` the
integration still needs is kept in one small crate and counted.

The earlier experiments (SERVO_INTEGRATION.md, servo-native/) run a side VM
next to SpiderMonkey through the typed `DomAdapter`. They prove the joint GC
protocol and the event, microtask, fetch and module plumbing. They do not
scale to the web platform: every DOM method would need a hand-written adapter
call. Roadmap C replaces the adapter with Servo's own generated bindings.

## What Servo needs from a JS engine (survey)

The survey covers the sparse checkout at `servo-src/` (components/script and
components/script_bindings at aac43a3). Its scripts and raw output are in
`scratch/c_survey/`.

- Servo pins `js = { package = "mozjs", version = "=0.26.4" }`, which sits on
  `mozjs_sys =153.3.0-1` (SpiderMonkey 153). The `mozjs` crate itself is about
  8.3k lines of Rust: rust.rs 1576, conversions.rs 921, jsapi2 wrappers 679,
  typedarray 515, gc/root 465, cell 386, realm 305, context 229.
- Servo imports 564 distinct `js::` paths (583 including qualified uses):
  - by module: rust::wrappers2 203, jsapi 200, glue 44, rust 39, typedarray 25,
    gc 13, jsval 12;
  - by area: objects, properties and classes 139; typed arrays and buffers 58;
    rooting and handles 54; values and ids 43; realms, globals and context 36;
    tracing and GC 31; modules and scripts 30; promises and jobs 23; strings 19;
    errors 16; proxies 12; structured clone 10; the rest is miscellaneous.
- The most used paths are `context::JSContext` (626 files), `rust::HandleObject`
  (282), `context::NoGC` (121), `HandleValue` (82), `realm::CurrentRealm` (77),
  `UndefinedValue` (76), `jsapi::Heap` (72) and `JSObject` (71).
- 694 files in components/script use mozjs. There are 1292 `unsafe`
  occurrences in script and 570 in script_bindings.
- There are 574 WebIDL files (18.6k lines). The generator, codegen.py, is
  about 9.6k lines and emits JSClass, JSPropertySpec, JSFunctionSpec and
  JSJitInfo statics, plus `extern "C"` JSNative and getter/setter functions.
- The patterns that shape the design:
  - The Reflector holds a `Heap<*mut JSObject>` and is traced through
    `js::gc::Traceable`.
  - The DOM private is stored with `PrivateValue` in `DOM_OBJECT_SLOT`
    (JS_Get/SetReservedSlot).
  - DOM proxies (WindowProxy, named-property and legacy platform objects) go
    through `glue::CreateProxyHandler` plus `ProxyTraps`.
  - Servo dereferences `get_object_class` results (`(*clasp).flags`), so a class
    pointer must point at Servo's own static `JSClass`.
  - `rooted!` (685 uses), `.handle()` (770) and `.handle_mut()` (393) dominate.
    `Handle::from_raw` appears 153 times; `from_marked_location` once.

## Options

### A. A mozjs-compatible facade crate (recommended first)

A new crate implements the public API of `mozjs 0.26.4` that Servo uses
(`jsapi`, `jsval`, `glue`, `rust`, `gc`, `conversions`, `typedarray`,
`context`, `realm`, `error`, `panic`) on top of this engine. Servo swaps it in
through its workspace dependency (`js = { package = "mwr-mozjs", path = ... }`).
That is a one-line change in Servo's Cargo.toml.

- **Object pointers are opaque tokens.** A `*mut JSObject` is built from an
  integer (`ptr::without_provenance_mut`, which is safe) that encodes a slot in
  the facade's handle table. The table maps the token to the engine `Id` and
  checks a generation number. Servo never dereferences a `JSObject`; mozjs
  already treats it as opaque. Our GC does not move objects, so no relocation
  is needed.
- **JSVal** keeps mozjs's 64-bit NaN-boxed layout as a safe bit-packed `u64`,
  with object tokens in the 47-bit payload.
- **Classes and private data.** The engine gets a *host object* kind that
  stores an opaque class token, N reserved slots (Values) and a host payload.
  The facade stores Servo's `&'static JSClass` address and returns it from
  `get_object_class`. It is a real pointer to Servo's own static, so Servo's
  existing `(*clasp)` reads stay valid.
- **Specs.** At interface setup, the facade reads the JSPropertySpec,
  JSFunctionSpec and JSJitInfo tables (NUL-terminated C strings and arrays
  that end in a zero entry) and installs host functions. Each host function
  remembers the `JSNative` or JSJitInfo op it wraps.
- **Calls.** When JS calls a host function, the engine hands the facade a
  receiver and arguments. The facade fills a `Vec<JSVal>` laid out as
  `[callee, this, args...]` and calls Servo's `extern "C"` function with
  `vp`. It then reads the return value from `vp[0]` and maps a `false` return
  plus the pending exception to a throw.
- **Rooting.** `rooted!` and `RootedGuard` register their stack slot in a
  per-context root list, just as mozjs links `Rooted` into the JSContext root
  stacks. When our collector marks, the facade reads every registered slot.
  `Heap<T>` is a cell whose contents are traced through `Traceable`, as today.
- **Tracing.** A `JSTracer` is an edge collector. `JS::TraceEdge` and
  `CallObjectTracer` push the referenced token's engine `Id` onto the mark list
  of the collection in progress. Servo's root tracer (`trace_roots`, which
  walks the DOM through `JSTraceable`) is registered as an extra-roots hook
  and runs during every collection. This is the same edge flow that
  JOINT_GC.md already validated, with one heap instead of two.
- **Proxies.** `CreateProxyHandler(&ProxyTraps)` stores the traps struct. The
  engine's host-exotic object calls back into those traps for each internal
  method ([[Get]], [[DefineOwnProperty]] and so on, ECMA-262 §10.5).
- **Realms and compartments.** There is one compartment per realm. Cross-realm
  wrappers (`JS_WrapObject`/`JS_WrapValue`) are identity, except for
  WindowProxy, which is already a proxy whose target the facade can swap.
  Same-origin checks stay in Servo's own code.
- **Scripts, modules and jobs.** `Compile*`, `JS_ExecuteScript`, module
  instantiation and evaluation, and the JobQueue/microtask hooks map onto the
  existing `Engine` API (MODULE_RECORDS.md, PROMISES.md).

**Where the `unsafe` lives.** The engine has none. The facade needs `unsafe`
in exactly three places:

1. Reading Servo's static spec tables and C strings.
2. Calling Servo's `extern "C"` function pointers (JSNative, finalize, trace,
   and the proxy traps).
3. Reading and writing locations behind `Handle`, `MutableHandle` and
   `Rooted`. These are raw pointers in mozjs's API, and Servo builds them with
   `from_raw` 153 times.

Each of these goes in its own module, and every block gets a `// SAFETY:`
comment. CI counts the blocks so the number can only shrink. Servo's own 1862
`unsafe` occurrences stay as they are. Removing them is option B's job.

**Costs and risks.**

- The API surface is large (about 580 paths), but most of it is thin:
  wrappers2 alone is 203 one-line forwards.
- Semantics come from SpiderMonkey rather than the spec: JSClass hooks
  (resolve, mayResolve, enumerate, finalize, trace), property attribute flags,
  `JSITER_*` flags and the exception state. The parts Servo really exercises
  will only show up when pages run.
- Some mozjs APIs have no meaning here: JIT info aliasing, GC zones and
  incremental barriers, stencils compiled off-thread. These become no-ops or
  synchronous equivalents, each listed in this note as it is decided.
- Typed arrays: Servo reads typed-array memory through
  `JS_GetArrayBufferViewData` and keeps borrowed slices. The engine stores
  buffers in a safe `Vec<u8>`. The facade can lend `&mut [u8]` for the scope
  of a `NoGC` token, which is how mozjs already guards that pointer.

### B. Retarget codegen.py and the script crate to a new safe API

Change codegen.py to emit safe trait implementations against an engine API
designed for Rust: typed handles with lifetimes and no raw pointers. Then port
the 694 script files to it.

- This removes most of Servo's binding `unsafe` and the three facade
  categories above.
- It is a fork that diverges across about 700 files and the generator. The
  first page would run only after nearly all of it compiles again, so there is
  no measurable signal for a long time. Every upstream rebase also conflicts
  everywhere.

### C. Keep extending the DomAdapter

This was rejected: every DOM method becomes hand-written glue, and it never
reaches Servo's WebIDL coverage.

## Decision

Start with **A**, then take on **B gradually** behind the same facade:

1. A delivers Servo-on-this-engine with a one-line Servo diff plus a small
   patch for whatever the facade cannot mimic. The engine stays fully safe.
2. Once pages run, safe APIs can be added to the facade (for example a typed
   `Handle<'gc, T>` without raw pointers) and codegen.py can move to them one
   template at a time. The facade's unsafe count and Servo's unsafe count are
   tracked as metrics. This is B as a sequence of measurable steps rather than
   one large rewrite.

## What the engine must add (safe Rust)

- **E1. Host objects:** a class token, reserved slots, an opaque host payload
  (a `u64` that the facade maps to Servo's private pointer) and a finalize
  notification when the object is swept.
- **E2. Host functions:** a callable object whose call goes to an embedder
  dispatch with an index and the receiver, plus constructor support.
- **E3. Host exotic objects:** every internal method can be routed to an
  embedder trait, building on the Proxy machinery in PROXY.md.
- **E4. Embedder roots and an extra-roots tracer hook:** at mark time, the
  collector calls the embedder to report extra `Id`s, possibly in several
  rounds until no new edges appear. JOINT_GC.md describes the same fixpoint.
- **E5. A public value and object API:** create objects, arrays and
  functions; get, set, define and delete properties; read own keys;
  instanceof; call and construct; plus string, number and BigInt
  conversions, pending exceptions and realm entry and exit. Much of this
  already exists as internals; it needs a stable embedding surface.
- **E6. Buffer and typed-array data access** scoped to a no-GC borrow.

## Milestones and how each is measured

Servo is built only by CI (the `servo-native` job on `[servo]` commits).
`cargo check` there is cheaper than today's builds, because the facade removes
the build of mozjs_sys, SpiderMonkey's C++ code.

| Milestone | Signal |
|---|---|
| C0: facade skeleton that type-checks against Servo | `cargo check -p script_bindings` error count, then the same for `-p script`, drives toward 0 |
| C1: servoshell links with the facade (no mozjs_sys) | CI job green; binary size and build time recorded |
| C2: classic inline scripts run; DOM reflectors created and collected | the existing servo-native fixtures (click, task list, promises, fetch, modules, GC 1500 cycles) rewritten as plain `<script>` |
| C3: events, microtasks, timers, module loading through Servo's own code | fixtures plus a fixed WPT subset (dom/nodes, dom/events, html/webappapis/timers, ecmascript-modules) |
| C4: WPT pass-rate parity tracking | the same WPT subset on mozjs Servo and on this engine; report both rates |

Each milestone ends with a checkpoint commit on `codex/gc-host-boundary` and
one milestone line to Santiago.

## Migration without losing passes

- test262 stays the engine's gate. The E1-E6 additions only add API, and the
  checkpoint procedure still applies to them.
- The DomAdapter path and its fixtures keep working until C2 replaces them.
  Then they are retired in one commit.
- The facade crate lives in this repo (`gc-lab/mozjs-facade/`) so that the
  engine and the facade change together. The Servo-side patch stays in
  `servo-native/servo.patch`.

## Open questions for review

- Is opaque-token pointer identity enough wherever mozjs callers compare
  `*mut JSObject`? Servo hashes and compares these pointers (for example in
  WeakReferenceable and maps keyed by object).
- Does a non-moving GC break any of Servo's assumptions? Moving-GC barriers
  become no-ops, which should be safe.
- How many workers per process are needed (one engine per thread), and is our
  `Rc`-based heap `!Send` in a way that conflicts with how Servo hands its
  runtime to threads?
