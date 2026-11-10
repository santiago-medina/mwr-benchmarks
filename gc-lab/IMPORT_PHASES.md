# Import phases: `import defer` and `import source` (gap 19)

Status: design, 2026-11-10. Extends MODULE_RECORDS.md.

## Why

In full25 (45309/47708), 505 test262 cases carry the `import-defer` or `source-phase-imports` feature. None pass except one, and almost all are unsupported:

| bucket | cases |
|---|---|
| positive cases: "syntax: unsupported import phase" | 252 |
| negative parse cases gated on `import-defer` | 93 |
| negative parse cases gated on `source-phase-imports` (+ `-module-source`) | 86 + 63 |
| `built-ins/AbstractModuleSource` (`$262.AbstractModuleSource` missing) | 8 failed |

## Spec sources

Both proposals are Stage 3 and live outside ECMA-262. Section names below are the proposals' own:

- Source phase imports, https://tc39.es/proposal-source-phase-imports/ (draft 2026-03-25).
- Deferred module evaluation, https://tc39.es/proposal-defer-import-eval/ (draft 2026-09-21).

The test262 `info:` blocks quote an older revision of the defer text, with **EnsureDeferredNamespaceEvaluation**. The current draft folds that into **GetModuleExportsList** plus **EvaluateModuleSync**. Both readings behave the same in every case test262 checks, so the code follows the test262 text and names both. On a conflict, the project rule applies: the spec text and test262 decide.

## Syntax

- **ModuleRequest Records gain `[[Phase]]`**: `source`, `defer` or `evaluation`. ModuleRequests dedupes on (specifier, phase), never on the specifier alone. `[[LoadedModules]]` stays keyed by specifier (ModuleRequestsKeyEqual ignores the phase), so the host answers both phases with the same record.
- `import source ImportedBinding FromClause ;` creates an ImportEntry with `[[ImportName]]` ~source~. `import source from "x"` is still a default import named `source`, and `import source from from "x"` is a source import named `from`. Line terminators between the tokens are allowed.
- `import defer NameSpaceImport FromClause ;` is the only defer form. `import defer x from`, `import defer {a} from`, `import defer x, * as ns from` and `export defer …` are SyntaxErrors. `import defer from "x"` is a default import named `defer`.
- `import.source ( AssignmentExpression ,opt )` and `import.defer ( AssignmentExpression ,opt )` become `Expr::ImportCall(specifier, phase)`. Without an argument, with a spread, under `new`, or with no call at all (`typeof import.source`), they are early SyntaxErrors. A second argument is import attributes, which stay Unsupported, as in `import(x, opts)`.
- ParseModule: `import source x from "m"; export { x };` reclassifies the local export as an indirect ExportEntry with `[[ImportName]]` ~source~ (the same rule that already exists for `import * as ns; export { ns }`).
- A WithClause (import attributes) is handled as for any import (JSON_MODULES.md).

## Semantics

**Loading.** InnerModuleLoading and ContinueModuleLoading load a `source` request as a single load: the record is fetched, but its own requests are not. Graph waiters therefore carry a load type.

**Linking.** InnerModuleLinking walks `defer` and `evaluation` requests, not `source` ones.

**Evaluation.** InnerModuleEvaluation builds an evaluation list:

- an `evaluation` request adds its module;
- a `defer` request adds GatherAsynchronousTransitiveDependencies(module): the TLA modules (and the cycles under them) the deferred module needs and that have not been evaluated yet;
- a `source` request adds nothing.

The DFS then runs over that list instead of the raw requests.

**InitializeEnvironment.**

- A `source` import binds the target's `[[ModuleSource]]`. If that is ~empty~, which it always is for a Source Text Module Record, the step throws a SyntaxError at link time.
- A namespace import passes its request's phase to GetModuleNamespace.
- ResolveExport returns `ResolvedBinding { m, ~source~ }` or `{ m, ~deferred-namespace~ }` for the re-export forms.

**Namespaces.** A record holds two namespace slots, `[[Namespace]]` and `[[DeferredNamespace]]`.

- The deferred namespace has `@@toStringTag` "Deferred Module", and its `[[Exports]]` leave out "then".
- On a deferred namespace, IsSymbolLikeNamespaceKey is true for symbols and for "then". Those keys go to the ordinary methods, and touching them never triggers evaluation.
- Every other string-keyed [[GetOwnProperty]], [[DefineOwnProperty]], [[HasProperty]], [[Get]] and [[Delete]] first runs EnsureDeferredNamespaceEvaluation, and so does [[OwnPropertyKeys]].
- EnsureDeferredNamespaceEvaluation throws a TypeError when the module is not evaluated and ReadyForSyncExecution is false (the module or a dependency is evaluating or evaluating-async, or has TLA). Otherwise it runs Evaluate() synchronously and throws the evaluation error if there is one.
- [[Set]], [[GetPrototypeOf]], [[SetPrototypeOf]], [[IsExtensible]] and [[PreventExtensions]] never trigger evaluation.
- In the engine the deferred namespace is a second `Object` with the same `data.namespace = Some(m)`. `namespace_key` tells the two apart by the record's `deferred` id.

**`import.defer()`.** In ContinueDynamicImport, after Link():

- it gathers the async transitive dependencies;
- it evaluates each one;
- it waits on all of them, reacting to each evaluation promise (the spec says SafePerformPromiseAll);
- it resolves with the deferred namespace.

The draft's unconditional `module.Evaluate()` at step 6.c would evaluate the deferred module eagerly. The tests (`import-defer-does-not-evaluate`) show it is a drafting slip, so it is not followed.

**`import.source()`.** After the load, ContinueDynamicImport resolves with `[[ModuleSource]]`, or rejects with a SyntaxError when that is ~empty~. It never links or evaluates.

**%AbstractModuleSource%** (proposal §28.3) is created per realm but is not a global property; the test host exposes it as `$262.AbstractModuleSource`.

- The constructor always throws a TypeError. Its `length` is 0, its `name` is "AbstractModuleSource", and its `[[Prototype]]` is %Function.prototype%.
- The prototype is an ordinary object with `constructor` and a `@@toStringTag` getter. The getter returns the record's GetModuleSourceKind() when `this` has a `[[ModuleSourceClassName]]` slot (`data.module_source`), and undefined otherwise.

## Host API (additive)

- `ModuleRequest` gains `phase: ImportPhase`. It is informative: HostLoadImportedModule's answer "must not be affected by the phase", and the engine keys requests on (referrer, specifier) exactly as before.
- `ModuleLoad::ModuleSource { url, class_name, host_defined }` is a host-provided module that has a source object (the shape a WebAssembly module record will take in Servo, roadmap D). The engine creates a record with no requests and no exports, and gives it a `[[ModuleSource]]` object. That object's prototype is a per-realm prototype for `class_name`, whose own prototype is %AbstractModuleSource.prototype%. Imported in the evaluation phase, it behaves as an empty module. A real Wasm record would instead instantiate; that waits for Servo.
- js_check (the test262 host) answers the specifier `<module source>` with `ModuleSource { class_name: "ModuleSource" }`, as INTERPRETING.md asks, and answers `<do not resolve>` with a TypeError.

## Migration and checks

1. Parser and ParseModule, keeping both features in UNIMPLEMENTED_SYNTAX. Gate: the existing module smoke runs show no change.
2. Source phase: records, linking, `import.source()`, %AbstractModuleSource%, the js_check host. Remove `source-phase-imports` and `source-phase-imports-module-source` from UNIMPLEMENTED_SYNTAX. Smoke: `language/module-code/source-phase-import`, `staging/source-phase-imports`, `built-ins/AbstractModuleSource`, and the `import-source` cases of `language/expressions/dynamic-import`.
3. Defer: evaluation lists, deferred namespaces, `import.defer()`. Remove `import-defer`. Smoke: `language/import/import-defer`, and `dynamic-import` with `import-defer`.
4. Module regressions: language/module-code, language/import, language/export and language/expressions/dynamic-import against full25, plus `tests/modules.rs`, `compare_module_contracts.py` and `differential_modules.py`.

Kill line: 3 fix rounds per bucket without progress.

## Side fixes found by the smoke runs

- **EvaluateModuleSync on an evaluated module.** If the module's status is already ~evaluated~, ensure-evaluation returns its `[[EvaluationError]]` (or normal completion) and never re-enters Evaluate(). Before this, a deferred namespace touched after its module finished raised "synchronous module evaluation suspended".
- **IsModuleSCCEvaluated** reads `[[CycleRoot]].[[Status]]`, as the test262 text does, so a module inside an evaluated cycle counts as evaluated.
- **Super property assignment** (`super.x = v`, `super[k] op= v`, `++super.x`, `super[await k] = v`) was rejected as "invalid assignment target". The parser now accepts SuperMember/SuperComputed targets, and the async lowering saves the computed key in a register. PutValue on a super reference throws when [[Set]] returns false, but only in strict code (ECMA-262 §6.2.5.6 PutValue, step 3.d-e).
- **Private names on non-extensible objects** (proposal `nonextensible-applies-to-private`): PrivateFieldAdd and PrivateMethodOrAccessorAdd throw a TypeError when O.[[Extensible]] is false. Proxies are exempt, because they have no private-element list of their own to protect.

## Results (step 4)

- `source-phase-imports` features: 253/253 cases. `import-defer`: 250/252, with the 2 left being import attributes (Unsupported).
- The module, class and super directories against full25: 2246 passed, 23 failed and 173 unsupported out of 2442. That is +510, with 0 regressions.
- `tests/modules.rs` 18/18, `compare_module_contracts.py` 0 failures, `differential_modules.py` 0 failing seeds.
