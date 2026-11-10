# Module records, namespaces, `import()` and `import.meta` (gap 17)

Status: design, 2026-11-09. Extends MODULES.md (the existing static-graph core and the native Servo loader).

## Why

In full23 (43818/47708), about 1800 non-passing test262 cases are module work, all of them unsupported, none failing:

| bucket | cases |
|---|---|
| `import(` in scripts (parser: "unsupported reserved expression") | 588 |
| `flags: [module]` (runner refuses them; includes the `generated` and `async` variants) | 830 |
| negative parse cases gated on `dynamic-import`, `import-defer`, `source-phase-imports` | 377 |
| `phase: resolution` negatives | counted inside `flags: [module]` |

The current core (`src/script/modules.rs`, 429 lines) is not the spec algorithm. It groups modules by weakly connected component instead of strongly connected components (SCCs), it has no `[[AsyncEvaluation]]` ordering, no namespaces, no `export *`, no string export names, and the host must hand it the whole graph up front. That last point blocks `import()`, which loads at run time.

## Spec algorithms (ECMA-262, 2025 draft numbering)

The rewrite follows these step by step, and each function cites its section:

- 16.2.1.4 Abstract Module Records; 16.2.1.6 Cyclic Module Records: `[[Status]]` new / unlinked / linking / linked / evaluating / evaluating-async / evaluated, `[[EvaluationError]]`, `[[DFSIndex]]`, `[[DFSAncestorIndex]]`, `[[RequestedModules]]`, `[[LoadedModules]]`, `[[CycleRoot]]`, `[[HasTLA]]`, `[[AsyncEvaluation]]` (as an order counter), `[[TopLevelCapability]]`, `[[AsyncParentModules]]`, `[[PendingAsyncDependencies]]`.
- 16.2.1.6.1 LoadRequestedModules / InnerModuleLoading / ContinueModuleLoading, with a GraphLoadingState record.
- 16.2.1.6.2 Link / InnerModuleLinking; 16.2.1.6.3 Evaluate / InnerModuleEvaluation / ExecuteAsyncModule / GatherAvailableAncestors / AsyncModuleExecutionFulfilled / AsyncModuleExecutionRejected.
- 16.2.1.7 Source Text Module Records: ParseModule builds ImportEntries, LocalExportEntries, IndirectExportEntries and StarExportEntries (16.2.1.7.1); GetExportedNames; ResolveExport (null / ambiguous / ResolvedBinding); InitializeEnvironment; ExecuteModule.
- 16.2.1.10 GetModuleNamespace; 10.4.6 Module Namespace Exotic Objects (all 11 internal methods, `@@toStringTag` "Module", null prototype, not extensible).
- 13.3.10 `import()`: EvaluateImportCall and ContinueDynamicImport. 13.3.12 `import.meta`: HostGetImportMetaProperties and HostFinalizeImportMeta (the test host adds nothing).
- 16.2.1.8 HostLoadImportedModule / FinishLoadingImportedModule, and the rule that the same (referrer, specifier) pair always yields the same record.
- Early errors (16.2.1.1): duplicate exports, an exported name with no binding, `import.meta` outside modules, `await` as an identifier in modules, the ModuleExportName string rule (IsStringWellFormedUnicode).

Static import attributes (`with { type: ... }`) are implemented (JSON_MODULES.md), and so are the import phases `import defer` and `import source` (IMPORT_PHASES.md).

## Host API (the engine↔embedder boundary)

HostLoadImportedModule is asynchronous in the spec: the host answers later through FinishLoadingImportedModule. The API is **SpiderMonkey/Boa-style deferred completion, exposed as a pull queue**, the same shape as the host job queue from gap 16 (`next_host_job` / `run_host_jobs`). Modern SpiderMonkey (the load hook returns, and the embedder calls `FinishLoadingImportedModule` later) and Boa (`async fn load_imported_module`) also complete later; only V8's old resolve callback and QuickJS's loader are synchronous. Literature Review (#82), request #331, report `task-2415-…/docs/literature-check-pull-module-loading-host-api-2026-11-09.md`, shaped the types below.

```rust
pub enum Referrer { Script { host_defined: u64 }, Module { url: String, host_defined: u64 }, Realm }
pub struct ModuleRequest { pub id: u64, pub referrer: Referrer, pub specifier: String, pub attributes: Vec<(String, String)> }
pub enum ModuleLoad {
    /// Source to parse: the engine raises the SyntaxError itself.
    Source { url: String, module_type: String /* "javascript" */, source: String, host_defined: u64 },
    /// Host failure: the engine throws a new `kind` error (TypeError for fetch failures, per HTML).
    Error { kind: String, message: String },
}
impl Engine {
    pub fn take_module_requests(&mut self) -> Vec<ModuleRequest>;
    pub fn pending_module_requests(&self) -> usize;     // never-answered requests stay visible
    pub fn finish_module_request(&mut self, id: u64, load: ModuleLoad) -> Result<()>; // unknown/settled id → Err(Host), never a panic
    pub fn execute_module(&mut self, url: &str, source: &str, host_defined: u64) -> Result<()>;
    pub fn set_script_host_defined(&mut self, host_defined: u64); // referrer data for import() in classic scripts
}
```

- `host_defined` is an opaque token the embedder maps to its own data (Servo: base URL, fetch options, settings object). A URL string is not enough for inline modules, event-handler `import()` or several realms.
- Record identity is the key the host returns, `(url, module_type)` (the HTML module map), never the URL alone. A different specifier that resolves to the same key reuses the record.
- `release_globals` (realm teardown) invalidates pending ids and releases their promise roots; a later `finish_module_request` gets `Err`.
- `execute_modules(entry, sources)` (used by `tests/modules.rs`, `module_check` and `servo-native/mwr.rs`) remains as a wrapper. It preloads the sources, answers each request from their `resolutions`, and fails on any request it cannot answer. Its signature and `module_status` / `module_binding_string` / `loaded_modules` do not change. The Servo adapter will later mirror `components/script/scripting/module_loading.rs::host_load_imported_module`.

## Engine representation

- `Registry.records: Vec<ModuleRecord>` (index = record id, never reused within a realm), released with the realm as today.
- Bindings keep the current mechanism: an import is an alias `(target env, name)` in the importer's environment (traced edge, live, TDZ-checked). `import * as ns` and `export * as ns` bind the namespace object as an ordinary initialised immutable binding.
- The namespace is a new `Object::Namespace { module: usize, exports: Vec<String> /* sorted by code units */ }`. The engine hooks it into the same `objects.rs` dispatch points that `is_proxy` already uses (get, set, has, delete, define, own property, own keys, prototype, extensibility). `[[Get]]` resolves through ResolveExport and reads the target binding (a ReferenceError in the TDZ). The GC traces the module environments it reaches through the registry.
- Execution: module bodies keep running as resumable VM programs (`compile_module` / `start_program`). `[[HasTLA]]` is `stmt_await` over the top level, as today. A module without TLA is executed synchronously, and its completion is read from the capability state.
- `import()`: a new `Expr::ImportCall(Box<Expr>)` in both evaluators (tree-walker and VM). It evaluates the specifier, runs ToString, creates a promise capability, enqueues a request with the active script or module as referrer, and returns the promise. ContinueDynamicImport runs when the host answers. Errors in the specifier's ToString reject the promise; they are not thrown.
- `import.meta`: `Expr::ImportMeta`, valid only in module code (early SyntaxError elsewhere). It is created lazily once per record: an object with a null prototype.

## Test262 runner

- `run.py`: `flags: [module]` → `js_check --module`; `negative: phase: resolution` → `<type> --negative-resolution` (pass only when linking throws that type); fixture files (`*_FIXTURE.js`) resolve relative to the importing file.
- `js_check`: after the script or module runs, it loops `take_module_requests` → read the file relative to the referrer (or the test file for scripts) → `finish_module_request`, interleaved with microtask checkpoints until both are empty. `--async` still waits for `$DONE`.
- Remove `dynamic-import` and `import.meta` from UNIMPLEMENTED_SYNTAX only when each one is implemented.

## Migration without losing passes

1. Phase-1 gate controls (request #331): a duplicate or late `finish_module_request`; a realm teardown with requests pending; an `import()` that is never answered (visible in `pending_module_requests`).
1. Keep the public API and the CI module steps green at every step: `tests/modules.rs`, `compare_module_contracts.py` (13 Node SourceTextModule graphs) and `differential_modules.py` (96 seeded graphs, including TLA ordering). Node v20 is available locally, so they run before each commit.
2. Phase 1: rewrite `modules.rs` onto the spec records (loading, linking, evaluation) behind the unchanged `execute_modules`. Gate: the three module checks above.
3. Phase 2: `--module` in the runner; the parser gets namespaces, `export *`, string names and `export default class`. Smoke: language/module-code, language/import, language/export.
4. Phase 3: `import()` and `import.meta`. Smoke: language/expressions/dynamic-import, language/expressions/import.meta.
5. Checkpoint after phase 3 (test262-checkpoint), with triage against full23.

The Servo bridge (`mwr.rs` module_loader) still provides the whole static graph up front; it does not drain `take_module_requests` yet, so `import()` on a page leaves the promise pending. That is a **roadmap D follow-up**, recorded next to the host job queue drain. The module limits (128 modules, 1 MiB) are test guards and must be revisited for the Servo path.

Kill line: 3 fix rounds per smoke bucket without progress, then narrow the survey or revisit this note.

## Phase 3 as implemented (2026-11-10)

- Parser: `import(x)` and `import(x,)` give `Expr::ImportCall`. `import()`, `import(...x)` and `new import(x)` are real early SyntaxErrors. `import(x, opts)` (import attributes), `import.source` and `import.defer` (import phases) stay "unsupported", never parsed as something close. `import.meta` outside module code is an early SyntaxError.
- EvaluateImportCall (13.3.10.1): the referrer is the active module, found by walking the environment chain up to a record's module environment; otherwise it is the script (`Referrer::Script` with `set_script_host_defined`). The capability is created first and ToString runs next, so an abrupt ToString rejects the promise. Pending imports live in `Registry.dynamic` slots whose capability values are held through `ExternalRoot`s, which are released when the import settles or the realm is torn down (`release_globals`).
- A second request with the same (referrer, specifier) joins the one in flight (16.2.1.10, FinishLoadingImportedModule: the same answer for the same pair). Script-referrer results are cached in `script_loaded`.
- ContinueDynamicImport (13.3.10.2) uses internal promise reactions (`Internal::DynamicImport { slot, step }`), so link, evaluate and namespace run in the job order the spec gives: the load promise, then `Link()`, then `Evaluate()`, then the namespace.
- A load failure whose cause is syntax this engine does not implement is reported to the host as Unsupported, not as a rejection (`Failure::unsupported`), so those test262 cases count as unsupported, not as failed.
- Spec fix found by `rejection-order.js`: AsyncModuleExecutionRejected (16.2.1.6.5) rejects the module's `[[TopLevelCapability]]` (step 9) before it recurses into `[[AsyncParentModules]]`. An earlier draft did it the other way round, which reordered the `import()` rejections.
- Smoke: language/expressions/import.meta 22/22, language/expressions/dynamic-import 556 passed, 0 failed (the remainder is unsupported: unimplemented syntax in fixtures 232, import phases 153, import attributes 60).
