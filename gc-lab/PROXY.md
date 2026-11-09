# Proxy exotic objects (gap 10)

Spec: ECMA-262 §10.5 (Proxy object internal methods and internal slots),
§28.2 (the Proxy constructor and `Proxy.revocable`), §7.3.1x helpers
(`IsCompatiblePropertyDescriptor` §10.1.6.2, `CompletePropertyDescriptor` §6.2.6.6),
§7.2.2 `IsArray`, §13.5.3 `typeof`.

## Why now

Servo's `WindowProxy` and several DOM exotic objects (legacy platform objects,
`Location`) are proxies in mozjs. Retargeting the bindings (roadmap C) needs a
Proxy-shaped dispatch point in the engine's internal-method layer. test262:
`built-ins/Proxy` has 311 non-passing cases (309 "standard intrinsic not
implemented"), `built-ins/Reflect` 11, and ~226 more cases elsewhere use
`new Proxy` as an observer (spec step order checks).

## Representation

A new `Object::Proxy { target: Option<Id>, handler: Option<Id>, callable: bool,
constructor: bool }` variant (`vm.rs`). `None` target/handler = revoked
(§10.5.14 `ValidateNonRevokedProxy`). `callable`/`constructor` are fixed at
creation from the target (§10.5.15 `ProxyCreate` steps 7.a/7.b.i). GC edges:
target and handler are linked with `runtime.link` at creation and unlinked at
revocation.

A proxy has **no `Data`**: `Engine::data(proxy)` keeps returning the
"ordinary reflection on host objects" Unsupported error. That is deliberate:
any code path that still reads ordinary slots directly (`data(id)?.prototype`,
`.extensible`, `.properties`) cannot silently give a proxy ordinary
semantics. Unconverted paths count as unsupported, never as a wrong pass.

## Dispatch

The essential internal methods live in `objects.rs`. Each one checks for a
proxy first and calls the trap version in `proxy.rs`:

| Internal method | Engine entry | Proxy trap (§) |
|---|---|---|
| [[GetPrototypeOf]] | `get_prototype_of` (new) | `getPrototypeOf` 10.5.1 |
| [[SetPrototypeOf]] | `set_prototype` | `setPrototypeOf` 10.5.2 |
| [[IsExtensible]] | `is_extensible` (new) | `isExtensible` 10.5.3 |
| [[PreventExtensions]] | `prevent_extensions` (new) | `preventExtensions` 10.5.4 |
| [[GetOwnProperty]] | `own` (now `&mut self`) | `getOwnPropertyDescriptor` 10.5.5 |
| [[DefineOwnProperty]] | `define` | `defineProperty` 10.5.6 |
| [[HasProperty]] | `has` (now `&mut self`) | `has` 10.5.7 |
| [[Get]] | `ordinary_get` | `get` 10.5.8 |
| [[Set]] | `ordinary_set` | `set` 10.5.9 |
| [[Delete]] | `delete` | `deleteProperty` 10.5.10 |
| [[OwnPropertyKeys]] | `own_keys` (now `&mut self`) | `ownKeys` 10.5.11 |
| [[Call]] | `invoke_inner` | `apply` 10.5.12 |
| [[Construct]] | `construct_with_new_target` | `construct` 10.5.13 |

The prototype walks in `has`, `ordinary_get` and `ordinary_set` follow
OrdinaryHasProperty / OrdinaryGet / OrdinarySet: when the walk reaches a parent
that is a proxy, it calls that parent's internal method with the original
receiver and returns its result.

`own`, `has` and `own_keys` were `&self`; trap calls need `&mut self`, so the
signatures change (pure refactor, no behaviour change for ordinary objects).

Every trap implements the invariant checks of its section step by step
(non-configurable / non-extensible target consistency) and throws TypeError
on violation.

## Callers converted

Builtins that must observe traps: `Object.*` and `Reflect.*` (keys, entries,
assign, getOwnPropertyDescriptor(s), defineProperty/ies, getPrototypeOf,
setPrototypeOf, isExtensible, preventExtensions, freeze/seal/isFrozen/isSealed
via the generic §7.3.15 SetIntegrityLevel / §7.3.16 TestIntegrityLevel when
the object is a proxy), `instanceof` (OrdinaryHasInstance walks
[[GetPrototypeOf]]), `Object.prototype.isPrototypeOf`, `Array.isArray` /
`IsArray` (§7.2.2: follows the proxy target, TypeError when revoked),
`typeof` (callable proxies are "function"), `callable()` / `constructible()`.
`for-in` enumeration, `JSON.stringify` and the Array methods are converted only
where the smoke shows them; elsewhere a proxy stays Unsupported.

## Migration and kill line

1. `&mut self` refactor of `own`/`has`/`own_keys` (compile, `cargo test`).
2. `Object::Proxy`, constructor, `Proxy.revocable`.
3. The traps with their invariants.
4. [[Call]] / [[Construct]], `typeof`, `IsArray`.
5. `Reflect` and `Object` callers.
6. Smoke: `built-ins/Proxy`, `built-ins/Reflect`, plus `built-ins/Object/keys`,
   `built-ins/Array/isArray`, `language/expressions/instanceof`.

Kill line: three fix rounds on `built-ins/Proxy` without progress means a
narrower survey of the remaining reasons.
