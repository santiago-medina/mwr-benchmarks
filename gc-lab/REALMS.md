# Realms (ECMA-262 §9.3) in gc-lab

Status: design note for the `$262.createRealm` / `$262.evalScript` gap (roadmap A), written
2026-11-10. Bucket in full run 32: 181 cases Unsupported "$262.createRealm (one realm)", 13
"$262.evalScript", plus 57 ShadowRealm cases (a Stage 2.7 proposal, out of scope here).

## What the spec asks

- A Realm Record (§9.3) holds [[Intrinsics]], [[GlobalObject]], [[GlobalEnv]], [[TemplateMap]]
  and [[HostDefined]]. One agent (one `Engine`) may hold any number of realms.
- Every function object has a [[Realm]] slot: ECMAScript functions get the current realm at
  OrdinaryFunctionCreate (§10.2.3, via the running execution context's Realm), built-ins get
  the realm they were created for (CreateBuiltinFunction §10.3.4 step 4).
- [[Call]] / [[Construct]] of an ECMAScript function run PrepareForOrdinaryCall (§10.2.1.1),
  which pushes an execution context whose Realm is F.[[Realm]]. Built-ins do the same
  (§10.3.1 step 6: calleeContext.Realm = F.[[Realm]]). So "the current realm" is always the
  realm of the innermost function being run, and everything that reads %Intrinsic% names
  (new objects' prototypes, thrown errors, array literals, template objects) uses it.
- GetFunctionRealm (§7.3.24) reads [[Realm]] through bound functions and proxies (revoked
  proxy -> TypeError). It is used by GetPrototypeFromConstructor (§10.1.14 step 4: a
  cross-realm `newTarget` without an object `prototype` falls back to *its* realm's intrinsic)
  and ArraySpeciesCreate (§10.4.2.3 step 5: another realm's %Array% constructor -> undefined).
- The symbol registry (GlobalSymbolRegistry, §20.4.2.2) and well-known symbols are shared by
  all realms of the agent; the job queue is per agent.
- test262 INTERPRETING.md: `$262.createRealm()` returns the new realm's `$262`;
  `$262.evalScript(src)` runs `src` as a Script in the realm of that `$262` (ParseScript +
  ScriptEvaluation, §16.1.6, with GlobalDeclarationInstantiation §16.1.7) and throws a
  SyntaxError on parse errors.

## What the current code constrains

- `Engine` holds one `intrinsics: Option<Intrinsics>` and one `global: Option<ExternalRoot>`
  (the global environment). About 150 sites read `self.intrinsics` directly; ~15 read
  `self.global` (indirect eval, `Function()`, modules, fetch, unresolved references).
- Built-ins are `Object::Builtin { name, data }`, dispatched by name; closures are
  `Object::Closure { environment, .. }`. Neither has a realm field. A closure's environment
  chain already ends at its realm's global environment, so identifier resolution needs no
  change; only intrinsic lookups and `self.global` readers do.
- Intrinsics stay alive because `bootstrap(env)` links each of them to the global environment,
  which is rooted by `self.global`.

## Options

1. **Realm index on every function object** (a new field in `Builtin` and `Closure`). Exact
   spec shape, but touches every constructor of those variants (~60 sites) and grows the
   hot `callee()` copy.
2. **Allocation-time side table** `object_realms: IdMap<u32>`, filled in `allocate()` only
   while the current realm is not realm 0. Realm 0 objects have no entry, so the single-realm
   path costs one branch per allocation and one hash probe per call only once a second realm
   exists (`realms.len() > 1`). Entries die with their objects in `prune_metadata`.
3. **Realm as an object pointer** (a Realm heap object referenced from functions). This is
   what Servo eventually needs to hand out (see "Servo"), but it is option 1 plus GC tracing.

Chosen: **option 2** now, because it touches no object constructor and costs nothing in the
one-realm case that every current test and the Servo bridge run. The realm *index* is an
engine-internal detail; option 1 or 3 can replace the table later without changing any
behaviour or the embedding API.

## Design

- `Engine.realms: Vec<RealmSlot>` where `RealmSlot { env: ExternalRoot, intrinsics:
  Option<Intrinsics>, template_cache }`, and `Engine.current_realm: usize`. The running
  realm's intrinsics stay in `self.intrinsics` / `self.global`, so the ~165 existing readers
  are unchanged. The slot of the running realm holds `None`; entering realm k moves the
  current intrinsics into their slot and takes k's (a move of a few HashMaps, no copy).
  Realm 0 gets a slot lazily, on the first `createRealm`.
- `enter_realm(k) -> usize` / `leave_realm(prev)`: the execution-context Realm switch of
  §10.2.1.1 / §10.3.1. Called in `invoke_this` and `construct_with_new_target` when
  `realm_of(callee) != current_realm`; restored on every exit path (normal, throw, Unsupported).
- `realm_of(id)`: `object_realms.get(id).unwrap_or(0)`.
  `function_realm(value)` = GetFunctionRealm: follows `Bound` targets and `Proxy` targets,
  TypeError on a revoked proxy.
- GetPrototypeFromConstructor and ArraySpeciesCreate gain the cross-realm steps above;
  the intrinsic fallback is read from the slot of `function_realm(newTarget)`.
- `$262.createRealm()`: build an empty global environment, root it, switch to a fresh slot,
  run `bootstrap(env)` (so `$262`, `globalThis` and every intrinsic are new), read the new
  `$262`, switch back, return it.
- `$262.evalScript(src)`: runs in the callee's realm (the call already switched to it). Parse
  with the engine's script default; parse error -> SyntaxError of the current realm. Then
  GlobalDeclarationInstantiation (§16.1.7) against the existing global environment (the
  checks for lexical/var/function clashes and CanDeclareGlobalFunction/Var), then evaluate.
  This is also the multi-script model a Web page needs (several `<script>` elements share
  one global).
- `[[TemplateMap]]` is per realm (§13.2.8.4 GetTemplateObject): the template cache moves into
  the realm slot with the intrinsics.
- GC: each slot's `env` root keeps that realm's global and intrinsics alive; `release_globals`
  unroots all slots and resets `realms`/`current_realm`. A realm that is no longer reachable is
  not collected while the engine runs a script (the harness never drops one); a later
  per-realm release for closed iframes is a roadmap C/D item.

## Servo (roadmap C/D)

Servo creates a realm per window/iframe/worklet global (mozjs `JS_NewGlobalObject` +
`JSAutoRealm`). The embedding API should expose an opaque `RealmId` (new_realm, enter/leave
scoped guard, global_of(realm)), not the internal slot index. That API is decided in the
roadmap C design note; this change keeps realms engine-internal and reachable only through
the test harness `$262`.

## Migration and risk

- No behaviour change with one realm: no table entries, no switches. Checked with the
  checkpoint full run (expected 0 lost outside the bucket).
- Kill line: 3 fix rounds on the createRealm + evalScript bucket.
- ShadowRealm was implemented later (checkpoint 30); see the next section.

## ShadowRealm (proposal, Stage 2.7; `src/script/shadow_realms.rs`)

- `new ShadowRealm()` builds a full realm with `new_realm()` (split out of `create_realm`)
  and stores its slot in `Data.shadow_realm`.
- `evaluate(source)` is PerformShadowRealmEval: the source is parsed in the caller's realm (a
  parse error is the caller's SyntaxError), evaluated as an indirect eval in the shadow realm,
  and an abrupt completion becomes a TypeError of the caller's realm. The result passes
  through GetWrappedValue.
- GetWrappedValue: primitives cross unchanged, callables become wrapped function exotic
  objects (WrappedFunctionCreate + CopyNameAndLength), other objects throw TypeError.
  A wrapped function's [[Call]] (`Internal::Wrapped`, dispatched in `invoke_inner`) wraps
  each argument into the target realm and the result back, and turns a throw into the
  caller-realm TypeError.
- Not done: `importValue` (it needs HostLoadImportedModule without a referrer from the
  module loader); its 6 cases stay Unsupported.
- Servo path: each ShadowRealm is a full realm with its own intrinsics. The realm count cap
  (64) is a test guard and must be revisited when this reaches Servo.
- Smoke: built-ins/ShadowRealm 58/64 (6 importValue unsupported), 0 regressions over 1721
  neighbour cases against full36.
