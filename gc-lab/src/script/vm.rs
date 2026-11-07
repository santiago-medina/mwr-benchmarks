//! Tree-walking interpreter. Environments and closures are JS arena objects;
//! metadata alone is never a GC root. Temporary values are rooted by frames.
use super::parser::{self, Expr, Stmt, SyntaxError};
use crate::{
    dual::{Domain, ExternalRoot, Id, Runtime},
    Error, Model,
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    Syntax(SyntaxError),
    Runtime(&'static str),
    Memory(Error),
}
impl From<Error> for ScriptError {
    fn from(e: Error) -> Self {
        Self::Memory(e)
    }
}
type Result<T> = std::result::Result<T, ScriptError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Undefined,
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
#[derive(Clone)]
pub(super) enum Object {
    Node {
        tag: String,
        text: String,
        listeners: Vec<Id>,
    },
    Environment {
        parent: Option<Id>,
        bindings: HashMap<String, Option<Value>>,
    },
    Closure {
        environment: Id,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
}

pub struct Engine {
    pub(super) runtime: Runtime,
    pub(super) objects: HashMap<Id, Object>,
    roots: Vec<ExternalRoot>,
    global: Option<ExternalRoot>,
    stress: bool,
    steps: usize,
    step_limit: usize,
    call_depth: usize,
    pub(super) native_depth: usize,
}

impl Engine {
    pub fn new(dom: Model, js: Model) -> Result<Self> {
        Ok(Self {
            runtime: Runtime::new(dom, js)?,
            objects: HashMap::new(),
            roots: vec![],
            global: None,
            stress: true,
            steps: 0,
            step_limit: 10_000,
            call_depth: 0,
            native_depth: 0,
        })
    }
    pub fn set_stress(&mut self, enabled: bool) {
        self.stress = enabled;
    }
    pub fn set_budget(&mut self, bytes: Option<usize>) {
        self.runtime.set_budget(bytes);
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
        self.objects.retain(|id, _| self.runtime.value(*id).is_ok());
    }
    pub fn release_globals(&mut self) {
        if let Some(global) = self.global.take() {
            self.runtime.unroot(global).expect("owned global root");
        }
        self.collect();
    }
    pub fn binding_string(&self, name: &str) -> Result<String> {
        match self.binding(name)? {
            Value::String(value) => Ok(value),
            _ => Err(ScriptError::Runtime("not a string")),
        }
    }
    pub(super) fn binding(&self, name: &str) -> Result<Value> {
        let global = self
            .global
            .ok_or(ScriptError::Runtime("no global environment"))?;
        self.lookup(self.runtime.resolve(global)?, name)
    }
    pub fn execute(&mut self, source: &str) -> Result<()> {
        let program = parser::parse(source).map_err(ScriptError::Syntax)?;
        self.release_globals();
        self.steps = 0;
        let global = self.new_environment(None, &program)?;
        self.global = Some(self.runtime.root(global)?);
        let result = self.block(&program, global, false).map(|_| ());
        if result.is_err() {
            self.release_globals();
        } else {
            self.collect();
        }
        result
    }
    fn tick(&mut self) -> Result<()> {
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
        let result = self.runtime.allocate(domain, tag, 64);
        // The budget can trigger backend collection even with stress disabled.
        self.objects.retain(|id, _| self.runtime.value(*id).is_ok());
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
    fn new_environment(&mut self, parent: Option<Id>, body: &[Stmt]) -> Result<Id> {
        let mut bindings = HashMap::new();
        for stmt in body {
            if let Stmt::Const(name, _) = stmt {
                if bindings.insert(name.clone(), None).is_some() {
                    return Err(ScriptError::Runtime("duplicate lexical binding"));
                }
            }
        }
        if parent.is_none() {
            bindings
                .entry("document".into())
                .or_insert(Some(Value::Document));
        }
        let id = self.allocate(Domain::Js, -300)?;
        if let Some(parent) = parent {
            self.runtime.link(id, parent)?;
        }
        self.objects
            .insert(id, Object::Environment { parent, bindings });
        Ok(id)
    }
    fn lookup(&self, mut env: Id, name: &str) -> Result<Value> {
        loop {
            let Some(Object::Environment { parent, bindings }) = self.objects.get(&env) else {
                return Err(ScriptError::Runtime("invalid environment"));
            };
            if let Some(value) = bindings.get(name) {
                return value
                    .clone()
                    .ok_or(ScriptError::Runtime("binding used before initialization"));
            }
            env = parent.ok_or(ScriptError::Runtime("unknown identifier"))?;
        }
    }
    fn initialize(&mut self, env: Id, name: &str, value: Value) -> Result<()> {
        if let Some(id) = value.id() {
            self.runtime.link(env, id)?;
        }
        let Some(Object::Environment { bindings, .. }) = self.objects.get_mut(&env) else {
            return Err(ScriptError::Runtime("invalid environment"));
        };
        bindings.insert(name.to_owned(), Some(value));
        Ok(())
    }
    fn block(&mut self, body: &[Stmt], env: Id, can_return: bool) -> Result<Option<Value>> {
        for stmt in body {
            self.tick()?;
            match stmt {
                Stmt::Const(name, expr) => {
                    let value = self.eval(expr, env)?;
                    self.initialize(env, name, value)?;
                }
                Stmt::Set(receiver, property, expr) => self.frame(|e| {
                    let receiver = e.eval(receiver, env)?;
                    e.protect(&receiver)?;
                    let value = e.eval(expr, env)?;
                    e.protect(&value)?;
                    e.set_member(receiver, property, value)
                })?,
                Stmt::Expr(expr) => {
                    self.eval(expr, env)?;
                }
                Stmt::Return(expr) => {
                    if !can_return {
                        return Err(ScriptError::Runtime("return outside function"));
                    }
                    return Ok(Some(self.eval(expr, env)?));
                }
            }
            if self.stress {
                self.collect();
            }
        }
        Ok(None)
    }
    fn eval(&mut self, expr: &Expr, env: Id) -> Result<Value> {
        self.tick()?;
        self.frame(|e| match expr {
            Expr::Name(name) => e.lookup(env, name),
            Expr::String(value) => Ok(Value::String(value.clone())),
            Expr::Arrow { params, body } => {
                let id = e.allocate(Domain::Js, -400)?;
                e.runtime.link(id, env)?;
                e.objects.insert(
                    id,
                    Object::Closure {
                        environment: env,
                        params: params.clone(),
                        body: body.clone(),
                    },
                );
                Ok(Value::Object(id))
            }
            Expr::Member(receiver, property) => {
                let receiver = e.eval(receiver, env)?;
                e.protect(&receiver)?;
                e.get_member(receiver, property)
            }
            Expr::Call(callee, args) => {
                let callee = e.eval(callee, env)?;
                e.protect(&callee)?;
                let mut values = Vec::new();
                for arg in args {
                    let value = e.eval(arg, env)?;
                    e.protect(&value)?;
                    values.push(value);
                }
                e.invoke(callee, values)
            }
        })
    }
    pub(super) fn invoke(&mut self, callee: Value, args: Vec<Value>) -> Result<Value> {
        self.tick()?;
        if let Value::Native { receiver, name } = callee {
            return self.call_native(receiver, &name, args);
        }
        let Value::Object(id) = callee else {
            return Err(ScriptError::Runtime("value is not callable"));
        };
        let Some(Object::Closure {
            environment,
            params,
            body,
        }) = self.objects.get(&id).cloned()
        else {
            return Err(ScriptError::Runtime("value is not callable"));
        };
        if self.call_depth >= 32 {
            return Err(ScriptError::Runtime("call depth limit"));
        }
        self.frame(|e| {
            e.protect(&Value::Object(id))?;
            for arg in &args {
                e.protect(arg)?;
            }
            let local = e.new_environment(Some(environment), &body)?;
            e.protect(&Value::Object(local))?;
            for (index, name) in params.iter().enumerate() {
                // Parameters and const declarations may not collide in this subset.
                if let Some(Object::Environment { bindings, .. }) = e.objects.get(&local) {
                    if bindings.contains_key(name) {
                        return Err(ScriptError::Runtime(
                            "parameter and lexical binding collide",
                        ));
                    }
                }
                e.initialize(
                    local,
                    name,
                    args.get(index).cloned().unwrap_or(Value::Undefined),
                )?;
            }
            e.call_depth += 1;
            let result = e.block(&body, local, true);
            e.call_depth -= 1;
            Ok(result?.unwrap_or(Value::Undefined))
        })
    }
}
struct Frame<'a> {
    engine: &'a mut Engine,
    mark: usize,
}
impl Drop for Frame<'_> {
    fn drop(&mut self) {
        for root in self.engine.roots.drain(self.mark..) {
            self.engine
                .runtime
                .unroot(root)
                .expect("owned execution root");
        }
    }
}
