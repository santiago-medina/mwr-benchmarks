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
    /// `break label` (also a desugared `continue label`), caught by `Stmt::Labeled`.
    LabelBreak(String),
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    BigInt(std::rc::Rc<super::bigint::Big>),
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
    Generator,
    GeneratorMethod,
    AsyncGenerator,
    AsyncGeneratorMethod,
    ClassConstructor,
    DerivedConstructor,
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
    /// Proxy exotic object (§10.5); `None` slots mean revoked. See PROXY.md.
    Proxy {
        target: Option<Id>,
        handler: Option<Id>,
        callable: bool,
        constructor: bool,
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
        /// Lexically declared `const` names (assignment throws even in sloppy code;
        /// other immutable bindings, such as a function expression's name, ignore it).
        constants: HashSet<String>,
        function_scope: bool,
        object_bindings: HashSet<String>,
        aliases: HashMap<String, (Id, String)>,
        /// Strict mode code (function and script environments read their body marker).
        strict: bool,
        /// Object environment record of a `with` statement.
        with_object: Option<Id>,
        /// `let`/`const`/class names (a sloppy eval `var` may not redeclare them).
        lexical: HashSet<String>,
    },
    Closure {
        kind: FunctionKind,
        program: Option<std::sync::Arc<super::asynchronous::Program>>,
        data: Data,
        environment: Id,
        params: Vec<parser::Param>,
        body: Vec<Stmt>,
    },
}

/// When the engine collects. See GC_SCHEDULING.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcPolicy {
    /// A full joint collection at every allocation safepoint (rooting tests).
    Stress,
    /// Collect when live objects reach max(MIN_GC_THRESHOLD, GC_GROWTH_FACTOR x live after the last GC).
    Threshold,
}
pub const MIN_GC_THRESHOLD: usize = 4096;
pub const GC_GROWTH_FACTOR: usize = 2;

pub struct Engine<'host> {
    pub(super) runtime: Runtime,
    pub(super) objects: crate::dual::IdMap<Object>,
    roots: Vec<ExternalRoot>,
    pub(super) modules: super::modules::Registry,
    pub(super) network: super::network::Network,
    pub(super) global: Option<ExternalRoot>,
    pub(super) intrinsics: Option<Intrinsics>,
    pub(super) template_cache: HashMap<u64, Id>,
    pub(super) test_harness: bool,
    pub(super) assertions: usize,
    pub(super) evaluation_depth: usize,
    pub(super) next_symbol: u64,
    pub(super) buffer_bytes: std::rc::Rc<std::cell::Cell<usize>>,
    pub(super) symbol_descriptions: HashMap<u64, Option<String>>,
    pub(super) symbol_registry: HashMap<String, u64>,
    pub(super) weak: super::weak::WeakState,
    pub(super) atomics: super::atomics::AtomicsState,
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
    /// Threshold policy: collect when live objects reach this (GC_SCHEDULING.md).
    next_gc: usize,
    seen_collections: u64,
    steps: usize,
    step_limit: usize,
    pub(super) call_depth: usize,
    /// Completion value of the running eval code (None outside eval).
    pub(super) completion: Option<(Value, Option<ExternalRoot>)>,
    pub(super) native_depth: usize,
    pub(super) adapter: Option<Box<dyn super::adapter::DomAdapter + 'host>>,
    pub(super) remote_nodes: crate::dual::IdMap<u64>,
    pub(super) adapter_error: Option<String>,
    pub(super) pending_release: Vec<u64>,
    pub(super) private_symbols: HashSet<u64>,
    pub(super) pending_construct: Option<(Value, Value)>,
}

impl<'host> Engine<'host> {
    pub fn new(dom: Model, js: Model) -> Result<Self> {
        Ok(Self {
            runtime: Runtime::new(dom, js)?,
            objects: Default::default(),
            roots: vec![],
            network: super::network::Network::default(),
            modules: super::modules::Registry::default(),
            global: None,
            intrinsics: None,
            template_cache: HashMap::new(),
            test_harness: false,
            assertions: 0,
            evaluation_depth: 0,
            next_symbol: super::symbols::WELL_KNOWN.len() as u64,
            buffer_bytes: std::rc::Rc::new(std::cell::Cell::new(0)),
            symbol_descriptions: HashMap::new(),
            symbol_registry: HashMap::new(),
            weak: Default::default(),
            atomics: Default::default(),
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
            next_gc: MIN_GC_THRESHOLD,
            seen_collections: 0,
            steps: 0,
            step_limit: 10_000,
            call_depth: 0,
            completion: None,
            native_depth: 0,
            adapter: None,
            remote_nodes: Default::default(),
            adapter_error: None,
            pending_release: vec![],
            private_symbols: HashSet::new(),
            pending_construct: None,
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
            template_cache: self.template_cache,
            test_harness: self.test_harness,
            assertions: self.assertions,
            evaluation_depth: self.evaluation_depth,
            next_symbol: self.next_symbol,
            buffer_bytes: self.buffer_bytes,
            symbol_descriptions: self.symbol_descriptions,
            symbol_registry: self.symbol_registry,
            weak: self.weak,
            atomics: self.atomics,
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
            next_gc: self.next_gc,
            seen_collections: self.seen_collections,
            steps: self.steps,
            step_limit: self.step_limit,
            call_depth: self.call_depth,
            completion: self.completion,
            native_depth: self.native_depth,
            remote_nodes: self.remote_nodes,
            adapter_error: self.adapter_error,
            pending_release: self.pending_release,
            private_symbols: self.private_symbols,
            pending_construct: self.pending_construct,
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
    /// Stress (the library default) collects at every allocation safepoint; embeddings
    /// that run real code select Threshold. See GC_SCHEDULING.md.
    pub fn set_gc_policy(&mut self, policy: GcPolicy) {
        self.stress = policy == GcPolicy::Stress;
    }
    pub fn gc_policy(&self) -> GcPolicy {
        if self.stress {
            GcPolicy::Stress
        } else {
            GcPolicy::Threshold
        }
    }
    pub fn collections(&self) -> u64 {
        self.runtime.collections()
    }
    /// Collect under Stress; under Threshold only once the live heap reached the trigger.
    pub(super) fn maybe_collect(&mut self) {
        if self.stress || self.runtime.live_objects() >= self.next_gc {
            self.collect();
        }
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
        self.seen_collections = self.runtime.collections();
        self.next_gc = self
            .runtime
            .live_objects()
            .saturating_mul(GC_GROWTH_FACTOR)
            .max(MIN_GC_THRESHOLD);
        let dead: Vec<_> = self
            .remote_nodes
            .iter()
            .filter(|(id, _)| self.runtime.value(**id).is_err())
            .map(|(_, remote)| *remote)
            .collect();
        self.remote_nodes
            .retain(|id, _| self.runtime.value(*id).is_ok());
        self.objects.retain(|id, _| self.runtime.value(*id).is_ok());
        self.scrub_weak();
        self.template_cache
            .retain(|_, id| self.objects.contains_key(id));
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
        self.cancel_weak_jobs();
        self.cancel_host_jobs();
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
        self.maybe_collect();
        if let Some(error) = self.adapter_error.clone() {
            return Err(ScriptError::Host(error));
        }
        let result = self.runtime.allocate(domain, tag, 64);
        // The budget can trigger backend collection under either policy.
        if self.runtime.collections() != self.seen_collections {
            self.prune_metadata();
        } else if !self.pending_release.is_empty() {
            self.flush_remote_releases();
        }
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
        let mut constants = HashSet::new();
        let mut lexical = HashSet::new();
        for stmt in parser::direct_statements(body) {
            match stmt {
                Stmt::Bind { kind, pattern, .. } if kind != "var" => {
                    for name in pattern.names() {
                        bindings.insert(name.clone(), None);
                        lexical.insert(name.clone());
                        if kind == "let" {
                            mutable.insert(name);
                        } else {
                            // const, using, await using: immutable bindings.
                            constants.insert(name);
                        }
                    }
                }

                Stmt::Const(n, _) | Stmt::Let(n, _) => {
                    bindings.insert(n.clone(), None);
                    lexical.insert(n.clone());
                    if matches!(stmt, Stmt::Let(..)) {
                        mutable.insert(n.clone());
                    } else {
                        constants.insert(n.clone());
                    }
                }
                Stmt::Function(n, ..)
                | Stmt::AsyncFunction(n, ..)
                | Stmt::GeneratorFunction(n, ..) => {
                    bindings.insert(n.clone(), Some(Value::Undefined));
                    mutable.insert(n.clone());
                }
                _ => {}
            }
        }
        if function_scope {
            let mut names = vec![];
            parser::var_names(body, &mut names);
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
            parser::var_names(body, &mut names);
            object_bindings.extend(names);
            for s in body.iter().map(parser::strip_labels) {
                if let Stmt::Function(n, ..)
                | Stmt::AsyncFunction(n, ..)
                | Stmt::GeneratorFunction(n, ..) = s
                {
                    object_bindings.insert(n.clone());
                }
            }
            bindings
                .entry("document".into())
                .or_insert(Some(Value::Document));
        }
        let strict = if function_scope {
            !matches!(body.first(), Some(Stmt::Sloppy))
        } else {
            parent.is_none_or(|p| self.env_strict(p))
        };
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
                constants,
                function_scope,
                object_bindings,
                aliases: HashMap::new(),
                strict,
                with_object: None,
                lexical,
            },
        );
        Ok(id)
    }
    pub(super) fn env_has_own(&self, env: Id, name: &str) -> bool {
        matches!(self.objects.get(&env), Some(Object::Environment { bindings, .. }) if bindings.contains_key(name))
    }
    pub(super) fn env_strict(&self, env: Id) -> bool {
        !matches!(
            self.objects.get(&env),
            Some(Object::Environment { strict: false, .. })
        )
    }
    pub(super) fn set_env_strict(&mut self, env: Id, value: bool) {
        if let Some(Object::Environment { strict, .. }) = self.objects.get_mut(&env) {
            *strict = value;
        }
    }
    /// A `with` statement's object environment over `object`, child of `parent`.
    pub(super) fn with_environment(&mut self, parent: Id, object: Id) -> Result<Id> {
        let env = self.new_environment(Some(parent), &[], false)?;
        self.runtime.link(env, object)?;
        if let Some(Object::Environment { with_object, .. }) = self.objects.get_mut(&env) {
            *with_object = Some(object);
        }
        Ok(env)
    }
    fn env_with_object(&self, env: Id) -> Option<Id> {
        match self.objects.get(&env) {
            Some(Object::Environment { with_object, .. }) => *with_object,
            _ => None,
        }
    }
    /// HasBinding of an object environment record (with @@unscopables).
    pub(super) fn with_binding(&mut self, object: Id, name: &str) -> Result<bool> {
        if !self.has(object, name)? {
            return Ok(false);
        }
        let unscopables = self.get_member(
            Value::Object(object),
            super::objects::Key::Symbol(super::symbols::UNSCOPABLES),
        )?;
        if let Value::Object(u) = unscopables {
            return Ok(!self.get_member(Value::Object(u), name)?.truthy());
        }
        Ok(true)
    }
    /// The `with` object that resolves `name` from `env`, if any (the call `this`).
    pub(super) fn with_base(&mut self, mut env: Id, name: &str) -> Result<Option<Id>> {
        loop {
            if let Some(object) = self.env_with_object(env) {
                if self.with_binding(object, name)? {
                    return Ok(Some(object));
                }
            }
            let Some(Object::Environment {
                parent, bindings, ..
            }) = self.objects.get(&env)
            else {
                return Ok(None);
            };
            if bindings.contains_key(name) {
                return Ok(None);
            }
            match parent {
                Some(p) => env = *p,
                None => return Ok(None),
            }
        }
    }
    /// `delete name` (sloppy code only): object records delete, declarative ones refuse.
    pub(super) fn delete_binding(&mut self, mut env: Id, name: &str) -> Result<bool> {
        loop {
            if let Some(object) = self.env_with_object(env) {
                if self.with_binding(object, name)? {
                    return self.delete(object, name);
                }
            }
            let Some(Object::Environment {
                parent,
                bindings,
                object_bindings,
                aliases,
                ..
            }) = self.objects.get(&env)
            else {
                return Err(ScriptError::Runtime("invalid environment"));
            };
            if object_bindings.contains(name) {
                let global = self.intrinsics.as_ref().unwrap().global;
                return self.delete(global, name);
            }
            if bindings.contains_key(name) || aliases.contains_key(name) {
                return Ok(false);
            }
            match parent {
                Some(p) => env = *p,
                None => break,
            }
        }
        match self.global_property(name)? {
            Some(global) => self.delete(global, name),
            None => Ok(true),
        }
    }
    /// The global object, if `name` is one of its properties (unresolved references).
    fn global_property(&mut self, name: &str) -> Result<Option<Id>> {
        let Some(i) = &self.intrinsics else {
            return Ok(None);
        };
        let global = i.global;
        Ok(self.has(global, name)?.then_some(global))
    }
    pub(super) fn lookup(&mut self, env: Id, name: &str) -> Result<Value> {
        self.lookup_with_base(env, name).map(|(v, _)| v)
    }
    /// ResolveBinding + GetBindingValue in one walk; also returns the `with` object
    /// that holds the binding (the call `this`, §13.3.6.1 EvaluateCall step 1.b.ii).
    pub(super) fn lookup_with_base(
        &mut self,
        mut env: Id,
        name: &str,
    ) -> Result<(Value, Option<Id>)> {
        loop {
            if let Some(object) = self.env_with_object(env) {
                if self.with_binding(object, name)? {
                    // §9.1.1.2.6 GetBindingValue: HasProperty again, then Get.
                    if !self.has(object, name)? {
                        if self.env_strict(env) {
                            return self
                                .throw_error("ReferenceError", &format!("{name} is not defined"));
                        }
                        return Ok((Value::Undefined, Some(object)));
                    }
                    let v = self.get_member(Value::Object(object), name)?;
                    return Ok((v, Some(object)));
                }
            }
            if let Some(Object::Environment { aliases, .. }) = self.objects.get(&env) {
                if let Some((target, name)) = aliases.get(name).cloned() {
                    return self.lookup_with_base(target, &name);
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
                return self
                    .get_member(Value::Object(global), name)
                    .map(|v| (v, None));
            }
            if let Some(v) = bindings.get(name) {
                return match v.clone() {
                    Some(v) => Ok((v, None)),
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
        ) && self.global_property(name)?.is_none()
        {
            return Err(ScriptError::Unsupported(
                "standard intrinsic not implemented",
            ));
        }
        if let Some(global) = self.global_property(name)? {
            return self
                .get_member(Value::Object(global), name)
                .map(|v| (v, None));
        }
        self.throw_error("ReferenceError", &format!("{name} is not defined"))
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
    /// ResolveBinding for strict assignment: true when `name` resolves nowhere, so
    /// PutValue must throw even if the right-hand side creates the global property.
    pub(super) fn unresolvable(&mut self, mut env: Id, name: &str) -> Result<bool> {
        loop {
            if let Some(object) = self.env_with_object(env) {
                if self.with_binding(object, name)? {
                    return Ok(false);
                }
            }
            let Some(Object::Environment {
                parent,
                bindings,
                aliases,
                object_bindings,
                ..
            }) = self.objects.get(&env)
            else {
                return Err(ScriptError::Runtime("invalid environment"));
            };
            if bindings.contains_key(name)
                || aliases.contains_key(name)
                || object_bindings.contains(name)
            {
                return Ok(false);
            }
            match parent {
                Some(p) => env = *p,
                None => return Ok(self.global_property(name)?.is_none()),
            }
        }
    }
    pub(super) fn assign_binding(&mut self, mut env: Id, name: &str, value: Value) -> Result<()> {
        let strict = self.env_strict(env);
        loop {
            if let Some(object) = self.env_with_object(env) {
                if self.with_binding(object, name)? {
                    // §9.1.1.2.5 SetMutableBinding steps 1-2.
                    if !self.has(object, name)? && strict {
                        return self
                            .throw_error("ReferenceError", &format!("{name} is not defined"));
                    }
                    return self.assign_member(Value::Object(object), name.into(), value, strict);
                }
            }
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
                    let constant = matches!(self.objects.get(&env), Some(Object::Environment { constants, .. }) if constants.contains(name));
                    if !strict && !constant {
                        return Ok(());
                    }
                    return self.throw_error("TypeError", "assignment to constant");
                }
                return self.initialize(env, name, value);
            }
            match parent {
                Some(p) => env = *p,
                None => {
                    if let Some(global) = self.global_property(name)? {
                        return self.assign_member(
                            Value::Object(global),
                            name.into(),
                            value,
                            strict,
                        );
                    }
                    if !strict {
                        // Sloppy mode: an unresolvable assignment creates a global property.
                        let global = self.intrinsics.as_ref().unwrap().global;
                        return self.assign_member(
                            Value::Object(global),
                            name.into(),
                            value,
                            false,
                        );
                    }
                    return self.throw_error("ReferenceError", &format!("{name} is not defined"));
                }
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
