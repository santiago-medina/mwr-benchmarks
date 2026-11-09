//! Cyclic Module Records (ECMA-262 §16.2.1.6) and Source Text Module Records
//! (§16.2.1.7) over traced lexical environments. Imports stay aliases
//! `(target env, name)`; MODULE_RECORDS.md describes the design and the pull
//! host API (`take_module_requests` / `finish_module_request`).
use super::{
    objects::{Descriptor, Key, Kind, Property},
    parser::{self, ExportImport, ImportName, Module, SyntaxError},
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
    /// [[LoadedModules]]: specifier → record.
    loaded: HashMap<String, usize>,
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
    specifier: String,
    graphs: Vec<usize>,
    imports: Vec<usize>,
}
/// Who waits on a HostLoadImportedModule request: a loading graph or an `import()`.
enum Waiter {
    Graph(usize),
    Import(usize),
}
/// A pending `import()` (§13.3.10.2 ContinueDynamicImport): its capability, the
/// loading promise and the module, held by external roots until it settles.
struct DynamicImport {
    cap: Capability,
    load: Option<Capability>,
    module: Option<usize>,
    roots: Vec<ExternalRoot>,
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
}
/// A failed load, kept as data so several waiting graphs can each throw it.
#[derive(Clone)]
enum Failure {
    Syntax(SyntaxError),
    Error(String, String),
}
enum Resolution {
    Null,
    Ambiguous,
    /// ResolvedBinding { [[Module]], [[BindingName]] }; `None` is ~namespace~.
    Binding(usize, Option<String>),
}
#[derive(Default)]
pub(super) struct Registry {
    records: Vec<Record>,
    /// The host's module map key: (url, module type).
    keys: HashMap<(String, String), usize>,
    graphs: Vec<Graph>,
    pending: HashMap<u64, Pending>,
    inflight: HashMap<(Option<usize>, String), u64>,
    /// [[LoadedModules]] of Script and Realm referrers (one host-wide map).
    script_loaded: HashMap<String, usize>,
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
    fn dependency(&self, m: usize, specifier: &str) -> Result<usize> {
        self.records[m]
            .loaded
            .get(specifier)
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
                return Resolution::Binding(m, e.local.clone());
            }
        }
        for e in &syntax.indirect_exports {
            if e.export_name.as_deref() == Some(name) {
                let Ok(dep) = self.dependency(m, e.request.as_deref().unwrap_or_default()) else {
                    return Resolution::Null;
                };
                return match &e.import_name {
                    Some(ExportImport::All) => Resolution::Binding(dep, None),
                    Some(ExportImport::Name(n)) => self.resolve_export(dep, n, set),
                    _ => Resolution::Null,
                };
            }
        }
        if name == "default" {
            return Resolution::Null;
        }
        let mut star: Option<(usize, Option<String>)> = None;
        for e in &syntax.star_exports {
            let Ok(dep) = self.dependency(m, e.request.as_deref().unwrap_or_default()) else {
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
            let Ok(dep) = self.dependency(m, e.request.as_deref().unwrap_or_default()) else {
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
        self.modules
            .inflight
            .remove(&(p.referrer, p.specifier.clone()));
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
            ModuleLoad::Error { kind, message } => Err(Failure::Error(kind, message)),
        };
        if let Ok(dep) = result {
            let loaded = match p.referrer {
                Some(r) => &mut self.modules.records[r].loaded,
                None => &mut self.modules.script_loaded,
            };
            match loaded.get(&p.specifier) {
                Some(&prev) if prev != dep => {
                    return Err(ScriptError::Host(
                        "host loaded two modules for one request".into(),
                    ))
                }
                _ => {
                    loaded.insert(p.specifier.clone(), dep);
                }
            }
        }
        let mut first = None;
        for g in p.graphs {
            if let Err(err) = self.frame(|e| e.continue_module_loading(g, result.clone())) {
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
        if module_type != "javascript" {
            return Err(ScriptError::Unsupported("unsupported module type"));
        }
        if self.modules.records.len() >= MODULE_LIMIT {
            return Err(ScriptError::Runtime("module graph limit"));
        }
        self.modules.bytes += source.len();
        if self.modules.bytes > SOURCE_LIMIT {
            return Err(ScriptError::Runtime("module source limit"));
        }
        let syntax = parser::parse_module(source).map_err(ScriptError::Syntax)?;
        let has_tla = syntax.body.iter().any(super::asynchronous::stmt_await);
        let i = self.modules.records.len();
        self.modules.keys.insert(key, i);
        self.modules.records.push(Record {
            url: url.into(),
            host_defined,
            syntax,
            loaded: HashMap::new(),
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
            meta: None,
        });
        Ok(i)
    }
    /// InnerModuleLoading (§16.2.1.6.1.1).
    fn inner_module_loading(&mut self, g: usize, m: usize) -> Result<()> {
        if self.modules.records[m].status == Status::New && self.modules.graphs[g].visited.insert(m)
        {
            let requests = self.modules.records[m].syntax.requests.clone();
            self.modules.graphs[g].pending += requests.len();
            for specifier in requests {
                if let Some(&dep) = self.modules.records[m].loaded.get(&specifier) {
                    self.inner_module_loading(g, dep)?;
                } else {
                    self.host_load_imported_module(Some(m), specifier, Waiter::Graph(g));
                }
                if !self.modules.graphs[g].loading {
                    return Ok(());
                }
            }
        }
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
    fn host_load_imported_module(&mut self, m: Option<usize>, specifier: String, w: Waiter) {
        let r = &mut self.modules;
        let id = match r.inflight.get(&(m, specifier.clone())) {
            Some(id) => *id,
            None => {
                let id = r.next_request;
                r.next_request += 1;
                r.inflight.insert((m, specifier.clone()), id);
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
                    specifier: specifier.clone(),
                    attributes: vec![],
                });
                r.pending.insert(
                    id,
                    Pending {
                        referrer: m,
                        specifier,
                        graphs: vec![],
                        imports: vec![],
                    },
                );
                id
            }
        };
        let p = r.pending.get_mut(&id).unwrap();
        match w {
            Waiter::Graph(g) => p.graphs.push(g),
            Waiter::Import(slot) => p.imports.push(slot),
        }
    }
    /// ContinueModuleLoading (§16.2.1.6.1.2).
    fn continue_module_loading(
        &mut self,
        g: usize,
        result: std::result::Result<usize, Failure>,
    ) -> Result<()> {
        if !self.modules.graphs[g].loading {
            return Ok(());
        }
        match result {
            Ok(m) => self.inner_module_loading(g, m),
            Err(failure) => {
                self.modules.graphs[g].loading = false;
                match self.modules.graphs[g].import {
                    // An `import()` graph rejects its loading promise instead.
                    Some(slot) => {
                        let reason = self.failure_value(&failure)?;
                        self.settle_load(slot, Err(reason))
                    }
                    None => Err(failure.script_error(self)),
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
            let mut data = self.data(p)?.clone();
            if let Some(record) = data.promise.as_mut() {
                record.handled = true;
            }
            self.replace_data(p, data)?;
            self.unhandled.remove(&p);
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
        for specifier in self.modules.records[m].syntax.requests.clone() {
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
        self.hoist_functions(&body, env)
    }
    /// InitializeEnvironment (§16.2.1.7.4), import bindings part; the lexical and
    /// function declarations were instantiated by `create_module_environment`.
    fn initialize_environment(&mut self, m: usize) -> Result<()> {
        let syntax = &self.modules.records[m].syntax;
        let indirect: Vec<_> = syntax
            .indirect_exports
            .iter()
            .map(|e| (e.request.clone().unwrap_or_default(), e.export_name.clone()))
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
                ImportName::Namespace => (dep, None),
                ImportName::Name(n) => match self.modules.resolve_export(dep, n, &mut vec![]) {
                    Resolution::Binding(t, b) => (t, b),
                    Resolution::Null => return self.missing_export(&import.request, n),
                    Resolution::Ambiguous => return self.ambiguous_export(&import.request, n),
                },
            };
            match binding {
                None => {
                    let namespace = self.module_namespace(target)?;
                    self.initialize(env, &import.local, namespace)?;
                }
                Some(name) => {
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
    /// GetModuleNamespace (§16.2.1.12) and ModuleNamespaceCreate (§10.4.6.12).
    fn module_namespace(&mut self, m: usize) -> Result<Value> {
        if let Some((id, _)) = &self.modules.records[m].namespace {
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
        self.frame(|e| {
            let id = e.new_object(None)?;
            e.protect(&Value::Object(id))?;
            // §28.3.1 @@toStringTag: "Module", non-writable, non-enumerable, non-configurable.
            e.define(
                id,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String("Module".into())),
                    ..Default::default()
                },
            )?;
            let mut data = e.data(id)?.clone();
            data.extensible = false;
            data.namespace = Some(m);
            e.replace_data(id, data)?;
            e.runtime.link(id, env)?;
            e.initialize(env, "\0namespace", Value::Object(id))?;
            e.modules.records[m].namespace = Some((id, names));
            Ok(Value::Object(id))
        })
    }
    pub(super) fn namespace_key(&self, id: Id, key: &Key) -> Option<(usize, String)> {
        match key {
            Key::String(s) => Some((self.data(id).ok()?.namespace?, s.clone())),
            Key::Symbol(_) => None,
        }
    }
    pub(super) fn namespace_has(&self, m: usize, name: &str) -> bool {
        self.modules
            .records
            .get(m)
            .and_then(|r| r.namespace.as_ref())
            .is_some_and(|(_, names)| names.iter().any(|n| n == name))
    }
    /// Module namespace [[Get]] (§10.4.6.8) for a string key.
    pub(super) fn namespace_get(&mut self, m: usize, name: &str) -> Result<Value> {
        if !self.namespace_has(m, name) {
            return Ok(Value::Undefined);
        }
        match self.modules.resolve_export(m, name, &mut vec![]) {
            Resolution::Binding(t, None) => self.module_namespace(t),
            Resolution::Binding(t, Some(binding)) => {
                let Some(env) = self.modules.records[t].env else {
                    return self.throw_error("ReferenceError", &format!("{name} is not defined"));
                };
                self.lookup(env, &binding)
            }
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
        let mut keys: Vec<Key> = self
            .modules
            .records
            .get(m)
            .and_then(|r| r.namespace.as_ref())
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
        for specifier in self.modules.records[m].syntax.requests.clone() {
            let mut dep = self.modules.dependency(m, &specifier)?;
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
    /// EvaluateImportCall (§13.3.10.1) from step 3, with the specifier evaluated.
    pub(super) fn import_call(&mut self, env: Id, specifier: Value) -> Result<Value> {
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
                Some(m) => e.modules.records[m].loaded.get(&specifier).copied(),
                None => e.modules.script_loaded.get(&specifier).copied(),
            };
            match cached {
                Some(dep) => e.continue_dynamic_import(slot, Ok(dep))?,
                None => e.host_load_imported_module(referrer, specifier, Waiter::Import(slot)),
            }
            Ok(cap.promise)
        })
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
        let Some(module) = self
            .modules
            .dynamic
            .get(slot)
            .and_then(|d| d.as_ref())
            .and_then(|d| d.module)
        else {
            return Err(ScriptError::Runtime("invalid dynamic import continuation"));
        };
        match step {
            DynamicStep::Rejected => self.finish_import(slot, Err(arg))?,
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
                let namespace = self.module_namespace(module)?;
                self.finish_import(slot, Ok(namespace))?;
            }
        }
        Ok(Value::Undefined)
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
