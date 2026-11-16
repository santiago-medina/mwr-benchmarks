# Host exotic objects (E3)

Step E3 of SERVO_BINDINGS.md: objects whose essential internal methods
(ECMA-262 §6.1.7.2, table 4) are implemented by the embedder, not by script
handlers. Servo needs them for every `js::Proxy` it creates through
`NewProxyObject`: the named-properties object of `Window`
(`window_named_properties.rs`, the current probe frontier), DOM objects with
named or indexed getters (`[LegacyUnenumerableNamedProperties]`,
`HTMLCollection`, `DOMStringMap`, ...), `Location`, and later `WindowProxy`.

## What Servo needs

`NewProxyObject(cx, handler, priv, proto, class, lazyProto)` creates a proxy
whose behaviour is a `ForwardingProxyHandler` (jsglue.cpp) built from a
`ProxyTraps` table of Rust callbacks. That handler:

- always calls the traps for [[GetOwnProperty]], [[DefineOwnProperty]],
  [[OwnPropertyKeys]], [[Delete]], [[PreventExtensions]], [[IsExtensible]] and
  `getPrototypeIfOrdinary`;
- calls `has`, `get`, `set`, `hasOwn`, `getOwnEnumerablePropertyKeys`,
  `className`, ... when the table has them, and otherwise falls back to
  `js::BaseProxyHandler`, whose `has`/`get`/`set` are the ordinary algorithms
  (OrdinaryHasProperty §10.1.7.1, OrdinaryGet §10.1.8.1, OrdinarySet
  §10.1.9.1) driven by the proxy's own [[GetOwnProperty]] and
  [[GetPrototypeOf]];
- has a *static* prototype (`lazyProto == false`): `getPrototypeIfOrdinary`
  answers `isOrdinary = true` with the prototype stored on the object, and
  [[SetPrototypeOf]] is the ordinary algorithm (or refuses when
  `JS_SetImmutablePrototype` made it an immutable prototype exotic object,
  §10.4.7);
- keeps a private value (`GetProxyPrivate`/`SetProxyPrivate`, the DOM expando
  object) and reserved slots (`Get/SetProxyReservedSlot`, the DOM reflector's
  native pointer) next to the handler.

A trap re-enters the engine through ordinary JSAPI calls
(`JS_GetOwnPropertyDescriptorById` on the prototype, `JS_DefinePropertyById`
on the expando, `GlobalScope::from_object(proxy)` reading reserved slots).

## What the current code constrains

- The engine's script `Proxy` (`Object::Proxy`, PROXY.md) has no `Data`:
  `data()` returns Unsupported, so ordinary fast paths that forget a proxy
  check fail loudly. About 35 sites branch on `is_proxy(id)`; each internal
  method in objects.rs dispatches to a `proxy_*` method in proxy.rs.
- `Host` (embed.rs) is held as an `Rc` and cloned before each call so it can
  re-enter with `&mut Engine`; `host_call` wraps the call in `frame` and
  protects the arguments. Host functions already work this way, so a host
  trap can follow the same contract.
- The facade's `call_native` parks the running engine in `Heap::engine` and
  swaps a spare in behind the `&mut Engine` while foreign code runs, so JSAPI
  entry points called from that code find the engine with `with_engine`.
- E1 host objects keep class/payload words in the runtime's `HostTag` and
  reserved slots in `Data::host_slots`; the facade mirrors slots bit-exact in
  `Shared::slots` and traces them in `FacadeHost::trace`.

## Options

1. **A script `Proxy` whose handler is a host object with native trap
   functions.** No new object kind: the facade would build a JS handler object
   with one host function per trap. Rejected: §10.5 runs every invariant
   check against a target the DOM proxy does not have, every trap
   allocates argument arrays and descriptor objects, `getPrototypeIfOrdinary`
   and static prototypes have no equivalent, and the private value and slots
   would need a separate target object. (WebIDL makes the class-string
   `@@toStringTag` and named properties configurable, §3.7.4.1, so the
   invariants themselves would not be violated by a correct binding.)
2. **A new object kind whose internal methods call `Host::exotic`
   (chosen).** `Object::HostExotic { data, handler }`: it has `Data` (prototype,
   extensibility, immutable-prototype flag, host slots, host tag), so the E1
   API (`reserved_slot`, `host_class`, `set_immutable_prototype`) works
   unchanged, but `is_proxy` answers true for it, so every ordinary fast path
   already skips it and every essential internal method reaches the `proxy_*`
   entry points, which branch to the host before touching proxy slots.
3. **Exotic hooks on ordinary objects** (a flag in `Data`, checked by
   `own`/`define`/...). Rejected: the fast paths that match `Object::Ordinary`
   directly (`plain`, `plain_set`, the 15 `Object::Ordinary(d)` matches) would
   all need the flag check, and a missed one is a silent wrong answer, not an
   Unsupported.

This is the same split SpiderMonkey makes (`ProxyObject` with a
`BaseProxyHandler`) and that V8 makes with interceptors on API objects; the
engine only needs the dispatch point, not a handler language.

## Design

Engine (`src/script/host_exotic.rs`, embed API in embed.rs):

```rust
pub enum ExoticOp { GetPrototypeOf, SetPrototypeOf(Option<ObjectId>), IsExtensible,
    PreventExtensions, GetOwnProperty(PropertyKey), DefineOwnProperty(PropertyKey,
    PartialDescriptor), HasProperty(PropertyKey), Get(PropertyKey, JsValue),
    Set(PropertyKey, JsValue, JsValue), Delete(PropertyKey), OwnPropertyKeys }
pub enum ExoticReply { Default, Bool(bool), Prototype(Option<ObjectId>),
    Property(Option<PropertyDescriptor>), Value(JsValue), Keys(Vec<PropertyKey>) }
trait Host { fn exotic(&self, engine, object, handler: u64, op) -> Result<ExoticReply>; }
Engine::create_host_exotic(prototype, handler, class, payload, slots, traced)
Engine::exotic_handler(object) -> Option<u64>
```

- Each internal method sends one `ExoticOp` inside `frame`, with the object
  and the operands protected (as `host_call` does). `Default` selects the
  ordinary behaviour, which is what a trap-less `BaseProxyHandler` does:
  [[GetPrototypeOf]] reads the static prototype, [[SetPrototypeOf]] is
  OrdinarySetPrototypeOf §10.1.2.1 (honouring the immutable-prototype flag),
  [[IsExtensible]]/[[PreventExtensions]] use `Data::extensible`,
  [[GetOwnProperty]]/[[DefineOwnProperty]]/[[Delete]]/[[OwnPropertyKeys]] use
  the ordinary property map, and [[HasProperty]]/[[Get]]/[[Set]] run
  OrdinaryHasProperty, OrdinaryGet and OrdinarySet (§10.1.9.1-2
  OrdinarySetWithOwnDescriptor) over the *dispatched* [[GetOwnProperty]] and
  [[GetPrototypeOf]]. So a host exotic with every answer `Default` behaves as
  an ordinary object, which is the engine test's baseline.
- No §10.5 invariant checks: like SpiderMonkey's DOM proxies, the embedder
  is trusted (WebIDL §3.9 legacy platform objects define their own
  invariants). A reply of the wrong shape for its op is a TypeError naming
  the op, never a crash.
- Host exotics are not callable and not constructors (Servo creates none that
  are; `WindowProxy` is not callable either). `typeof` is "object".
- GC: the edges are those of its `Data` (prototype, slots), plus whatever
  the host reports for the host tag. The handler word is not an object.

Facade (`sys/imp/proxy.rs`):

- `NewProxyObject` (glue and jsapi forms) creates a host exotic with the
  handler pointer as handler word, the class as host class, the class's
  reserved slot count, and the private value stored with the slots
  (`Shared::proxy_private`, traced like slots).
- `FacadeHost::exotic` maps each op to the handler's `ProxyTraps` entry with
  the same park/unpark as `call_native` (factored into one helper), converting
  keys (`jsid`), descriptors (`JS::PropertyDescriptor` + `isNone`),
  `ObjectOpResult` (a failure code is `Bool(false)`) and id vectors. Missing
  optional traps return `Default`; a trap returning false raises the pending
  exception, as for a JSNative.
- `GetProxyHandler`, `GetProxyHandlerExtra`, `IsProxyHandlerFamily`,
  `GetProxyPrivate`, `SetProxyPrivate`, `Get/SetProxyReservedSlot`,
  `IsProxy`-style class checks and `JS_SetImmutablePrototype` work on them.

## Migration and risk

- Script proxies are untouched: every `proxy_*` method checks the host exotic
  case first and otherwise runs as before, so test262 Proxy results cannot
  move. A smoke run of the Proxy and Reflect directories checks it.
- Sites that call `proxy_slots` directly (`is_array`, GetFunctionRealm,
  `proxy_call`) see a host exotic as "not a proxy target chain": `is_array`
  answers false, GetFunctionRealm uses the current realm, the call paths
  match `Object::Proxy` explicitly and never see one.
- Known gaps: private elements on host exotics (same as script proxies),
  `enumerate`/`getOwnEnumerablePropertyKeys` traps (for-in uses
  [[OwnPropertyKeys]] + [[GetOwnProperty]], which is the spec algorithm),
  `className`/`fun_toString`/`boxedValue_unbox`, lazy (dynamic) prototypes
  (`lazyProto == true`, needed by `WindowProxy` and cross-origin objects:
  E3b), wrappers and cross-compartment handlers (D).

Kill line: if the Servo probe has not moved past `window_named_properties`
after 3 CI rounds on this unit, stop and survey every proxy entry point the
probe reaches statically.

## Rules from the literature check

- **Re-entrancy after `Default`.** A trap may run script that changes the
  same object (CVE-2022-1096 shape: a [[DefineOwnProperty]] trap that defines
  the same key, then answers `Default`). The engine holds no `Data` borrow or
  copy across `Host::exotic`: every `Default` path re-reads the object after
  the host returns, and each internal method sends at most one op. Tested:
  one property, no duplicate key, also with the key on the prototype
  (CVE-2021-30551 path).
- **Receiver.** `Default` [[Get]]/[[Set]] forward the original receiver up
  the chain, so a getter on the prototype (or on a DOM expando reached
  through it) sees the proxy, not the holder (CVE-2013-1737). Tested.
- **No fast paths.** Host exotics stay off every shape or inline-cache fast
  path (`is_proxy` already does it). If inline caches come (step B), guard
  them with a generation counter, as SpiderMonkey does.
- **Debug invariant check.** In debug builds a host reply reporting a
  non-configurable own property prints a warning (WebIDL legacy platform
  objects report configurable properties, so it is a binding bug). Never a
  throw in release.
- **Immutable prototype.** `JS_SetImmutablePrototype` on the named-properties
  object sets the per-object flag; HTML's "global prototype chain mutable"
  bit maps to the same flag on each object of the chain, which is how Servo
  calls it (one call per object). Revisit if a spec change makes the bit
  per-realm only.
- **E3b (WindowProxy, lazy prototypes).** Keep `ObjectId` stable and the
  handler/private/slots swappable so a WindowProxy can be retargeted on
  navigation (SpiderMonkey's transplant); WindowProxy [[GetOwnProperty]]
  must not report non-configurable properties (check the current HTML text
  before implementing it); `lazyProto` needs a dynamic [[GetPrototypeOf]],
  which is the existing `Prototype` reply.

Literature Review (#82), request #426, 2026-11-16: "Adopt it". Report:
`/home/santi/tasky_spaces/task-2581-solicitud-426-request-literature-check-c/docs/literature-check-host-exotic-dom-proxies-2026-11-16.md`.
Prior art: SpiderMonkey `js::BaseProxyHandler`/`ForwardingProxyHandler`
(jsglue.cpp), V8 named/indexed interceptors, Ladybird LibJS `PlatformObject`,
WebIDL §3.9 legacy platform objects.

## E3b: the WindowProxy

Servo's `ScriptThread::load` makes a WindowProxy for every new Window
(windowproxy.rs `WindowProxy::new`): `NewWindowProxy(cx, window, handler)`
with a handler from `CreateWrapperProxyHandler(&PROXY_TRAPS)`, then reserved
slot 0 = the `WindowProxy` Rust object and `SetWindowProxy(cx, window, proxy)`.
`script_thread.rs` first registers the class with
`SetWindowProxyClass(GetWindowProxyClass())`.

**What it is in the specs.** HTML 7.2 (WindowProxy) makes the realm's global
this value the WindowProxy, which forwards every internal method to the
browsing context's current Window. ECMA-262 has the hook:
§9.3.3 SetRealmGlobalObject step 3 stores `thisValue` as the global
Environment Record's [[GlobalThisValue]] (§9.1.1.4), which answers top-level
`this` (§9.1.1.4.11 GetThisBinding), the sloppy coercion of a nullish `this`
(§10.2.1.2 OrdinaryCallBindThis step 6) and the initial value of `globalThis`
(§19.1.1). The global object itself stays the Window: bare identifiers still
resolve on it.

**Design.** No new mechanism, so no new literature check: it is E3's host
exotic with one more handler kind behaviour.

- Engine: `Intrinsics::global_this` (initially the global object) and
  `Engine::set_global_this_value(realm, this)`, which sets it, rebinds the
  global environment's `this` and rewrites `globalThis` while it still holds
  the global object. `realm_global_this(realm)` reads it back. The binding
  keeps the value alive (a `link` from the global environment).
- Facade `WrapperNew` (jsglue.cpp, `js::Wrapper::New`): a host exotic whose
  private value is the target and whose prototype is lazy
  (`TaggedProto::LazyProto`). `NewWindowProxy` is `WrapperNew` with the glue's
  `WindowProxyClass` ("Proxy", `JSCLASS_IS_PROXY`, two reserved slots,
  `js::SwappableProxyReservedSlots`).
- A Wrapper-kind handler forwards every trap it lacks to the target, as
  `js::ForwardingProxyHandler` does (a DOM handler's missing trap is still
  the ordinary algorithm on the proxy). [[IsExtensible]] and
  [[PreventExtensions]] have no embedding entry point for the forward and stay
  ordinary on the wrapper; Servo defines both traps.
- `GetWindowProxyClass` returns the glue class; `IsWindowProxy` compares
  against the class `SetWindowProxyClass` registered (the runtime's
  `maybeWindowProxyClass`). Before this change the glue getter returned the
  registered class, null at Servo's registration, so `IsWindowProxy` was
  always false.
- Unwrapping (js/src/proxy/Wrapper.cpp): `IsWrapper` is true for a
  Wrapper-kind proxy; `UncheckedUnwrapObject`/`UnwrapObjectDynamic` follow it
  to the target unless asked to stop at a WindowProxy; `UnwrapObjectStatic`
  and `CheckedUnwrapStatic` always stop there. Servo's
  `private_from_proto_check` relies on the dynamic unwrap when a Window method
  runs with the WindowProxy as `this`.
- `SetWindowProxy` sets the global's realm [[GlobalThisValue]];
  `ToWindowIfWindowProxy` returns the target; `ToWindowProxyIfWindowSlow`
  maps a realm's global to its [[GlobalThisValue]] when that is a WindowProxy.

**Navigation (not yet).** Servo retargets a WindowProxy on navigation with
`JS_TransplantObject` (windowproxy.rs, `set_window`): the old proxy object
keeps its identity and gets the new Window as target. With the E3 layout
that is a swap of the facade `ProxyState` (handler, private) and the reserved
slots under the same `ObjectId`, plus `set_global_this_value` on the new
realm; it waits until the Servo probe reaches it.

**Open point.** HTML's WindowProxy [[GetOwnProperty]] and
[[DefineOwnProperty]] make non-configurable properties look configurable;
that is Servo's trap code, not the facade's, and runs as is.

Tests: `mozjs-facade/tests/window_proxy.rs` (class registration, unwraps,
trap and forwarded reads, `this`/`globalThis`/sloppy `this` before and after
`SetWindowProxy`) and `tests/embedding.rs::host_global_this_value`.
