//! Tree-walking interpreter. Environments and closures are JS arena objects;
//! metadata alone is never a GC root. Temporary values are rooted by frames.
use super::parser::{self, Stmt, SyntaxError};
use super::{builtins::Intrinsics, objects::Data};
use crate::{
    dual::{Domain, ExternalRoot, Id, Runtime},
    Error, Model,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    Syntax(SyntaxError),
    Runtime(&'static str),
    Memory(Error),
    Host(String),
    Unsupported(&'static str),
    LoopBreak,
    LoopContinue,
    Thrown(Exception),
}
impl ScriptError {
    pub(super) fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Memory(_)
                | Self::Runtime(_)
                | Self::Host(_)
                | Self::Unsupported(_)
                | Self::Syntax(_)
        )
    }
}
impl From<Error> for ScriptError {
    fn from(e: Error) -> Self {
        Self::Memory(e)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exception {
    pub name: Option<String>,
    pub message: String,
    pub(super) value: Value,
}
type Result<T> = std::result::Result<T, ScriptError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    Symbol(u64),
    String(String),
    Object(Id),
    Document,
    Native { receiver: Option<Id>, name: String },
}
impl Value {
    pub(super) fn id(&self) -> Option<Id> {
        match self {
            Self::Object(id)
            | Self::Native {
                receiver: Some(id), ..
            } => Some(*id),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FunctionKind {
    Ordinary,
    Arrow,
    Method,
    Async,
    AsyncArrow,
    AsyncMethod,
}
#[derive(Clone)]
pub(super) enum Object {
    Ordinary(Data),
    Builtin {
        name: String,
        data: Data,
    },
    Bound {
        target: Value,
        receiver: Value,
        args: Vec<Value>,
        data: Data,
    },
    Node {
        tag: String,
        text: String,
        listeners: Vec<Id>,
    },
    Environment {
        parent: Option<Id>,
        bindings: HashMap<String, Option<Value>>,
        mutable: HashSet<String>,
        function_scope: bool,
        object_bindings: HashSet<String>,
        aliases: HashMap<String, (Id, String)>,
    },
    Closure {
        kind: FunctionKind,
        program: Option<std::sync::Arc<super::asynchronous::Program>>,
        data: Data,
        environment: Id,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
}

pub struct Engine<'host> {
    pub(super) runtime: Runtime,
    pub(super) objects: HashMap<Id, Object>,
    roots: Vec<ExternalRoot>,
    pub(super) modules: super::modules::Registry,
    pub(super) network: super::network::Network,
    pub(super) global: Option<ExternalRoot>,
    pub(super) intrinsics: Option<Intrinsics>,
    pub(super) test_harness: bool,
    pub(super) assertions: usize,
    pub(super) evaluation_depth: usize,
    pub(super) next_symbol: u64,
    pub(super) join_stack: Vec<Id>,
    pub(super) jobs: std::collections::VecDeque<super::promises::QueuedJob>,
    pub(super) next_job: u64,
    pub(super) checkpoint_running: bool,
    pub(super) microtask_queue_limit: usize,
    pub(super) microtask_errors: Vec<String>,
    pub(super) unhandled: HashMap<Id, (bool, String)>,
    pub(super) done_count: usize,
    pub(super) done_error: Option<String>,
    stress: bool,
    steps: usize,
    step_limit: usize,
    pub(super) call_depth: usize,
    pub(super) native_depth: usize,
    pub(super) adapter: Option<Box<dyn super::adapter::DomAdapter + 'host>>,
    pub(super) remote_nodes: HashMap<Id, u64>,
    pub(super) adapter_error: Option<String>,
    pub(super) pending_release: Vec<u64>,
}

impl<'host> Engine<'host> {
    pub fn new(dom: Model, js: Model) -> Result<Self> {
        Ok(Self {
            runtime: Runtime::new(dom, js)?,
            objects: HashMap::new(),
            roots: vec![],
            network: super::network::Network::default(),
            modules: super::modules::Registry::default(),
            global: None,
            intrinsics: None,
            test_harness: false,
            assertions: 0,
            evaluation_depth: 0,
            next_symbol: 3,
            join_stack: vec![],
            jobs: super::promises::empty_queue(),
            next_job: 1,
            checkpoint_running: false,
            microtask_queue_limit: 10_000,
            microtask_errors: vec![],
            unhandled: HashMap::new(),
            done_count: 0,
            done_error: None,
            stress: true,
            steps: 0,
            step_limit: 10_000,
            call_depth: 0,
            native_depth: 0,
            adapter: None,
            remote_nodes: HashMap::new(),
            adapter_error: None,
            pending_release: vec![],
        })
    }
    // Move all VM state while changing only the lifetime of the borrowed host.
    fn rebind<'next>(
        self,
        adapter: Option<Box<dyn super::adapter::DomAdapter + 'next>>,
    ) -> Engine<'next> {
        let mut engine = Engine {
            runtime: self.runtime,
            objects: self.objects,
            roots: self.roots,
            network: self.network,
            modules: self.modules,
            global: self.global,
            intrinsics: self.intrinsics,
            test_harness: self.test_harness,
            assertions: self.assertions,
            evaluation_depth: self.evaluation_depth,
            next_symbol: self.next_symbol,
            join_stack: self.join_stack,
            jobs: self.jobs,
            next_job: self.next_job,
            checkpoint_running: self.checkpoint_running,
            microtask_queue_limit: self.microtask_queue_limit,
            microtask_errors: self.microtask_errors,
            unhandled: self.unhandled,
            done_count: self.done_count,
            done_error: self.done_error,
            stress: self.stress,
            steps: self.steps,
            step_limit: self.step_limit,
            call_depth: self.call_depth,
            native_depth: self.native_depth,
            remote_nodes: self.remote_nodes,
            adapter_error: self.adapter_error,
            pending_release: self.pending_release,
            adapter,
        };
        engine.flush_network();
        engine
    }
    /// End a host invocation without releasing the persistent VM or DOM leases.
    /// The embedder must keep the same host registry alive until it resumes or
    /// discards this state. No script can execute while the VM is suspended.
    pub fn suspend(self) -> SuspendedEngine {
        SuspendedEngine(self.rebind(None))
    }
    pub fn enable_test_harness(&mut self) {
        self.test_harness = true;
    }
    pub fn assertion_count(&self) -> usize {
        self.assertions
    }
    pub fn set_stress(&mut self, enabled: bool) {
        self.stress = enabled;
    }
    pub fn set_budget(&mut self, bytes: Option<usize>) {
        self.runtime.set_budget(bytes);
    }
    pub(super) fn steps_for_external_event(&mut self) {
        self.steps = 0;
    }
    pub fn set_step_limit(&mut self, steps: usize) {
        self.step_limit = steps;
    }
    pub fn live_objects(&self) -> usize {
        self.runtime.live_objects()
    }
    pub fn live_roots(&self) -> usize {
        self.runtime.live_roots()
    }
    pub fn metadata_objects(&self) -> usize {
        self.objects.len()
    }
    pub fn charged_bytes(&self) -> usize {
        self.runtime.charged_bytes()
    }
    pub fn collect(&mut self) {
        self.runtime.collect();
        self.prune_metadata();
    }
    pub(super) fn prune_metadata(&mut self) {
        let dead: Vec<_> = self
            .remote_nodes
            .iter()
            .filter(|(id, _)| self.runtime.value(**id).is_err())
            .map(|(_, remote)| *remote)
            .collect();
        self.remote_nodes
            .retain(|id, _| self.runtime.value(*id).is_ok());
        self.objects.retain(|id, _| self.runtime.value(*id).is_ok());
        if self
            .intrinsics
            .as_ref()
            .is_some_and(|i| self.runtime.value(i.object).is_err())
        {
            self.intrinsics = None;
        }
        self.pending_release.extend(dead);
        self.prune_responses();
        self.flush_remote_releases();
    }
    pub(super) fn queue_remote_release(&mut self, remote: u64) {
        self.pending_release.push(remote);
        self.flush_remote_releases();
    }
    fn flush_remote_releases(&mut self) {
        if self.pending_release.is_empty() {
            return;
        }
        if let Some(adapter) = self.adapter.as_mut() {
            match adapter.release(&self.pending_release) {
                Ok(()) => {
                    self.pending_release.clear();
                    self.adapter_error = None;
                }
                Err(error) => self.adapter_error = Some(error),
            }
        }
    }
    pub fn host_error(&self) -> Option<&str> {
        self.adapter_error.as_deref()
    }
    pub fn set_dom_adapter(
        &mut self,
        adapter: Box<dyn super::adapter::DomAdapter + 'host>,
    ) -> Result<()> {
        if self.runtime.live_objects() != 0 || !self.pending_release.is_empty() {
            return Err(ScriptError::Runtime("attach adapter before executing"));
        }
        self.adapter = Some(adapter);
        self.adapter_error = None;
        Ok(())
    }
    pub fn release_globals(&mut self) {
        self.cancel_microtasks();
        self.modules = super::modules::Registry::default();
        self.unhandled.clear();
        if let Some(global) = self.global.take() {
            self.runtime.unroot(global).expect("owned global root");
        }
        self.collect();
    }
    /// Invoke an existing global function without replacing its realm. A failed
    /// call unwinds its frames, preserving listeners already installed in the DOM.
    pub fn call_global(&mut self, name: &str) -> Result<()> {
        self.steps = 0;
        let callee = self.binding(name)?;
        let result = self.frame(|e| {
            e.protect(&callee)?;
            e.invoke(callee, vec![]).map(|_| ())
        });
        if result.as_ref().is_err_and(|e| e.is_terminal()) {
            self.cancel_microtasks();
        }
        self.collect();
        result
    }
    /// Complete a native stop-the-world mark. Conservative listener pins must
    /// not participate in the initial mark, or cross-heap cycles never die.
    pub fn collect_native(&mut self, marked_remotes: &[u64]) -> Result<()> {
        let seeds: Vec<_> = self
            .remote_nodes
            .iter()
            .filter(|(_, remote)| marked_remotes.contains(remote))
            .map(|(id, _)| *id)
            .collect();
        self.runtime.collect_joint(&seeds)?;
        self.prune_metadata();
        Ok(())
    }
    pub fn binding_string(&mut self, name: &str) -> Result<String> {
        match self.binding(name)? {
            Value::String(value) => Ok(value),
            _ => Err(ScriptError::Runtime("not a string")),
        }
    }
    pub(super) fn binding(&mut self, name: &str) -> Result<Value> {
        self.frame(|e| {
            let global = e
                .global
                .ok_or(ScriptError::Runtime("no global environment"))?;
            let id = e.runtime.resolve(global)?;
            e.lookup(id, name)
        })
    }
    pub fn execute(&mut self, source: &str) -> Result<()> {
        let program = parser::parse(source).map_err(ScriptError::Syntax)?;
        self.release_globals();
        self.steps = 0;
        self.assertions = 0;
        self.done_count = 0;
        self.done_error = None;
        self.microtask_errors.clear();
        let global = self.new_environment(None, &program, true)?;
        self.global = Some(self.runtime.root(global)?);
        let result = self.frame(|e| {
            e.bootstrap(global)?;
            e.block(&program, global, false).map(|_| ())
        });
        if result.is_err() {
            self.release_globals();
        } else {
            self.collect();
        }
        result
    }
    pub(super) fn tick(&mut self) -> Result<()> {
        if self.steps >= self.step_limit {
            return Err(ScriptError::Runtime("execution step limit"));
        }
        self.steps += 1;
        Ok(())
    }
    pub(super) fn allocate(&mut self, domain: Domain, tag: i64) -> Result<Id> {
        if self.stress {
            self.collect();
        }
        if let Some(error) = self.adapter_error.clone() {
            return Err(ScriptError::Host(error));
        }
        let result = self.runtime.allocate(domain, tag, 64);
        // The budget can trigger backend collection even with stress disabled.
        self.prune_metadata();
        Ok(result?)
    }
    pub(super) fn protect(&mut self, value: &Value) -> Result<()> {
        if let Some(id) = value.id() {
            self.roots.push(self.runtime.root(id)?);
        }
        Ok(())
    }
    pub(super) fn frame<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let mark = self.roots.len();
        let mut frame = Frame { engine: self, mark };
        f(&mut frame.engine)
    }
    pub(super) fn new_environment(
        &mut self,
        parent: Option<Id>,
        body: &[Stmt],
        function_scope: bool,
    ) -> Result<Id> {
        let mut bindings = HashMap::new();
        let mut mutable = HashSet::new();
        for stmt in body {
            match stmt {
                Stmt::Const(n, _) | Stmt::Let(n, _) => {
                    bindings.insert(n.clone(), None);
                    if matches!(stmt, Stmt::Let(..)) {
                        mutable.insert(n.clone());
                    }
                }
                Stmt::Function(n, ..) | Stmt::AsyncFunction(n, ..) => {
                    bindings.insert(n.clone(), Some(Value::Undefined));
                    mutable.insert(n.clone());
                }
                _ => {}
            }
        }
        fn vars(body: &[Stmt], out: &mut Vec<String>) {
            for s in body {
                match s {
                    Stmt::Var(n, _) => out.push(n.clone()),
                    Stmt::ForOf {
                        kind, name, body, ..
                    } => {
                        if kind == "var" {
                            out.push(name.clone());
                        }
                        vars(std::slice::from_ref(body), out);
                    }
                    Stmt::Block(b) => vars(b, out),
                    Stmt::If(_, a, b) => {
                        vars(std::slice::from_ref(a), out);
                        if let Some(b) = b {
                            vars(std::slice::from_ref(b), out);
                        }
                    }
                    Stmt::Try {
                        body,
                        catch,
                        finally,
                    } => {
                        vars(body, out);
                        if let Some((_, b)) = catch {
                            vars(b, out);
                        }
                        if let Some(b) = finally {
                            vars(b, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        if function_scope {
            let mut names = vec![];
            vars(body, &mut names);
            for n in names {
                if bindings.get(&n) == Some(&None) {
                    return Err(ScriptError::Runtime("var conflicts with lexical binding"));
                }
                bindings.entry(n.clone()).or_insert(Some(Value::Undefined));
                mutable.insert(n);
            }
        }
        let mut object_bindings = HashSet::new();
        if parent.is_none() {
            let mut names = vec![];
            vars(body, &mut names);
            object_bindings.extend(names);
            for s in body {
                if let Stmt::Function(n, ..) | Stmt::AsyncFunction(n, ..) = s {
                    object_bindings.insert(n.clone());
                }
            }
            bindings
                .entry("document".into())
                .or_insert(Some(Value::Document));
        }
        let id = self.allocate(Domain::Js, -300)?;
        if let Some(p) = parent {
            self.runtime.link(id, p)?;
        }
        self.objects.insert(
            id,
            Object::Environment {
                parent,
                bindings,
                mutable,
                function_scope,
                object_bindings,
                aliases: HashMap::new(),
            },
        );
        Ok(id)
    }
    pub(super) fn lookup(&mut self, mut env: Id, name: &str) -> Result<Value> {
        loop {
            if let Some(Object::Environment { aliases, .. }) = self.objects.get(&env) {
                if let Some((target, name)) = aliases.get(name).cloned() {
                    return self.lookup(target, &name);
                }
            }
            let Some(Object::Environment {
                parent,
                bindings,
                object_bindings,
                ..
            }) = self.objects.get(&env)
            else {
                return Err(ScriptError::Runtime("invalid environment"));
            };
            if object_bindings.contains(name) {
                let global = self.intrinsics.as_ref().unwrap().global;
                return self.get_member(Value::Object(global), name);
            }
            if let Some(v) = bindings.get(name) {
                return match v.clone() {
                    Some(v) => Ok(v),
                    None => {
                        self.throw_error("ReferenceError", "binding used before initialization")
                    }
                };
            }
            if let Some(p) = parent {
                env = *p;
            } else {
                break;
            }
        }
        if matches!(
            name,
            "Array"
                | "Symbol"
                | "BigInt"
                | "Proxy"
                | "Date"
                | "RegExp"
                | "Promise"
                | "Map"
                | "Set"
                | "WeakMap"
                | "WeakSet"
                | "Math"
                | "JSON"
                | "eval"
                | "AggregateError"
                | "ArrayBuffer"
                | "SharedArrayBuffer"
                | "DataView"
                | "Int8Array"
                | "Uint8ClampedArray"
                | "Int16Array"
                | "Uint16Array"
                | "Int32Array"
                | "Uint32Array"
                | "Float32Array"
                | "Float64Array"
                | "BigInt64Array"
                | "BigUint64Array"
                | "WeakRef"
                | "FinalizationRegistry"
                | "Uint8Array"
                | "$262"
        ) {
            return Err(ScriptError::Unsupported(
                "standard intrinsic not implemented",
            ));
        }
        self.throw_error("ReferenceError", "unknown identifier")
    }
    pub(super) fn initialize(&mut self, env: Id, name: &str, value: Value) -> Result<()> {
        if matches!(self.objects.get(&env), Some(Object::Environment{object_bindings,..}) if object_bindings.contains(name))
        {
            if let Some(i) = &self.intrinsics {
                let id = i.global;
                return if self.define(
                    id,
                    name,
                    super::objects::Descriptor {
                        value: Some(value),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: None,
                        ..super::objects::Descriptor::default()
                    },
                )? {
                    Ok(())
                } else {
                    self.throw_error("TypeError", "global definition refused")
                };
            }
        }
        let old = match self.objects.get(&env) {
            Some(Object::Environment { bindings, .. }) => bindings.get(name).cloned().flatten(),
            _ => return Err(ScriptError::Runtime("invalid environment")),
        };
        if let Some(id) = value.id() {
            self.runtime.link(env, id)?;
        }
        if let Some(id) = old.and_then(|v| v.id()) {
            self.runtime.unlink(env, id)?;
        }
        if let Some(Object::Environment { bindings, .. }) = self.objects.get_mut(&env) {
            bindings.insert(name.into(), Some(value));
        }
        Ok(())
    }
    pub(super) fn mutable_binding(&mut self, env: Id, name: &str, value: Value) -> Result<()> {
        if let Some(Object::Environment { mutable, .. }) = self.objects.get_mut(&env) {
            mutable.insert(name.into());
        }
        self.initialize(env, name, value)
    }
    pub(super) fn assign_binding(&mut self, mut env: Id, name: &str, value: Value) -> Result<()> {
        loop {
            if matches!(self.objects.get(&env),Some(Object::Environment{aliases,..})if aliases.contains_key(name))
            {
                return self.throw_error("TypeError", "assignment to imported binding");
            }
            let Some(Object::Environment {
                parent,
                bindings,
                mutable,
                ..
            }) = self.objects.get(&env)
            else {
                return Err(ScriptError::Runtime("invalid environment"));
            };
            if matches!(self.objects.get(&env),Some(Object::Environment{object_bindings,..})if object_bindings.contains(name))
            {
                let id = self.intrinsics.as_ref().unwrap().global;
                return self.set_member(Value::Object(id), name, value);
            }
            if let Some(v) = bindings.get(name) {
                if v.is_none() {
                    return self
                        .throw_error("ReferenceError", "binding used before initialization");
                }
                if !mutable.contains(name) {
                    return self.throw_error("TypeError", "assignment to constant");
                }
                return self.initialize(env, name, value);
            }
            match parent {
                Some(p) => env = *p,
                None => return self.throw_error("ReferenceError", "unknown identifier"),
            }
        }
    }
    pub(super) fn var_environment(&self, mut env: Id) -> Result<Id> {
        loop {
            match self.objects.get(&env) {
                Some(Object::Environment {
                    function_scope: true,
                    ..
                }) => return Ok(env),
                Some(Object::Environment {
                    parent: Some(p), ..
                }) => env = *p,
                _ => return Err(ScriptError::Runtime("missing function environment")),
            }
        }
    }
}
struct Frame<'a, 'host> {
    engine: &'a mut Engine<'host>,
    mark: usize,
}
impl Drop for Frame<'_, '_> {
    fn drop(&mut self) {
        for root in self.engine.roots.drain(self.mark..) {
            self.engine
                .runtime
                .unroot(root)
                .expect("owned execution root");
        }
    }
}

/// A VM between native event-loop invocations; it holds no borrowed host context.
pub struct SuspendedEngine(Engine<'static>);
impl SuspendedEngine {
    pub fn module_status(&mut self, url: &str) -> Result<super::modules::ModuleStatus> {
        self.0.module_status(url)
    }
    pub fn take_unhandled_rejections(&mut self) -> Vec<String> {
        self.0.take_unhandled_rejections()
    }

    pub fn pending_microtasks(&self) -> usize {
        self.0.pending_microtasks()
    }
    pub fn cancel_microtasks(&mut self) {
        self.0.cancel_microtasks();
        self.0.collect();
    }
    pub fn unhandled_rejection_count(&self) -> usize {
        self.0.unhandled_rejection_count()
    }

    /// Return native edges reachable from real VM roots or ONE native listener.
    /// Read-only: never allocate VM objects, call a host operation or collect.
    pub fn trace_native_edges(&self, listener: Option<u64>) -> Vec<u64> {
        let mut seeds = if let Some(remote) = listener {
            self.0
                .remote_nodes
                .iter()
                .filter(|(_, r)| **r == remote)
                .map(|(id, _)| *id)
                .collect()
        } else {
            self.0.runtime.root_ids()
        };
        // Empty seeds intentionally do not include the conservative native pins.
        let live = self.0.runtime.reachable(&seeds).expect("valid VM graph");
        seeds.clear();
        self.0
            .remote_nodes
            .iter()
            .filter(|(id, _)| live.contains(id))
            .map(|(_, remote)| *remote)
            .collect()
    }

    /// Resume against the same DOM registry, borrowing its current context.
    pub fn resume<'host>(
        self,
        adapter: Box<dyn super::adapter::DomAdapter + 'host>,
    ) -> Engine<'host> {
        self.0.rebind(Some(adapter))
    }
}
