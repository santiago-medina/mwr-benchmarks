//! Cyclic Module Records (ECMA-262 §16.2.1.6) and Source Text Module Records
//! (§16.2.1.7) over traced lexical environments. Imports stay aliases
//! `(target env, name)`; MODULE_RECORDS.md describes the design and the pull
//! host API (`take_module_requests` / `finish_module_request`).
use super::{
    objects::{Descriptor, Key, Kind, Property},
    parser::{self, ExportImport, ImportName, ImportPhase, Module, RequestKey, SyntaxError},
    promises::{Capability, Internal, State},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id};
use std::collections::{HashMap, HashSet};
type Result<T> = std::result::Result<T, ScriptError>;

/// Test guards, not web limits: revisit before the Servo path relies on them.
const MODULE_LIMIT: usize = 128;
const SOURCE_LIMIT: usize = 1048576;

#[derive(Clone, Debug)]
pub struct ModuleSource {
    pub url: String,
    pub source: String,
    pub resolutions: Vec<(String, String)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleStatus {
    Evaluating,
    Evaluated,
    Errored(String),
}
/// The referrer of a HostLoadImportedModule request (§16.2.1.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Referrer {
    Script { host_defined: u64 },
    Module { url: String, host_defined: u64 },
    Realm,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRequest {
    pub id: u64,
    pub referrer: Referrer,
    pub specifier: String,
    pub attributes: Vec<(String, String)>,
    /// [[Phase]] of the first request for (referrer, specifier). Informative: the
    /// host's answer must not depend on it (IMPORT_PHASES.md).
    pub phase: ImportPhase,
}
/// The host's answer to a request (FinishLoadingImportedModule's `result`).
#[derive(Clone, Debug)]
pub enum ModuleLoad {
    /// Source text to parse; the engine raises its own SyntaxError.
    Source {
        url: String,
        module_type: String,
        source: String,
        host_defined: u64,
    },
    /// A host-provided module with a [[ModuleSource]] object (the shape of a Wasm
    /// module record). It has no requests and no exports; IMPORT_PHASES.md.
    ModuleSource {
        url: String,
        class_name: String,
        host_defined: u64,
    },
    /// A host failure; the engine throws a new `kind` error (HTML: TypeError).
    Error { kind: String, message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    New,
    Unlinked,
    Linking,
    Linked,
    Evaluating,
    EvaluatingAsync,
    Evaluated,
}
/// [[AsyncEvaluationOrder]]: unset, an integer, or done.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Unset,
    Pending(u64),
    Done,
}
struct Record {
    url: String,
    host_defined: u64,
    syntax: Module,
    /// [[LoadedModules]]: ModuleRequest (specifier and attributes) → record.
    loaded: HashMap<RequestKey, usize>,
    /// A JSON module (CreateDefaultExportSyntheticModule over ParseJSONModule): the
    /// source text, validated at load and materialized into the "default" binding
    /// when the environment is created (JSON_MODULES.md).
    json: Option<String>,
    status: Status,
    /// [[EvaluationError]]; the value is also kept alive by a hidden env binding.
    error: Option<Value>,
    dfs_index: usize,
    dfs_ancestor: usize,
    cycle_root: usize,
    has_tla: bool,
    order: Order,
    async_parents: Vec<usize>,
    pending_async: usize,
    /// [[TopLevelCapability]], rooted through hidden bindings of `env`.
    top: Option<Capability>,
    env: Option<Id>,
    /// The namespace object and its sorted [[Exports]], once created.
    namespace: Option<(Id, Vec<String>)>,
    /// [[DeferredNamespace]]: the deferred namespace and its [[Exports]] without "then".
    deferred: Option<(Id, Vec<String>)>,
    /// [[ModuleSource]] and its [[ModuleSourceClassName]]; ~empty~ for source text.
    source: Option<(Id, String)>,
    /// [[ImportMeta]], created lazily (§13.3.12.1); rooted by a hidden env binding.
    meta: Option<Id>,
}
/// GraphLoadingState (§16.2.1.6.1); the promise is replaced by `graph_loaded`.
struct Graph {
    root: usize,
    visited: HashSet<usize>,
    pending: usize,
    loading: bool,
    /// The `import()` slot whose LoadRequestedModules this graph is.
    import: Option<usize>,
}
/// A request in flight. `referrer` is None for a Script or Realm referrer.
struct Pending {
    referrer: Option<usize>,
    key: RequestKey,
    graphs: Vec<(usize, bool)>,
    imports: Vec<usize>,
}
/// Who waits on a HostLoadImportedModule request: a loading graph (with `true` for a
/// source-phase request, loaded without its own requests) or an `import()`.
enum Waiter {
    Graph(usize, bool),
    Import(usize),
}
/// A pending `import()` (§13.3.10.2 ContinueDynamicImport): its capability, the
/// loading promise and the module, held by external roots until it settles.
struct DynamicImport {
    cap: Capability,
    load: Option<Capability>,
    module: Option<usize>,
    roots: Vec<ExternalRoot>,
    phase: ImportPhase,
    /// `import.defer()`: evaluation promises not yet settled, and whether the
    /// import promise already settled (a rejection settles it early).
    waiting: usize,
    settled: bool,
}
/// The reaction steps of ContinueDynamicImport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DynamicStep {
    /// linkAndEvaluateClosure (step 6).
    Loaded,
    /// onFulfilled of the evaluation promise (step 6.e).
    Evaluated,
    /// rejectedClosure (step 3).
    Rejected,
    /// `import.defer()`: one asynchronous dependency's evaluation fulfilled.
    DeferFulfilled,
    /// `import.defer()`: one asynchronous dependency's evaluation rejected.
    DeferRejected,
}
/// A failed load, kept as data so several waiting graphs can each throw it.
#[derive(Clone)]
enum Failure {
    Syntax(SyntaxError),
    Error(String, String),
}
/// [[BindingName]] of a ResolvedBinding Record.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Binding {
    Name(String),
    Namespace,
    DeferredNamespace,
    Source,
}
enum Resolution {
    Null,
    Ambiguous,
    /// ResolvedBinding { [[Module]], [[BindingName]] }.
    Binding(usize, Binding),
}
#[derive(Default)]
pub(super) struct Registry {
    records: Vec<Record>,
    /// The host's module map key: (url, module type).
    keys: HashMap<(String, String), usize>,
    graphs: Vec<Graph>,
    pending: HashMap<u64, Pending>,
    inflight: HashMap<(Option<usize>, RequestKey), u64>,
    /// [[LoadedModules]] of Script and Realm referrers (one host-wide map).
    script_loaded: HashMap<RequestKey, usize>,
    /// Per-realm prototypes of host module source objects, by class name.
    source_protos: HashMap<String, Id>,
    dynamic: Vec<Option<DynamicImport>>,
    outbox: Vec<ModuleRequest>,
    next_request: u64,
    next_order: u64,
    bytes: usize,
    script_host_defined: u64,
}
impl Registry {
    /// The external roots of every unsettled `import()`, for teardown.
    pub(super) fn dynamic_roots(self) -> Vec<ExternalRoot> {
        self.dynamic
            .into_iter()
            .flatten()
            .flat_map(|d| d.roots)
            .collect()
    }
    fn index(&self, url: &str) -> Result<usize> {
        self.keys
            .get(&(url.to_string(), "javascript".to_string()))
            .copied()
            .ok_or(ScriptError::Runtime("unknown module"))
    }
    fn dependency(&self, m: usize, key: &RequestKey) -> Result<usize> {
        self.records[m]
            .loaded
            .get(key)
            .copied()
            .ok_or(ScriptError::Runtime("module request not loaded"))
    }
    /// ResolveExport (§16.2.1.7.3); `set` is the shared resolveSet.
    fn resolve_export(&self, m: usize, name: &str, set: &mut Vec<(usize, String)>) -> Resolution {
        if set.iter().any(|(r, n)| *r == m && n == name) {
            return Resolution::Null; // circular import request
        }
        set.push((m, name.to_string()));
        let syntax = &self.records[m].syntax;
        for e in &syntax.local_exports {
            if e.export_name.as_deref() == Some(name) {
                return Resolution::Binding(m, Binding::Name(e.local.clone().unwrap_or_default()));
            }
        }
        for e in &syntax.indirect_exports {
            if e.export_name.as_deref() == Some(name) {
                let Ok(dep) = self.dependency(m, &e.request.clone().unwrap_or_default()) else {
                    return Resolution::Null;
                };
                return match &e.import_name {
                    Some(ExportImport::All) => Resolution::Binding(dep, Binding::Namespace),
                    Some(ExportImport::DeferredNamespace) => {
                        Resolution::Binding(dep, Binding::DeferredNamespace)
                    }
                    Some(ExportImport::Source) => Resolution::Binding(dep, Binding::Source),
                    Some(ExportImport::Name(n)) => self.resolve_export(dep, n, set),
                    _ => Resolution::Null,
                };
            }
        }
        if name == "default" {
            return Resolution::Null;
        }
        let mut star: Option<(usize, Binding)> = None;
        for e in &syntax.star_exports {
            let Ok(dep) = self.dependency(m, &e.request.clone().unwrap_or_default()) else {
                continue;
            };
            match self.resolve_export(dep, name, set) {
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Null => {}
                Resolution::Binding(r, b) => match &star {
                    None => star = Some((r, b)),
                    Some((sr, sb)) => {
                        if *sr != r || *sb != b {
                            return Resolution::Ambiguous;
                        }
                    }
                },
            }
        }
        match star {
            Some((r, b)) => Resolution::Binding(r, b),
            None => Resolution::Null,
        }
    }
    /// GetExportedNames (§16.2.1.7.2).
    pub(super) fn exported_names(&self, m: usize, set: &mut HashSet<usize>) -> Vec<String> {
        if !set.insert(m) {
            return vec![];
        }
        let syntax = &self.records[m].syntax;
        let mut names: Vec<String> = syntax
            .local_exports
            .iter()
            .chain(&syntax.indirect_exports)
            .filter_map(|e| e.export_name.clone())
            .collect();
        for e in &syntax.star_exports {
            let Ok(dep) = self.dependency(m, &e.request.clone().unwrap_or_default()) else {
                continue;
            };
            for n in self.exported_names(dep, set) {
                if n != "default" && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        names
    }
}
/// HostGetSupportedImportAttributes (§16.2.1.12.1): the HTML host's « "type" ».
const SUPPORTED_IMPORT_ATTRIBUTES: &[&str] = &["type"];
/// AllImportAttributesSupported (§16.2.1.12.2): the first key outside the list.
fn unsupported_attribute(key: &RequestKey) -> Option<&str> {
    key.attributes
        .iter()
        .map(|(k, _)| k.as_str())
        .find(|k| !SUPPORTED_IMPORT_ATTRIBUTES.contains(k))
}
impl Failure {
    /// A load failure caused by syntax or a module type this engine does not implement:
    /// the host sees an Unsupported error instead of a catchable rejection.
    fn unsupported(&self) -> Option<ScriptError> {
        match self {
            Failure::Syntax(s)
                if s.message.contains("unsupported") || s.message.ends_with("limit") =>
            {
                Some(ScriptError::Syntax(s.clone()))
            }
            Failure::Error(kind, m) if kind == "SyntaxError" && m.contains("unsupported") => Some(
                ScriptError::Unsupported("unsupported module syntax or type"),
            ),
            _ => None,
        }
    }
    fn script_error(&self, e: &mut Engine<'_>) -> ScriptError {
        match self {
            Failure::Syntax(s) => ScriptError::Syntax(s.clone()),
            Failure::Error(kind, message) => match e.throw_error::<()>(kind, message) {
                Err(err) => err,
                Ok(()) => ScriptError::Runtime("module load failure"),
            },
        }
    }
}
impl Engine<'_> {
    /// Compatibility host: answers every request from `sources` (whole graph up front).
    pub fn execute_modules(&mut self, entry: &str, sources: &[ModuleSource]) -> Result<()> {
        if sources.len() > MODULE_LIMIT
            || sources.iter().map(|s| s.source.len()).sum::<usize>() > SOURCE_LIMIT
        {
            return Err(ScriptError::Runtime("module source limit"));
        }
        let mut input = HashMap::new();
        for s in sources {
            if input.insert(s.url.as_str(), s).is_some() {
                return Err(ScriptError::Runtime("duplicate module URL"));
            }
        }
        let first = input
            .get(entry)
            .ok_or_else(|| ScriptError::Host(format!("missing module source: {entry}")))?;
        self.execute("")?;
        let result = (|| {
            self.execute_module(entry, &first.source, 0)?;
            loop {
                let requests = self.take_module_requests();
                if requests.is_empty() {
                    break;
                }
                for r in requests {
                    let Referrer::Module { url, .. } = &r.referrer else {
                        return Err(ScriptError::Host("unexpected module referrer".into()));
                    };
                    let target = input
                        .get(url.as_str())
                        .and_then(|s| s.resolutions.iter().find(|(s, _)| *s == r.specifier))
                        .map(|(_, u)| u.as_str())
                        .ok_or_else(|| {
                            ScriptError::Host(format!(
                                "unresolved module specifier: {}",
                                r.specifier
                            ))
                        })?;
                    let source = input.get(target).ok_or_else(|| {
                        ScriptError::Host(format!("missing module source: {target}"))
                    })?;
                    self.finish_module_request(
                        r.id,
                        ModuleLoad::Source {
                            url: target.into(),
                            module_type: "javascript".into(),
                            source: source.source.clone(),
                            host_defined: 0,
                        },
                    )?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.release_globals();
        } else {
            self.collect();
        }
        result
    }
    /// Load, link and evaluate a module graph rooted at `url` (HTML "run a module
    /// script"). Load, parse and link errors of this graph are returned; evaluation
    /// errors are reported through `module_status`.
    pub fn execute_module(&mut self, url: &str, source: &str, host_defined: u64) -> Result<()> {
        if self.global.is_none() {
            self.execute("")?;
        }
        let root = self.module_record(url, "javascript", source, host_defined)?;
        let g = self.modules.graphs.len();
        self.modules.graphs.push(Graph {
            root,
            visited: HashSet::new(),
            pending: 1,
            loading: true,
            import: None,
        });
        self.frame(|e| e.inner_module_loading(g, root))
    }
    pub fn set_script_host_defined(&mut self, host_defined: u64) {
        self.modules.script_host_defined = host_defined;
    }
    pub fn take_module_requests(&mut self) -> Vec<ModuleRequest> {
        std::mem::take(&mut self.modules.outbox)
    }
    /// Requests the host has not answered yet (taken or not).
    pub fn pending_module_requests(&self) -> usize {
        self.modules.pending.len()
    }
    /// FinishLoadingImportedModule (§16.2.1.10). An unknown or already settled id
    /// is a host error, never a panic.
    pub fn finish_module_request(&mut self, id: u64, load: ModuleLoad) -> Result<()> {
        let Some(p) = self.modules.pending.remove(&id) else {
            return Err(ScriptError::Host(format!(
                "unknown or settled module request {id}"
            )));
        };
        self.modules.inflight.remove(&(p.referrer, p.key.clone()));
        self.modules.outbox.retain(|r| r.id != id);
        let result = match load {
            ModuleLoad::Source {
                url,
                module_type,
                source,
                host_defined,
            } => match self.module_record(&url, &module_type, &source, host_defined) {
                Ok(i) => Ok(i),
                Err(ScriptError::Syntax(s)) => Err(Failure::Syntax(s)),
                Err(ScriptError::Unsupported(m)) => {
                    Err(Failure::Error("SyntaxError".into(), m.into()))
                }
                Err(other) => return Err(other),
            },
            ModuleLoad::ModuleSource {
                url,
                class_name,
                host_defined,
            } => Ok(self.module_source_record(&url, &class_name, host_defined)?),
            ModuleLoad::Error { kind, message } => Err(Failure::Error(kind, message)),
        };
        if let Ok(dep) = result {
            let loaded = match p.referrer {
                Some(r) => &mut self.modules.records[r].loaded,
                None => &mut self.modules.script_loaded,
            };
            match loaded.get(&p.key) {
                Some(&prev) if prev != dep => {
                    return Err(ScriptError::Host(
                        "host loaded two modules for one request".into(),
                    ))
                }
                _ => {
                    loaded.insert(p.key.clone(), dep);
                }
            }
        }
        let mut first = None;
        for (g, single) in p.graphs {
            if let Err(err) = self.frame(|e| e.continue_module_loading(g, single, result.clone())) {
                first.get_or_insert(err);
            }
        }
        for slot in p.imports {
            if let Err(err) = self.frame(|e| e.continue_dynamic_import(slot, result.clone())) {
                first.get_or_insert(err);
            }
        }
        first.map_or(Ok(()), Err)
    }
    /// ParseModule plus the host's module map: one record per (url, type).
    fn module_record(
        &mut self,
        url: &str,
        module_type: &str,
        source: &str,
        host_defined: u64,
    ) -> Result<usize> {
        let key = (url.to_string(), module_type.to_string());
        if let Some(&i) = self.modules.keys.get(&key) {
            return Ok(i);
        }
        if module_type != "javascript" && module_type != "json" {
            return Err(ScriptError::Unsupported("unsupported module type"));
        }
        if self.modules.records.len() >= MODULE_LIMIT {
            return Err(ScriptError::Runtime("module graph limit"));
        }
        self.modules.bytes += source.len();
        if self.modules.bytes > SOURCE_LIMIT {
            return Err(ScriptError::Runtime("module source limit"));
        }
        if module_type == "json" {
            // ParseJSONModule step 1: Call(%JSON.parse%, undefined, « source »); a
            // syntax error fails the load. CreateDefaultExportSyntheticModule: one
            // "default" export and no requests.
            super::json::json_validate(source).map_err(ScriptError::Syntax)?;
            let mut syntax = parser::parse_module("").map_err(ScriptError::Syntax)?;
            syntax.local_exports.push(parser::Export {
                export_name: Some("default".into()),
                request: None,
                import_name: None,
                local: Some("\0default".into()),
            });
            let i = self.module_record_from(url, key, syntax, host_defined)?;
            self.modules.records[i].json = Some(source.to_string());
            return Ok(i);
        }
        let syntax = parser::parse_module(source).map_err(ScriptError::Syntax)?;
        self.module_record_from(url, key, syntax, host_defined)
    }
    fn module_record_from(
        &mut self,
        url: &str,
        key: (String, String),
        syntax: Module,
        host_defined: u64,
    ) -> Result<usize> {
        let has_tla = syntax.body.iter().any(super::asynchronous::stmt_await);
        let i = self.modules.records.len();
        self.modules.keys.insert(key, i);
        self.modules.records.push(Record {
            url: url.into(),
            host_defined,
            syntax,
            loaded: HashMap::new(),
            json: None,
            status: Status::New,
            error: None,
            dfs_index: 0,
            dfs_ancestor: 0,
            cycle_root: i,
            has_tla,
            order: Order::Unset,
            async_parents: vec![],
            pending_async: 0,
            top: None,
            env: None,
            namespace: None,
            deferred: None,
            source: None,
            meta: None,
        });
        Ok(i)
    }
    /// A host module with a [[ModuleSource]] (ModuleLoad::ModuleSource): an empty
    /// record whose source object inherits from a per-realm prototype for
    /// `class_name`, itself inheriting from %AbstractModuleSource.prototype%.
    fn module_source_record(
        &mut self,
        url: &str,
        class_name: &str,
        host_defined: u64,
    ) -> Result<usize> {
        let key = (url.to_string(), format!("source:{class_name}"));
        if let Some(&i) = self.modules.keys.get(&key) {
            return Ok(i);
        }
        if self.modules.records.len() >= MODULE_LIMIT {
            return Err(ScriptError::Runtime("module graph limit"));
        }
        let syntax = parser::parse_module("").map_err(ScriptError::Syntax)?;
        let i = self.module_record_from(url, key, syntax, host_defined)?;
        let global = self.runtime.resolve(self.global.unwrap())?;
        let abstract_proto = self.intrinsics.as_ref().unwrap().collection["AbstractModuleSource"];
        self.frame(|e| {
            let proto = match e.modules.source_protos.get(class_name) {
                Some(&p) => p,
                None => {
                    let p = e.new_object(Some(abstract_proto))?;
                    e.runtime.link(global, p)?;
                    e.modules.source_protos.insert(class_name.into(), p);
                    p
                }
            };
            let object = e.new_object(Some(proto))?;
            e.runtime.link(global, object)?;
            e.modules.records[i].source = Some((object, class_name.into()));
            Ok(i)
        })
    }
    /// InnerModuleLoading (§16.2.1.6.1.1).
    fn inner_module_loading(&mut self, g: usize, m: usize) -> Result<()> {
        if self.modules.records[m].status == Status::New && self.modules.graphs[g].visited.insert(m)
        {
            let requests = self.modules.records[m].syntax.requests.clone();
            self.modules.graphs[g].pending += requests.len();
            for (key, phase) in requests {
                // A source-phase request loads the module but not its requests.
                let single = phase == ImportPhase::Source;
                if let Some(attribute) = unsupported_attribute(&key) {
                    // Step 2.c.i: AllImportAttributesSupported is false.
                    let message = format!("Import attribute \"{attribute}\" is not supported");
                    let failure = Failure::Error("SyntaxError".into(), message);
                    self.continue_module_loading(g, single, Err(failure))?;
                } else if let Some(&dep) = self.modules.records[m].loaded.get(&key) {
                    if single {
                        self.graph_request_done(g)?;
                    } else {
                        self.inner_module_loading(g, dep)?;
                    }
                } else {
                    self.host_load_imported_module(Some(m), key, phase, Waiter::Graph(g, single));
                }
                if !self.modules.graphs[g].loading {
                    return Ok(());
                }
            }
        }
        self.graph_request_done(g)
    }
    /// InnerModuleLoading steps 3-4: one request of the graph is done.
    fn graph_request_done(&mut self, g: usize) -> Result<()> {
        let graph = &mut self.modules.graphs[g];
        graph.pending -= 1;
        if graph.pending == 0 {
            graph.loading = false;
            let visited: Vec<usize> = graph.visited.iter().copied().collect();
            for v in visited {
                if self.modules.records[v].status == Status::New {
                    self.modules.records[v].status = Status::Unlinked;
                }
            }
            self.graph_loaded(g)?;
        }
        Ok(())
    }
    /// HostLoadImportedModule (§16.2.1.10): queue a request for the host. A second
    /// graph waiting on the same (referrer, specifier) joins the request in flight.
    fn host_load_imported_module(
        &mut self,
        m: Option<usize>,
        key: RequestKey,
        phase: ImportPhase,
        w: Waiter,
    ) {
        let r = &mut self.modules;
        let id = match r.inflight.get(&(m, key.clone())) {
            Some(id) => *id,
            None => {
                let id = r.next_request;
                r.next_request += 1;
                r.inflight.insert((m, key.clone()), id);
                r.outbox.push(ModuleRequest {
                    id,
                    referrer: match m {
                        Some(m) => Referrer::Module {
                            url: r.records[m].url.clone(),
                            host_defined: r.records[m].host_defined,
                        },
                        None => Referrer::Script {
                            host_defined: r.script_host_defined,
                        },
                    },
                    specifier: key.specifier.clone(),
                    attributes: key.attributes.clone(),
                    phase,
                });
                r.pending.insert(
                    id,
                    Pending {
                        referrer: m,
                        key,
                        graphs: vec![],
                        imports: vec![],
                    },
                );
                id
            }
        };
        let p = r.pending.get_mut(&id).unwrap();
        match w {
            Waiter::Graph(g, single) => p.graphs.push((g, single)),
            Waiter::Import(slot) => p.imports.push(slot),
        }
    }
    /// ContinueModuleLoading (§16.2.1.6.1.2).
    fn continue_module_loading(
        &mut self,
        g: usize,
        single: bool,
        result: std::result::Result<usize, Failure>,
    ) -> Result<()> {
        if !self.modules.graphs[g].loading {
            return Ok(());
        }
        match result {
            Ok(_) if single => self.graph_request_done(g),
            Ok(m) => self.inner_module_loading(g, m),
            Err(failure) => {
                self.modules.graphs[g].loading = false;
                match self.modules.graphs[g].import {
                    // An `import()` graph rejects its loading promise instead.
                    Some(slot) => {
                        let reason = self.failure_value(&failure)?;
                        self.settle_load(slot, Err(reason))
                    }
                    // Unimplemented syntax or module types never become a catchable error.
                    None => Err(failure
                        .unsupported()
                        .unwrap_or_else(|| failure.script_error(self))),
                }
            }
        }
    }
    /// The loading promise of a top-level graph fulfilled: Link, then Evaluate.
    fn graph_loaded(&mut self, g: usize) -> Result<()> {
        if let Some(slot) = self.modules.graphs[g].import {
            return self.settle_load(slot, Ok(()));
        }
        let root = self.modules.graphs[g].root;
        self.link(root)?;
        let promise = self.evaluate(root)?;
        if let Value::Object(p) = promise {
            // Evaluation errors of a top-level graph are read through module_status.
            self.mark_handled(p)?;
        }
        Ok(())
    }
    /// Link (§16.2.1.6.2).
    fn link(&mut self, m: usize) -> Result<()> {
        let mut stack = vec![];
        let result = self.inner_module_linking(m, &mut stack, 0);
        if result.is_err() {
            for s in stack {
                self.modules.records[s].status = Status::Unlinked;
                self.modules.records[s].env = None;
            }
        }
        result.map(|_| ())
    }
    /// InnerModuleLinking (§16.2.1.6.2.1).
    fn inner_module_linking(
        &mut self,
        m: usize,
        stack: &mut Vec<usize>,
        mut index: usize,
    ) -> Result<usize> {
        match self.modules.records[m].status {
            Status::Linking
            | Status::Linked
            | Status::Evaluating
            | Status::EvaluatingAsync
            | Status::Evaluated => return Ok(index),
            Status::New => return Err(ScriptError::Runtime("module not loaded")),
            Status::Unlinked => {}
        }
        let r = &mut self.modules.records[m];
        r.status = Status::Linking;
        r.dfs_index = index;
        r.dfs_ancestor = index;
        index += 1;
        stack.push(m);
        // The environment exists before the dependencies link, so a cycle member's
        // import alias can name it; InitializeEnvironment fills it in afterwards.
        self.create_module_environment(m)?;
        // Source-phase requests are not linked (IMPORT_PHASES.md, Linking).
        for (specifier, phase) in self.modules.records[m].syntax.requests.clone() {
            if phase == ImportPhase::Source {
                continue;
            }
            let dep = self.modules.dependency(m, &specifier)?;
            index = self.inner_module_linking(dep, stack, index)?;
            if self.modules.records[dep].status == Status::Linking {
                let a = self.modules.records[dep].dfs_ancestor;
                let r = &mut self.modules.records[m];
                r.dfs_ancestor = r.dfs_ancestor.min(a);
            }
        }
        self.initialize_environment(m)?;
        if self.modules.records[m].dfs_ancestor == self.modules.records[m].dfs_index {
            while let Some(x) = stack.pop() {
                self.modules.records[x].status = Status::Linked;
                if x == m {
                    break;
                }
            }
        }
        Ok(index)
    }
    fn create_module_environment(&mut self, m: usize) -> Result<()> {
        let global = self.runtime.resolve(self.global.unwrap())?;
        let body = self.modules.records[m].syntax.body.clone();
        let env = self.new_environment(Some(global), &body, true)?;
        self.runtime.link(global, env)?;
        self.modules.records[m].env = Some(env);
        self.initialize(env, "this", Value::Undefined)?;
        if let Some(source) = self.modules.records[m].json.clone() {
            // The synthetic module's evaluation steps (SetSyntheticModuleExport of
            // "default") run here: the record has no requests, so no code can observe
            // the binding before evaluation.
            let value = self.json_parse_text(&source)?;
            return self.initialize(env, "\0default", value);
        }
        self.hoist_functions(&body, env)
    }
    /// InitializeEnvironment (§16.2.1.7.4), import bindings part; the lexical and
    /// function declarations were instantiated by `create_module_environment`.
    fn initialize_environment(&mut self, m: usize) -> Result<()> {
        let syntax = &self.modules.records[m].syntax;
        let indirect: Vec<_> = syntax
            .indirect_exports
            .iter()
            .map(|e| {
                let request = e.request.clone().unwrap_or_default();
                (request.specifier, e.export_name.clone())
            })
            .collect();
        let imports = syntax.imports.clone();
        for (request, name) in indirect {
            let name = name.unwrap_or_default();
            match self.modules.resolve_export(m, &name, &mut vec![]) {
                Resolution::Binding(..) => {}
                Resolution::Null => return self.missing_export(&request, &name),
                Resolution::Ambiguous => return self.ambiguous_export(&request, &name),
            }
        }
        let env = self.modules.records[m].env.unwrap();
        for import in imports {
            let dep = self.modules.dependency(m, &import.request)?;
            let (target, binding) = match &import.import_name {
                ImportName::Namespace => (dep, Binding::Namespace),
                ImportName::DeferredNamespace => (dep, Binding::DeferredNamespace),
                ImportName::Source => (dep, Binding::Source),
                ImportName::Name(n) => match self.modules.resolve_export(dep, n, &mut vec![]) {
                    Resolution::Binding(t, b) => (t, b),
                    Resolution::Null => return self.missing_export(&import.request.specifier, n),
                    Resolution::Ambiguous => {
                        return self.ambiguous_export(&import.request.specifier, n)
                    }
                },
            };
            match binding {
                Binding::Namespace | Binding::DeferredNamespace | Binding::Source => {
                    let value = self.binding_value(target, &binding, &import.request.specifier)?;
                    self.initialize(env, &import.local, value)?;
                }
                Binding::Name(name) => {
                    let target = self.modules.records[target]
                        .env
                        .ok_or(ScriptError::Runtime("module environment missing"))?;
                    self.runtime.link(env, target)?;
                    if let Some(Object::Environment { aliases, .. }) = self.objects.get_mut(&env) {
                        aliases.insert(import.local.clone(), (target, name));
                    }
                }
            }
        }
        Ok(())
    }
    /// The value of a ~namespace~, ~deferred-namespace~ or ~source~ binding:
    /// GetModuleNamespace(target, phase), or GetModuleSource, which throws a
    /// SyntaxError for a module without a [[ModuleSource]] (IMPORT_PHASES.md).
    fn binding_value(&mut self, target: usize, binding: &Binding, request: &str) -> Result<Value> {
        match binding {
            Binding::Namespace => self.module_namespace(target, false),
            Binding::DeferredNamespace => self.module_namespace(target, true),
            Binding::Source => match &self.modules.records[target].source {
                Some((id, _)) => Ok(Value::Object(*id)),
                None => self.throw_error(
                    "SyntaxError",
                    &format!("The requested module '{request}' does not provide a module source"),
                ),
            },
            Binding::Name(_) => Err(ScriptError::Runtime("binding is not a module value")),
        }
    }
    fn missing_export<T>(&mut self, request: &str, name: &str) -> Result<T> {
        self.throw_error(
            "SyntaxError",
            &format!("The requested module '{request}' does not provide an export named '{name}'"),
        )
    }
    fn ambiguous_export<T>(&mut self, request: &str, name: &str) -> Result<T> {
        self.throw_error(
            "SyntaxError",
            &format!(
                "The requested module '{request}' contains conflicting star exports for name '{name}'"
            ),
        )
    }
    /// GetModuleNamespace (§16.2.1.12) and ModuleNamespaceCreate (§10.4.6.12), with
    /// the defer proposal's phase: a deferred namespace is a separate object whose
    /// [[Exports]] leave out "then".
    fn module_namespace(&mut self, m: usize, deferred: bool) -> Result<Value> {
        let slot = if deferred {
            &self.modules.records[m].deferred
        } else {
            &self.modules.records[m].namespace
        };
        if let Some((id, _)) = slot {
            return Ok(Value::Object(*id));
        }
        let env = self.modules.records[m]
            .env
            .ok_or(ScriptError::Runtime("module environment missing"))?;
        let mut names: Vec<String> = self
            .modules
            .exported_names(m, &mut HashSet::new())
            .into_iter()
            .filter(|n| {
                matches!(
                    self.modules.resolve_export(m, n, &mut vec![]),
                    Resolution::Binding(..)
                )
            })
            .collect();
        // [[Exports]] is ordered by code unit order (§10.4.6.12 step 6).
        names.sort_by_cached_key(|n| n.encode_utf16().collect::<Vec<u16>>());
        if deferred {
            names.retain(|n| n != "then");
        }
        self.frame(|e| {
            let id = e.new_object(None)?;
            e.protect(&Value::Object(id))?;
            // §28.3.1 @@toStringTag: "Module" ("Deferred Module"), non-writable,
            // non-enumerable, non-configurable.
            let tag = if deferred {
                "Deferred Module"
            } else {
                "Module"
            };
            e.define(
                id,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String(tag.into())),
                    ..Default::default()
                },
            )?;
            let mut data = e.data(id)?.clone();
            data.extensible = false;
            data.namespace = Some(m);
            e.replace_data(id, data)?;
            e.runtime.link(id, env)?;
            if deferred {
                e.initialize(env, "\0deferredNamespace", Value::Object(id))?;
                e.modules.records[m].deferred = Some((id, names));
            } else {
                e.initialize(env, "\0namespace", Value::Object(id))?;
                e.modules.records[m].namespace = Some((id, names));
            }
            Ok(Value::Object(id))
        })
    }
    fn is_deferred_namespace(&self, id: Id, m: usize) -> bool {
        self.modules
            .records
            .get(m)
            .and_then(|r| r.deferred.as_ref())
            .is_some_and(|(d, _)| *d == id)
    }
    /// The namespace dispatch of a string-keyed [[GetOwnProperty]], [[DefineOwnProperty]],
    /// [[HasProperty]], [[Get]] or [[Delete]]. IsSymbolLikeNamespaceKey keys (symbols, and
    /// "then" on a deferred namespace) return None and take the ordinary path; any other
    /// key of a deferred namespace first runs GetModuleExportsList, which may evaluate.
    pub(super) fn namespace_key(&mut self, id: Id, key: &Key) -> Result<Option<(usize, String)>> {
        let Key::String(s) = key else {
            return Ok(None);
        };
        let Some(m) = self.data(id)?.namespace else {
            return Ok(None);
        };
        if self.is_deferred_namespace(id, m) {
            if s == "then" {
                return Ok(None);
            }
            self.ensure_deferred_evaluation(m)?;
        }
        Ok(Some((m, s.clone())))
    }
    /// GetModuleExportsList (defer proposal) steps 1.a-b, as quoted in test262
    /// `import-defer/evaluation-triggers`: TypeError unless the module is evaluated or
    /// ReadyForSyncExecution; then EvaluateSync, which throws a rejection's reason.
    fn ensure_deferred_evaluation(&mut self, m: usize) -> Result<()> {
        if self.modules.records[m].status != Status::Evaluated
            && !self.ready_for_sync_execution(m, &mut HashSet::new())
        {
            return self.throw_error(
                "TypeError",
                "deferred module cannot be evaluated synchronously while it or a dependency is evaluating or has top-level await",
            );
        }
        // EvaluateModuleSync on an ~evaluated~ module (the draft's step 1): its
        // [[EvaluationError]], if any. Its Evaluate() promise may never settle here,
        // for a TLA module whose async evaluation already finished.
        if self.modules.records[m].status == Status::Evaluated {
            return match self.modules.records[m].error.clone() {
                Some(error) => Err(self.thrown(error)),
                None => Ok(()),
            };
        }
        let Value::Object(p) = self.evaluate(m)? else {
            return Err(ScriptError::Runtime("module evaluation without promise"));
        };
        self.mark_handled(p)?;
        match self.data(p)?.promise.as_ref().map(|r| r.state.clone()) {
            Some(State::Fulfilled(_)) => Ok(()),
            Some(State::Rejected(error)) => Err(self.thrown(error)),
            _ => Err(ScriptError::Runtime(
                "synchronous module evaluation suspended",
            )),
        }
    }
    /// IsModuleSCCEvaluated (defer proposal, as quoted in test262): the status of
    /// [[CycleRoot]] once evaluation set it, else the module's own status.
    fn is_scc_evaluated(&self, m: usize) -> bool {
        let r = &self.modules.records[m];
        let root = match r.status {
            Status::Evaluating | Status::EvaluatingAsync | Status::Evaluated => r.cycle_root,
            _ => m,
        };
        self.modules.records[root].status == Status::Evaluated
    }
    /// ReadyForSyncExecution (defer proposal); source-phase requests are not
    /// evaluated and are skipped.
    fn ready_for_sync_execution(&self, m: usize, seen: &mut HashSet<usize>) -> bool {
        if !seen.insert(m) {
            return true;
        }
        let r = &self.modules.records[m];
        if self.is_scc_evaluated(m) {
            return true;
        }
        if matches!(r.status, Status::Evaluating | Status::EvaluatingAsync) || r.has_tla {
            return false;
        }
        r.syntax.requests.iter().all(|(specifier, phase)| {
            *phase == ImportPhase::Source
                || self
                    .modules
                    .dependency(m, specifier)
                    .is_ok_and(|dep| self.ready_for_sync_execution(dep, seen))
        })
    }
    /// GatherAsynchronousTransitiveDependencies (defer proposal), skipping
    /// source-phase requests.
    fn gather_async_dependencies(&self, m: usize, seen: &mut Vec<usize>, out: &mut Vec<usize>) {
        if seen.contains(&m) {
            return;
        }
        seen.push(m);
        let r = &self.modules.records[m];
        if r.status == Status::Evaluating || self.is_scc_evaluated(m) {
            return;
        }
        if r.has_tla {
            if !out.contains(&m) {
                out.push(m);
            }
            return;
        }
        for (specifier, phase) in &r.syntax.requests {
            if *phase == ImportPhase::Source {
                continue;
            }
            if let Ok(dep) = self.modules.dependency(m, specifier) {
                self.gather_async_dependencies(dep, seen, out);
            }
        }
    }
    fn mark_handled(&mut self, p: Id) -> Result<()> {
        let mut data = self.data(p)?.clone();
        if let Some(record) = data.promise.as_mut() {
            record.handled = true;
        }
        self.replace_data(p, data)?;
        self.unhandled.remove(&p);
        Ok(())
    }
    pub(super) fn namespace_has(&self, m: usize, name: &str) -> bool {
        self.modules
            .records
            .get(m)
            .and_then(|r| r.namespace.as_ref().or(r.deferred.as_ref()))
            .is_some_and(|(_, names)| names.iter().any(|n| n == name))
    }
    /// Module namespace [[Get]] (§10.4.6.8) for a string key.
    pub(super) fn namespace_get(&mut self, m: usize, name: &str) -> Result<Value> {
        if !self.namespace_has(m, name) {
            return Ok(Value::Undefined);
        }
        match self.modules.resolve_export(m, name, &mut vec![]) {
            Resolution::Binding(t, Binding::Name(binding)) => {
                let Some(env) = self.modules.records[t].env else {
                    return self.throw_error("ReferenceError", &format!("{name} is not defined"));
                };
                self.lookup(env, &binding)
            }
            Resolution::Binding(t, binding) => self.binding_value(t, &binding, name),
            _ => Err(ScriptError::Runtime("namespace export no longer resolves")),
        }
    }
    /// Module namespace [[GetOwnProperty]] (§10.4.6.5) for a string key.
    pub(super) fn namespace_own(&mut self, m: usize, name: &str) -> Result<Option<Property>> {
        if !self.namespace_has(m, name) {
            return Ok(None);
        }
        let value = self.namespace_get(m, name)?;
        Ok(Some(Property::data(value, true, true, false)))
    }
    /// Module namespace [[DefineOwnProperty]] (§10.4.6.6) for a string key.
    pub(super) fn namespace_define(&mut self, m: usize, name: &str, d: Descriptor) -> Result<bool> {
        let Some(current) = self.namespace_own(m, name)? else {
            return Ok(false);
        };
        if d.configurable == Some(true)
            || d.enumerable == Some(false)
            || d.get.is_some()
            || d.set.is_some()
            || d.writable == Some(false)
        {
            return Ok(false);
        }
        Ok(match (d.value, current.kind) {
            (Some(v), Kind::Data { value, .. }) => v.same_value(&value),
            _ => true,
        })
    }
    /// Module namespace [[OwnPropertyKeys]] (§10.4.6.11).
    pub(super) fn namespace_own_keys(&mut self, id: Id, m: usize) -> Result<Vec<Key>> {
        let deferred = self.is_deferred_namespace(id, m);
        if deferred {
            self.ensure_deferred_evaluation(m)?;
        }
        let mut keys: Vec<Key> = self
            .modules
            .records
            .get(m)
            .and_then(|r| {
                if deferred {
                    r.deferred.as_ref()
                } else {
                    r.namespace.as_ref()
                }
            })
            .map(|(_, names)| names.iter().map(|n| Key::String(n.clone())).collect())
            .unwrap_or_default();
        keys.extend(
            self.data(id)?
                .properties
                .iter()
                .filter(|(key, _)| matches!(key, Key::Symbol(_)) && !self.is_private(key))
                .map(|(key, _)| key.clone()),
        );
        Ok(keys)
    }
    /// Evaluate (§16.2.1.6.3): returns the [[TopLevelCapability]] promise.
    fn evaluate(&mut self, m: usize) -> Result<Value> {
        let mut module = m;
        if matches!(
            self.modules.records[m].status,
            Status::EvaluatingAsync | Status::Evaluated
        ) {
            module = self.modules.records[m].cycle_root;
        }
        if let Some(cap) = &self.modules.records[module].top {
            return Ok(cap.promise.clone());
        }
        let cap = self.new_capability()?;
        let env = self.modules.records[module].env.unwrap();
        for (n, v) in cap.values().into_iter().enumerate() {
            self.initialize(env, &format!("\0moduleTop{n}"), v)?;
        }
        self.modules.records[module].top = Some(cap.clone());
        let mut stack = vec![];
        match self.inner_module_evaluation(module, &mut stack, 0) {
            Ok(_) => {
                if self.modules.records[module].order == Order::Unset {
                    self.invoke(cap.resolve.clone(), vec![Value::Undefined])?;
                }
            }
            Err(ScriptError::Thrown(ex)) => {
                for s in stack {
                    self.modules.records[s].status = Status::Evaluated;
                    self.set_evaluation_error(s, ex.value.clone())?;
                }
                self.invoke(cap.reject.clone(), vec![ex.value])?;
            }
            Err(other) => return Err(other),
        }
        Ok(cap.promise)
    }
    fn new_capability(&mut self) -> Result<Capability> {
        let promise = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        self.capability(promise)
    }
    fn set_evaluation_error(&mut self, m: usize, error: Value) -> Result<()> {
        if let Some(env) = self.modules.records[m].env {
            self.initialize(env, "\0moduleError", error.clone())?;
        }
        self.modules.records[m].error = Some(error);
        Ok(())
    }
    /// InnerModuleEvaluation (§16.2.1.6.3.1).
    fn inner_module_evaluation(
        &mut self,
        m: usize,
        stack: &mut Vec<usize>,
        mut index: usize,
    ) -> Result<usize> {
        match self.modules.records[m].status {
            Status::EvaluatingAsync | Status::Evaluated => {
                return match self.modules.records[m].error.clone() {
                    None => Ok(index),
                    Some(v) => Err(self.thrown(v)),
                }
            }
            Status::Evaluating => return Ok(index),
            Status::Linked => {}
            _ => return Err(ScriptError::Runtime("module not linked")),
        }
        let r = &mut self.modules.records[m];
        r.status = Status::Evaluating;
        r.dfs_index = index;
        r.dfs_ancestor = index;
        r.pending_async = 0;
        index += 1;
        stack.push(m);
        // The evaluationList (defer proposal): an evaluation-phase request adds its
        // module, a defer request its asynchronous transitive dependencies, and a
        // source-phase request nothing.
        let mut list = vec![];
        for (specifier, phase) in self.modules.records[m].syntax.requests.clone() {
            let dep = self.modules.dependency(m, &specifier)?;
            match phase {
                ImportPhase::Evaluation => {
                    if !list.contains(&dep) {
                        list.push(dep);
                    }
                }
                ImportPhase::Defer => {
                    let mut found = vec![];
                    self.gather_async_dependencies(dep, &mut vec![], &mut found);
                    for d in found {
                        if !list.contains(&d) {
                            list.push(d);
                        }
                    }
                }
                ImportPhase::Source => {}
            }
        }
        for mut dep in list {
            index = self.inner_module_evaluation(dep, stack, index)?;
            if self.modules.records[dep].status == Status::Evaluating {
                let a = self.modules.records[dep].dfs_ancestor;
                let r = &mut self.modules.records[m];
                r.dfs_ancestor = r.dfs_ancestor.min(a);
            } else {
                dep = self.modules.records[dep].cycle_root;
                if let Some(v) = self.modules.records[dep].error.clone() {
                    return Err(self.thrown(v));
                }
            }
            if matches!(self.modules.records[dep].order, Order::Pending(_)) {
                self.modules.records[m].pending_async += 1;
                self.modules.records[dep].async_parents.push(m);
            }
        }
        let r = &self.modules.records[m];
        if r.pending_async > 0 || r.has_tla {
            let order = self.modules.next_order;
            self.modules.next_order += 1;
            self.modules.records[m].order = Order::Pending(order);
            if self.modules.records[m].pending_async == 0 {
                self.execute_async_module(m)?;
            }
        } else {
            self.execute_module_sync(m)?;
        }
        if self.modules.records[m].dfs_ancestor == self.modules.records[m].dfs_index {
            while let Some(x) = stack.pop() {
                let r = &mut self.modules.records[x];
                r.status = if matches!(r.order, Order::Pending(_)) {
                    Status::EvaluatingAsync
                } else {
                    Status::Evaluated
                };
                r.cycle_root = m;
                if x == m {
                    break;
                }
            }
        }
        Ok(index)
    }
    /// ExecuteModule: runs the body as a resumable program; returns its promise.
    fn run_module_body(&mut self, m: usize) -> Result<(Id, State)> {
        let env = self.modules.records[m].env.unwrap();
        let body = self.modules.records[m].syntax.body.clone();
        let cap = self.new_capability()?;
        for (n, v) in cap.values().into_iter().enumerate() {
            self.initialize(env, &format!("\0moduleCap{n}"), v)?;
        }
        let program = super::asynchronous::compile_module(&body)?;
        let Value::Object(p) = self.start_program(env, program, cap)? else {
            unreachable!()
        };
        let state = self.data(p)?.promise.as_ref().unwrap().state.clone();
        Ok((p, state))
    }
    fn execute_module_sync(&mut self, m: usize) -> Result<()> {
        let (p, state) = self.run_module_body(m)?;
        match state {
            State::Fulfilled(_) => Ok(()),
            State::Rejected(error) => {
                self.unhandled.remove(&p);
                Err(self.thrown(error))
            }
            State::Pending => Err(ScriptError::Runtime("module without await suspended")),
        }
    }
    /// ExecuteAsyncModule (§16.2.1.6.3.2).
    fn execute_async_module(&mut self, m: usize) -> Result<()> {
        let (p, _) = self.run_module_body(m)?;
        let yes = self.internal_function(
            Internal::ModuleSettled {
                index: m,
                reject: false,
            },
            1,
        )?;
        let no = self.internal_function(
            Internal::ModuleSettled {
                index: m,
                reject: true,
            },
            1,
        )?;
        self.attach_reaction(p, Some(yes), Some(no), None)?;
        self.unhandled.remove(&p);
        Ok(())
    }
    pub(super) fn module_settled(
        &mut self,
        index: usize,
        value: Value,
        reject: bool,
    ) -> Result<Value> {
        if index >= self.modules.records.len() {
            return Err(ScriptError::Runtime("invalid module continuation"));
        }
        if reject {
            self.async_module_rejected(index, value)?;
        } else {
            self.async_module_fulfilled(index)?;
        }
        Ok(Value::Undefined)
    }
    /// GatherAvailableAncestors (§16.2.1.6.3.3).
    fn gather_available_ancestors(&mut self, m: usize, exec: &mut Vec<usize>) {
        for p in self.modules.records[m].async_parents.clone() {
            let root = self.modules.records[p].cycle_root;
            if !exec.contains(&p) && self.modules.records[root].error.is_none() {
                let r = &mut self.modules.records[p];
                r.pending_async -= 1;
                if r.pending_async == 0 {
                    exec.push(p);
                    if !r.has_tla {
                        self.gather_available_ancestors(p, exec);
                    }
                }
            }
        }
    }
    /// AsyncModuleExecutionFulfilled (§16.2.1.6.3.4).
    fn async_module_fulfilled(&mut self, m: usize) -> Result<()> {
        if self.modules.records[m].status == Status::Evaluated {
            return Ok(());
        }
        self.modules.records[m].order = Order::Done;
        self.modules.records[m].status = Status::Evaluated;
        if let Some(cap) = self.modules.records[m].top.clone() {
            self.invoke(cap.resolve, vec![Value::Undefined])?;
        }
        let mut exec = vec![];
        self.gather_available_ancestors(m, &mut exec);
        exec.sort_by_key(|&x| match self.modules.records[x].order {
            Order::Pending(n) => n,
            _ => u64::MAX,
        });
        for x in exec {
            if self.modules.records[x].status == Status::Evaluated {
                continue;
            }
            if self.modules.records[x].has_tla {
                self.execute_async_module(x)?;
                continue;
            }
            match self.execute_module_sync(x) {
                Ok(()) => {
                    self.modules.records[x].order = Order::Done;
                    self.modules.records[x].status = Status::Evaluated;
                    if let Some(cap) = self.modules.records[x].top.clone() {
                        self.invoke(cap.resolve, vec![Value::Undefined])?;
                    }
                }
                Err(ScriptError::Thrown(ex)) => self.async_module_rejected(x, ex.value)?,
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }
    /// AsyncModuleExecutionRejected (§16.2.1.6.3.5).
    fn async_module_rejected(&mut self, m: usize, error: Value) -> Result<()> {
        if self.modules.records[m].status == Status::Evaluated {
            return Ok(());
        }
        self.set_evaluation_error(m, error.clone())?;
        self.modules.records[m].status = Status::Evaluated;
        self.modules.records[m].order = Order::Done;
        // Steps 8-9: the module's own [[TopLevelCapability]] rejects before its
        // [[AsyncParentModules]] (test262 module-code/top-level-await/rejection-order).
        if let Some(cap) = self.modules.records[m].top.clone() {
            self.invoke(cap.reject, vec![error.clone()])?;
        }
        for p in self.modules.records[m].async_parents.clone() {
            self.async_module_rejected(p, error.clone())?;
        }
        Ok(())
    }
    pub fn loaded_modules(&self) -> Vec<String> {
        self.modules.records.iter().map(|r| r.url.clone()).collect()
    }
    pub fn module_status(&mut self, url: &str) -> Result<ModuleStatus> {
        let i = self.modules.index(url)?;
        let r = &self.modules.records[i];
        let error = r.error.clone().or_else(|| match r.status {
            Status::Evaluated | Status::EvaluatingAsync => {
                self.modules.records[r.cycle_root].error.clone()
            }
            _ => None,
        });
        if let Some(error) = error {
            return Ok(ModuleStatus::Errored(match self.thrown(error) {
                ScriptError::Thrown(ex) => format!(
                    "{}: {}",
                    ex.name.unwrap_or_else(|| "Error".into()),
                    ex.message
                ),
                other => format!("{other:?}"),
            }));
        }
        Ok(if r.status == Status::Evaluated {
            ModuleStatus::Evaluated
        } else {
            ModuleStatus::Evaluating
        })
    }
    pub fn module_binding_string(&mut self, url: &str, name: &str) -> Result<String> {
        let i = self.modules.index(url)?;
        let env = self.modules.records[i]
            .env
            .ok_or(ScriptError::Runtime("module not linked"))?;
        self.frame(|e| {
            let value = e.lookup(env, name)?;
            e.to_string(value)
        })
    }
}
/// `import()` (§13.3.10) and `import.meta` (§13.3.12).
impl Engine<'_> {
    /// GetActiveScriptOrModule (§9.4.1), read from the lexical environment chain: a
    /// module's code always runs under its module environment.
    fn active_module(&self, env: Id) -> Option<usize> {
        let mut current = Some(env);
        while let Some(id) = current {
            if let Some(m) = self.modules.records.iter().position(|r| r.env == Some(id)) {
                return Some(m);
            }
            current = match self.objects.get(&id) {
                Some(Object::Environment { parent, .. }) => *parent,
                _ => None,
            };
        }
        None
    }
    /// EvaluateImportCall (§13.3.10.2) from step 4, with the specifier and options
    /// evaluated (options is undefined without an options expression).
    pub(super) fn import_call(
        &mut self,
        env: Id,
        specifier: Value,
        options: Value,
        phase: ImportPhase,
    ) -> Result<Value> {
        let referrer = self.active_module(env);
        self.frame(|e| {
            // Step 5: NewPromiseCapability(%Promise%).
            let cap = e.new_capability()?;
            // Steps 6-7: IfAbruptRejectPromise(ToString(specifier)).
            let specifier = match e.to_string(specifier) {
                Ok(s) => s,
                Err(ScriptError::Thrown(ex)) => {
                    e.invoke(cap.reject.clone(), vec![ex.value])?;
                    return Ok(cap.promise);
                }
                Err(other) => return Err(other),
            };
            // Steps 6-7: the import attributes.
            let attributes = match e.import_call_attributes(options)? {
                Ok(attributes) => attributes,
                Err(reason) => {
                    e.invoke(cap.reject.clone(), vec![reason])?;
                    return Ok(cap.promise);
                }
            };
            // Step 8: the ModuleRequest Record.
            let key = RequestKey::new(specifier, attributes);
            let mut roots = vec![];
            for v in cap.values() {
                if let Some(id) = v.id() {
                    roots.push(e.runtime.root(id)?);
                }
            }
            let entry = DynamicImport {
                cap: cap.clone(),
                load: None,
                module: None,
                roots,
                phase,
                waiting: 0,
                settled: false,
            };
            let slot = match e.modules.dynamic.iter().position(Option::is_none) {
                Some(i) => {
                    e.modules.dynamic[i] = Some(entry);
                    i
                }
                None => {
                    e.modules.dynamic.push(Some(entry));
                    e.modules.dynamic.len() - 1
                }
            };
            // Step 12: HostLoadImportedModule(referrer, moduleRequest, empty, cap). A
            // request this referrer already loaded is answered from [[LoadedModules]]
            // (the host must return the same module, §16.2.1.10).
            let cached = match referrer {
                Some(m) => e.modules.records[m].loaded.get(&key).copied(),
                None => e.modules.script_loaded.get(&key).copied(),
            };
            match cached {
                Some(dep) => e.continue_dynamic_import(slot, Ok(dep))?,
                None => e.host_load_imported_module(referrer, key, phase, Waiter::Import(slot)),
            }
            Ok(cap.promise)
        })
    }
    /// EvaluateImportCall (§13.3.10.2) step 7: the attributes of `options`, or the
    /// reason the promise rejects with (abrupt completions and TypeErrors).
    fn import_call_attributes(
        &mut self,
        options: Value,
    ) -> Result<std::result::Result<Vec<(String, String)>, Value>> {
        let mut attributes = vec![];
        // Step 7: options is undefined.
        let Value::Object(options) = options else {
            if matches!(options, Value::Undefined) {
                return Ok(Ok(attributes));
            }
            // Step 7.a.
            let reason = self.error_value(
                "TypeError",
                "The second argument of import() must be an object",
            )?;
            return Ok(Err(reason));
        };
        let caught = |r: Result<Value>| match r {
            Ok(v) => Ok(Ok(v)),
            Err(ScriptError::Thrown(ex)) => Ok(Err(ex.value)),
            Err(other) => Err(other),
        };
        // Step 7.b: Get(options, "with").
        let with = match caught(self.get_member(Value::Object(options), "with"))? {
            Ok(v) => v,
            Err(reason) => return Ok(Err(reason)),
        };
        self.protect(&with)?;
        if !matches!(with, Value::Undefined) {
            // Step 7.c.i.
            let Value::Object(with) = with else {
                let reason = self.error_value(
                    "TypeError",
                    "The 'with' option of import() must be an object",
                )?;
                return Ok(Err(reason));
            };
            // Step 7.c.ii: EnumerableOwnProperties(attributesObj, key+value).
            let keys = match self.own_keys(with) {
                Ok(keys) => keys,
                Err(ScriptError::Thrown(ex)) => return Ok(Err(ex.value)),
                Err(other) => return Err(other),
            };
            let mut entries = vec![];
            for key in keys {
                let Key::String(key) = key else {
                    continue;
                };
                let own = match self.own(with, &key) {
                    Ok(own) => own,
                    Err(ScriptError::Thrown(ex)) => return Ok(Err(ex.value)),
                    Err(other) => return Err(other),
                };
                if !own.is_some_and(|p| p.enumerable) {
                    continue;
                }
                let value = match caught(self.get_member(Value::Object(with), &key))? {
                    Ok(v) => v,
                    Err(reason) => return Ok(Err(reason)),
                };
                self.protect(&value)?;
                entries.push((key, value));
            }
            // Step 7.c.iii: every value is a String.
            for (key, value) in entries {
                let Value::String(value) = value else {
                    let reason =
                        self.error_value("TypeError", "Import attribute value must be a string")?;
                    return Ok(Err(reason));
                };
                attributes.push((key.to_string(), value.to_string()));
            }
        }
        // Step 7.d: AllImportAttributesSupported.
        let probe = RequestKey::new("", attributes);
        if let Some(attribute) = unsupported_attribute(&probe) {
            let message = format!("Import attribute \"{attribute}\" is not supported");
            let reason = self.error_value("TypeError", &message)?;
            return Ok(Err(reason));
        }
        Ok(Ok(probe.attributes))
    }
    /// The value a failed load rejects with.
    fn failure_value(&mut self, failure: &Failure) -> Result<Value> {
        if let Some(err) = failure.unsupported() {
            return Err(err);
        }
        match failure {
            Failure::Syntax(s) => self.error_value("SyntaxError", &s.message),
            Failure::Error(kind, message) => self.error_value(kind, message),
        }
    }
    /// ContinueDynamicImport (§13.3.10.2) steps 1-2 and 7: reject, or start
    /// LoadRequestedModules and react to its promise.
    fn continue_dynamic_import(
        &mut self,
        slot: usize,
        result: std::result::Result<usize, Failure>,
    ) -> Result<()> {
        let module = match result {
            Ok(m) => m,
            Err(failure) => {
                let reason = self.failure_value(&failure)?;
                return self.finish_import(slot, Err(reason));
            }
        };
        // Source phase: resolve with GetModuleSource(module), never link or evaluate.
        if self.modules.dynamic[slot].as_ref().map(|d| d.phase) == Some(ImportPhase::Source) {
            let result = match self.binding_value(module, &Binding::Source, "") {
                Ok(v) => Ok(v),
                Err(ScriptError::Thrown(ex)) => Err(ex.value),
                Err(other) => return Err(other),
            };
            return self.finish_import(slot, result);
        }
        let load = self.new_capability()?;
        let mut roots = vec![];
        for v in load.values() {
            if let Some(id) = v.id() {
                roots.push(self.runtime.root(id)?);
            }
        }
        let loaded = self.internal_function(
            Internal::DynamicImport {
                slot,
                step: DynamicStep::Loaded,
            },
            0,
        )?;
        let rejected = self.internal_function(
            Internal::DynamicImport {
                slot,
                step: DynamicStep::Rejected,
            },
            1,
        )?;
        let Value::Object(p) = load.promise.clone() else {
            return Err(ScriptError::Runtime("dynamic import without promise"));
        };
        self.attach_reaction(p, Some(loaded), Some(rejected), None)?;
        let entry = self.modules.dynamic[slot]
            .as_mut()
            .ok_or(ScriptError::Runtime("settled dynamic import"))?;
        entry.load = Some(load);
        entry.module = Some(module);
        entry.roots.extend(roots);
        let g = self.modules.graphs.len();
        self.modules.graphs.push(Graph {
            root: module,
            visited: HashSet::new(),
            pending: 1,
            loading: true,
            import: Some(slot),
        });
        self.inner_module_loading(g, module)
    }
    /// Settles the GraphLoadingState promise of an `import()` graph.
    fn settle_load(&mut self, slot: usize, result: std::result::Result<(), Value>) -> Result<()> {
        let Some(load) = self.modules.dynamic[slot]
            .as_ref()
            .and_then(|d| d.load.clone())
        else {
            return Err(ScriptError::Runtime("settled dynamic import"));
        };
        match result {
            Ok(()) => self.invoke(load.resolve, vec![Value::Undefined])?,
            Err(reason) => self.invoke(load.reject, vec![reason])?,
        };
        Ok(())
    }
    /// Resolves or rejects the `import()` promise and drops its roots.
    fn finish_import(
        &mut self,
        slot: usize,
        result: std::result::Result<Value, Value>,
    ) -> Result<()> {
        let Some(entry) = self.modules.dynamic.get_mut(slot).and_then(Option::take) else {
            return Err(ScriptError::Runtime("settled dynamic import"));
        };
        let outcome = match result {
            Ok(v) => self.invoke(entry.cap.resolve.clone(), vec![v]),
            Err(v) => self.invoke(entry.cap.reject.clone(), vec![v]),
        };
        for root in entry.roots {
            self.runtime.unroot(root)?;
        }
        outcome.map(|_| ())
    }
    /// The reaction closures of ContinueDynamicImport (§13.3.10.2 steps 3 and 6).
    pub(super) fn dynamic_import_step(
        &mut self,
        slot: usize,
        step: DynamicStep,
        arg: Value,
    ) -> Result<Value> {
        let Some((module, phase)) = self
            .modules
            .dynamic
            .get(slot)
            .and_then(|d| d.as_ref())
            .and_then(|d| Some((d.module?, d.phase)))
        else {
            return Err(ScriptError::Runtime("invalid dynamic import continuation"));
        };
        match step {
            DynamicStep::Rejected => self.finish_import(slot, Err(arg))?,
            DynamicStep::DeferFulfilled | DynamicStep::DeferRejected => {
                // The SafePerformPromiseAll of ContinueDynamicImport (defer proposal):
                // the first rejection rejects; the import promise resolves with the
                // deferred namespace once every evaluation fulfilled. The entry stays
                // until the last reaction so the slot is not reused under it.
                let entry = self.modules.dynamic[slot].as_mut().unwrap();
                entry.waiting -= 1;
                let (waiting, settled) = (entry.waiting, entry.settled);
                if settled {
                    if waiting == 0 {
                        self.finish_import(slot, Ok(Value::Undefined))?;
                    }
                } else if step == DynamicStep::DeferRejected {
                    if waiting == 0 {
                        self.finish_import(slot, Err(arg))?;
                    } else {
                        let entry = self.modules.dynamic[slot].as_mut().unwrap();
                        entry.settled = true;
                        let reject = entry.cap.reject.clone();
                        self.invoke(reject, vec![arg])?;
                    }
                } else if waiting == 0 {
                    let namespace = self.module_namespace(module, true)?;
                    self.finish_import(slot, Ok(namespace))?;
                }
            }
            DynamicStep::Loaded => {
                // Steps 6.a-b: Link(); an abrupt completion rejects.
                match self.link(module) {
                    Ok(()) => {}
                    Err(ScriptError::Thrown(ex)) => {
                        self.finish_import(slot, Err(ex.value))?;
                        return Ok(Value::Undefined);
                    }
                    Err(other) => return Err(other),
                }
                if phase == ImportPhase::Defer {
                    return self.defer_import_evaluate(slot, module);
                }
                // Steps 6.c-f: Evaluate(), then react to its promise.
                let Value::Object(p) = self.evaluate(module)? else {
                    return Err(ScriptError::Runtime("module evaluation without promise"));
                };
                let fulfilled = self.internal_function(
                    Internal::DynamicImport {
                        slot,
                        step: DynamicStep::Evaluated,
                    },
                    0,
                )?;
                let rejected = self.internal_function(
                    Internal::DynamicImport {
                        slot,
                        step: DynamicStep::Rejected,
                    },
                    1,
                )?;
                self.attach_reaction(p, Some(fulfilled), Some(rejected), None)?;
                self.unhandled.remove(&p);
            }
            DynamicStep::Evaluated => {
                // Step 6.e.i: GetModuleNamespace(module).
                let namespace = self.module_namespace(module, false)?;
                self.finish_import(slot, Ok(namespace))?;
            }
        }
        Ok(Value::Undefined)
    }
    /// ContinueDynamicImport for `import.defer()` after Link() (IMPORT_PHASES.md):
    /// evaluate only GatherAsynchronousTransitiveDependencies(module), wait on all
    /// of them, then resolve with GetModuleNamespace(module, defer).
    fn defer_import_evaluate(&mut self, slot: usize, module: usize) -> Result<Value> {
        let mut deps = vec![];
        self.gather_async_dependencies(module, &mut vec![], &mut deps);
        if deps.is_empty() {
            let namespace = self.module_namespace(module, true)?;
            self.finish_import(slot, Ok(namespace))?;
            return Ok(Value::Undefined);
        }
        self.modules.dynamic[slot].as_mut().unwrap().waiting = deps.len();
        for dep in deps {
            let Value::Object(p) = self.evaluate(dep)? else {
                return Err(ScriptError::Runtime("module evaluation without promise"));
            };
            let fulfilled = self.internal_function(
                Internal::DynamicImport {
                    slot,
                    step: DynamicStep::DeferFulfilled,
                },
                0,
            )?;
            let rejected = self.internal_function(
                Internal::DynamicImport {
                    slot,
                    step: DynamicStep::DeferRejected,
                },
                1,
            )?;
            self.attach_reaction(p, Some(fulfilled), Some(rejected), None)?;
            self.unhandled.remove(&p);
        }
        Ok(Value::Undefined)
    }
    /// %AbstractModuleSource% and its prototype (source phase imports §28.3), per
    /// realm. Not a global property; the test host exposes it on `$262`.
    pub(super) fn bootstrap_module_source(&mut self, env: Id) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let (object, function) = (i.object, i.function);
        let base = self.intrinsic_function(env, "%AbstractModuleSource%", Some(function))?;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        let intrinsics = self.intrinsics.as_mut().unwrap();
        intrinsics
            .functions
            .insert("%AbstractModuleSource%".into(), base);
        intrinsics
            .collection
            .insert("AbstractModuleSource".into(), proto);
        // §28.3.1-2: name "AbstractModuleSource", length 0, a non-writable,
        // non-enumerable, non-configurable `prototype`.
        for (key, value) in [
            ("name", Value::String("AbstractModuleSource".into())),
            ("length", Value::number(0.0)),
            ("prototype", Value::Object(proto)),
        ] {
            self.define(
                base,
                key,
                Descriptor {
                    value: Some(value),
                    ..Default::default()
                },
            )?;
        }
        self.hidden(proto, "constructor", Value::Object(base))?;
        // §28.3.3.2 get %AbstractModuleSource%.prototype [ @@toStringTag ].
        let getter =
            self.intrinsic_function(env, "%AbstractModuleSource%.prototype.tag", Some(function))?;
        for (key, value) in [
            ("name", Value::String("get [Symbol.toStringTag]".into())),
            ("length", Value::number(0.0)),
        ] {
            self.define(
                getter,
                key,
                Descriptor {
                    value: Some(value),
                    ..Default::default()
                },
            )?;
        }
        self.define(
            proto,
            Key::Symbol(1),
            Descriptor {
                get: Some(Value::Object(getter)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    /// Calls of %AbstractModuleSource% (§28.3.1.1: always a TypeError) and of its
    /// @@toStringTag getter (§28.3.3.2: the [[ModuleSourceClassName]] of `this`, or
    /// undefined when `this` has no such slot).
    pub(super) fn module_source_builtin(&mut self, name: &str, receiver: Value) -> Result<Value> {
        if name == "%AbstractModuleSource%" {
            return self.throw_error("TypeError", "AbstractModuleSource is an abstract class");
        }
        let Value::Object(id) = receiver else {
            return Ok(Value::Undefined);
        };
        Ok(self
            .modules
            .records
            .iter()
            .find_map(|r| r.source.as_ref().filter(|(s, _)| *s == id))
            .map_or(Value::Undefined, |(_, class)| {
                Value::String(class.as_str().into())
            }))
    }
    /// `import.meta` (§13.3.12.1): the module's [[ImportMeta]], a null-prototype
    /// object; HostGetImportMetaProperties adds nothing yet.
    pub(super) fn import_meta(&mut self, env: Id) -> Result<Value> {
        let m = self
            .active_module(env)
            .ok_or(ScriptError::Runtime("import.meta outside a module"))?;
        if let Some(id) = self.modules.records[m].meta {
            return Ok(Value::Object(id));
        }
        let menv = self.modules.records[m]
            .env
            .ok_or(ScriptError::Runtime("module environment missing"))?;
        self.frame(|e| {
            let id = e.new_object(None)?;
            e.protect(&Value::Object(id))?;
            e.initialize(menv, "\0importMeta", Value::Object(id))?;
            e.modules.records[m].meta = Some(id);
            Ok(Value::Object(id))
        })
    }
}
