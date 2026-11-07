//! Evaluation and completion propagation. Every allocating operation runs under a root frame.
use super::{
    objects::{Data, Descriptor, Key, Kind},
    parser::{Expr, Property as Literal, Stmt},
    vm::{Engine, Exception, FunctionKind, Object, ScriptError, Value},
};
use crate::dual::{Domain, Id};
type Result<T> = std::result::Result<T, ScriptError>;
impl Value {
    pub(super) fn number(n: f64) -> Self {
        Self::Number(n.to_bits())
    }
    pub(super) fn truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Bool(b) => *b,
            Self::Number(n) => {
                let n = f64::from_bits(*n);
                n != 0.0 && !n.is_nan()
            }
            Self::String(s) => !s.is_empty(),
            _ => true,
        }
    }
    pub(super) fn same_value(&self, b: &Self) -> bool {
        match (self, b) {
            (Self::Number(a), Self::Number(b)) => {
                a == b || (f64::from_bits(*a).is_nan() && f64::from_bits(*b).is_nan())
            }
            _ => self == b,
        }
    }
    pub(super) fn strict_equal(&self, b: &Self) -> bool {
        match (self, b) {
            (Self::Number(a), Self::Number(b)) => f64::from_bits(*a) == f64::from_bits(*b),
            _ => self == b,
        }
    }
}
pub(super) fn number_string(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    if n.abs() >= 1e21 || n.abs() < 1e-6 {
        let s = format!("{n:e}");
        if let Some((a, b)) = s.split_once('e') {
            return format!("{a}e{}{b}", if b.starts_with('-') { "" } else { "+" });
        }
    }
    n.to_string()
}
impl Engine<'_> {
    pub(super) fn callable(&self, v: &Value) -> bool {
        matches!(v, Value::Native { .. })
            || matches!(v,Value::Object(id) if matches!(self.objects.get(id),Some(Object::Closure{..}|Object::Builtin{..}|Object::Bound{..})))
    }
    pub(super) fn thrown(&self, value: Value) -> ScriptError {
        let (name, message) = if let Value::Object(id) = &value {
            if let Ok(d) = self.data(*id) {
                (
                    d.error_class.clone(),
                    match d.own("message").map(|p| &p.kind) {
                        Some(Kind::Data {
                            value: Value::String(s),
                            ..
                        }) => s.clone(),
                        _ => String::new(),
                    },
                )
            } else {
                (None, String::new())
            }
        } else {
            (None, String::new())
        };
        ScriptError::Thrown(Exception {
            name,
            message,
            value,
        })
    }
    pub(super) fn make_function(
        &mut self,
        env: Id,
        name: Option<&str>,
        params: &[String],
        body: &[Stmt],
        kind: FunctionKind,
        named: bool,
    ) -> Result<Value> {
        let env = if named {
            let local = self.new_environment(Some(env), &[], false)?;
            self.protect(&Value::Object(local))?;
            local
        } else {
            env
        };
        let id = self.allocate(Domain::Js, -400)?;
        self.runtime.link(id, env)?;
        let program = if matches!(
            kind,
            FunctionKind::Async | FunctionKind::AsyncArrow | FunctionKind::AsyncMethod
        ) {
            Some(super::asynchronous::compile(body)?)
        } else {
            None
        };
        self.objects.insert(
            id,
            Object::Closure {
                environment: env,
                params: params.to_vec(),
                body: body.to_vec(),
                kind,
                program,
                data: Data::new(None),
            },
        );
        self.protect(&Value::Object(id))?;
        self.replace_data(
            id,
            Data::new(Some(
                if matches!(
                    kind,
                    FunctionKind::Async | FunctionKind::AsyncArrow | FunctionKind::AsyncMethod
                ) {
                    self.intrinsics.as_ref().unwrap().async_function
                } else {
                    self.intrinsics.as_ref().unwrap().function
                },
            )),
        )?;
        self.define(
            id,
            "name",
            Descriptor {
                value: Some(Value::String(name.unwrap_or("").into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        self.define(
            id,
            "length",
            Descriptor {
                value: Some(Value::number(params.len() as f64)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        if kind == FunctionKind::Ordinary {
            let p = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
            self.protect(&Value::Object(p))?;
            self.define(
                p,
                "constructor",
                Descriptor {
                    value: Some(Value::Object(id)),
                    writable: Some(true),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
            self.define(
                id,
                "prototype",
                Descriptor {
                    value: Some(Value::Object(p)),
                    writable: Some(true),
                    ..Descriptor::default()
                },
            )?;
        }
        if named {
            if let Some(n) = name {
                self.initialize(env, n, Value::Object(id))?;
            }
        }
        Ok(Value::Object(id))
    }
    pub(super) fn hoist_functions(&mut self, body: &[Stmt], env: Id) -> Result<()> {
        for s in body {
            if let Stmt::Function(n, p, b) | Stmt::AsyncFunction(n, p, b) = s {
                let kind = if matches!(s, Stmt::AsyncFunction(..)) {
                    FunctionKind::Async
                } else {
                    FunctionKind::Ordinary
                };
                let name = if n == "\0default" { "default" } else { n };
                let f = self.make_function(env, Some(name), p, b, kind, false)?;
                self.initialize(env, n, f)?;
            }
        }
        Ok(())
    }
    pub(super) fn block(
        &mut self,
        body: &[Stmt],
        env: Id,
        can_return: bool,
    ) -> Result<Option<Value>> {
        self.hoist_functions(body, env)?;
        for s in body {
            self.tick()?;
            let completion = self.frame(|e| e.statement(s, env, can_return))?;
            if completion.is_some() {
                return Ok(completion);
            }
        }
        Ok(None)
    }
    fn scoped(&mut self, body: &[Stmt], env: Id, can_return: bool) -> Result<Option<Value>> {
        let local = self.new_environment(Some(env), body, false)?;
        self.protect(&Value::Object(local))?;
        self.block(body, local, can_return)
    }
    pub(super) fn statement(
        &mut self,
        s: &Stmt,
        env: Id,
        can_return: bool,
    ) -> Result<Option<Value>> {
        match s {
            Stmt::Const(n, x) | Stmt::Let(n, x) => {
                let v = self.eval(x, env)?;
                self.initialize(env, n, v)?;
            }
            Stmt::Var(n, x) => {
                if let Some(x) = x {
                    let v = self.eval(x, env)?;
                    let local = self.var_environment(env)?;
                    self.initialize(local, n, v)?;
                }
            }
            Stmt::Break => return Err(ScriptError::LoopBreak),
            Stmt::Continue => return Err(ScriptError::LoopContinue),
            Stmt::ForOf {
                kind,
                name,
                iterable,
                body,
            } => return self.for_of(kind, name, iterable, body, env, can_return),
            Stmt::Function(..) | Stmt::AsyncFunction(..) => {}
            Stmt::Expr(x) => {
                self.eval(x, env)?;
            }
            Stmt::Return(x) => {
                if !can_return {
                    return Err(ScriptError::Runtime("return outside function"));
                }
                return Ok(Some(self.eval(x, env)?));
            }
            Stmt::Throw(x) => {
                let v = self.eval(x, env)?;
                return Err(self.thrown(v));
            }
            Stmt::Set(a, k, b) => {
                let a = self.eval(a, env)?;
                self.protect(&a)?;
                let b = self.eval(b, env)?;
                self.protect(&b)?;
                self.set_member(a, k, b)?;
            }
            Stmt::Block(b) => return self.scoped(b, env, can_return),
            Stmt::If(c, a, b) => {
                if self.eval(c, env)?.truthy() {
                    return self.statement(a, env, can_return);
                }
                if let Some(b) = b {
                    return self.statement(b, env, can_return);
                }
            }
            Stmt::Try {
                body,
                catch,
                finally,
            } => {
                let mut pending = self.scoped(body, env, can_return);
                if let (Err(ScriptError::Thrown(ex)), Some((name, b))) = (&pending, catch) {
                    let value = ex.value.clone();
                    self.protect(&value)?;
                    let local = self.new_environment(Some(env), b, false)?;
                    self.protect(&Value::Object(local))?;
                    self.mutable_binding(local, name, value)?;
                    pending = self.block(b, local, can_return);
                }
                if matches!(
                    pending,
                    Err(ScriptError::Memory(_)
                        | ScriptError::Runtime(_)
                        | ScriptError::Unsupported(_)
                        | ScriptError::Host(_))
                ) {
                    return pending;
                }
                match &pending {
                    Ok(Some(v)) => self.protect(v)?,
                    Err(ScriptError::Thrown(ex)) => self.protect(&ex.value)?,
                    _ => {}
                }
                if let Some(b) = finally {
                    match self.scoped(b, env, can_return) {
                        Ok(None) => {}
                        other => return other,
                    }
                }
                return pending;
            }
        }
        Ok(None)
    }
    fn for_of(
        &mut self,
        kind: &str,
        name: &str,
        source: &Expr,
        body: &Stmt,
        env: Id,
        can_return: bool,
    ) -> Result<Option<Value>> {
        let scope = if kind == "var" {
            env
        } else {
            let id = self.new_environment(
                Some(env),
                &[Stmt::Const(name.into(), Expr::Undefined)],
                false,
            )?;
            self.protect(&Value::Object(id))?;
            id
        };
        let value = self.eval(source, scope)?;
        self.protect(&value)?;
        let method = self.get_member(value.clone(), Key::Symbol(0))?;
        self.protect(&method)?;
        let iterator = self.invoke_this(method, value, vec![])?;
        self.protect(&iterator)?;
        if !matches!(iterator, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator is not object");
        }
        let next = self.get_member(iterator.clone(), "next")?;
        self.protect(&next)?;
        loop {
            self.tick()?;
            let (done, completion) = self.frame(|e| {
                let result = e.invoke_this(next.clone(), iterator.clone(), vec![])?;
                e.protect(&result)?;
                if !matches!(result, Value::Object(_)) {
                    return e.throw_error("TypeError", "iterator result is not object");
                }
                if e.get_member(result.clone(), "done")?.truthy() {
                    return Ok((true, None));
                }
                let value = e.get_member(result, "value")?;
                e.protect(&value)?;
                let pending = e.frame(|e| {
                    let local = if kind == "var" {
                        e.var_environment(env)?
                    } else {
                        let stmt = if kind == "const" {
                            Stmt::Const(name.into(), Expr::Undefined)
                        } else {
                            Stmt::Let(name.into(), Expr::Undefined)
                        };
                        let id = e.new_environment(Some(scope), &[stmt], false)?;
                        e.protect(&Value::Object(id))?;
                        id
                    };
                    e.initialize(local, name, value)?;
                    e.statement(body, local, can_return)
                });
                match pending {
                    Ok(None) | Err(ScriptError::LoopContinue) => Ok((false, None)),
                    Err(ScriptError::LoopBreak) => e
                        .close_iterator(iterator.clone(), Ok(None))
                        .map(|v| (true, v)),
                    other => e.close_iterator(iterator.clone(), other).map(|v| (true, v)),
                }
            })?;
            if done {
                return Ok(completion);
            }
        }
    }
    fn close_iterator(
        &mut self,
        iterator: Value,
        pending: Result<Option<Value>>,
    ) -> Result<Option<Value>> {
        match &pending {
            Ok(Some(v)) => self.protect(v)?,
            Err(ScriptError::Thrown(e)) => self.protect(&e.value)?,
            Err(_) => return pending,
            _ => {}
        }
        let close = self.frame(|e| {
            let method = e.get_member(iterator.clone(), "return")?;
            e.protect(&method)?;
            if matches!(method, Value::Undefined | Value::Null) {
                return Ok(());
            }
            let value = e.invoke_this(method, iterator, vec![])?;
            if !matches!(value, Value::Object(_)) {
                return e.throw_error("TypeError", "iterator return is not object");
            }
            Ok(())
        });
        if let Err(error) = &close {
            if !matches!(error, ScriptError::Thrown(_)) {
                return Err(error.clone());
            }
        }
        if matches!(pending, Err(ScriptError::Thrown(_))) {
            return pending;
        }
        close?;
        pending
    }
    pub(super) fn eval(&mut self, x: &Expr, env: Id) -> Result<Value> {
        self.tick()?;
        if self.evaluation_depth >= 128 {
            return Err(ScriptError::Runtime("evaluation depth limit"));
        }
        self.evaluation_depth += 1;
        let r = self.frame(|e| e.eval_inner(x, env));
        self.evaluation_depth -= 1;
        r
    }
    fn eval_inner(&mut self, x: &Expr, env: Id) -> Result<Value> {
        match x {
            Expr::Name(n) => self.lookup(env, n),
            Expr::This => self.lookup(env, "this"),
            Expr::String(s) => Ok(Value::String(s.clone())),
            Expr::Number(n) => Ok(Value::number(n.parse().unwrap())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Undefined => Ok(Value::Undefined),
            Expr::Arrow { params, body } => {
                self.make_function(env, None, params, body, FunctionKind::Arrow, false)
            }
            Expr::Function {
                name,
                params,
                body,
                method,
            } => self.make_function(
                env,
                name.as_deref(),
                params,
                body,
                if *method {
                    FunctionKind::Method
                } else {
                    FunctionKind::Ordinary
                },
                name.is_some(),
            ),
            Expr::Await(_) => Err(ScriptError::Runtime("await requires continuation")),
            Expr::AsyncArrow { params, body } => {
                self.make_function(env, None, params, body, FunctionKind::AsyncArrow, false)
            }
            Expr::AsyncFunction {
                name,
                params,
                body,
                method,
            } => self.make_function(
                env,
                name.as_deref(),
                params,
                body,
                if *method {
                    FunctionKind::AsyncMethod
                } else {
                    FunctionKind::Async
                },
                name.is_some(),
            ),
            Expr::Array(elements) => {
                let id = self.new_array(elements.len() as u32)?;
                self.protect(&Value::Object(id))?;
                for (i, x) in elements.iter().enumerate() {
                    if let Some(x) = x {
                        let v = self.eval(x, env)?;
                        self.protect(&v)?;
                        self.put_data(id, i.to_string(), v)?;
                    }
                }
                Ok(Value::Object(id))
            }
            Expr::Object(p) => self.eval_object(p, env),
            Expr::Member(a, k) => {
                let a = self.eval(a, env)?;
                self.protect(&a)?;
                self.get_member(a, k)
            }
            Expr::Computed(a, k) => {
                let a = self.eval(a, env)?;
                self.protect(&a)?;
                let k = self.eval(k, env)?;
                self.protect(&k)?;
                let k = self.member_key(&a, k)?;
                self.get_member(a, &k)
            }
            Expr::Call(f, args) => self.eval_call(f, args, env, false),
            Expr::New(f, args) => self.eval_call(f, args, env, true),
            Expr::Unary(op, a) => self.eval_unary(op, a, env),
            Expr::Binary(op, a, b) => self.eval_binary(op, a, b, env),
            Expr::Assign(a, b) => match a.as_ref() {
                Expr::Name(n) => {
                    let v = self.eval(b, env)?;
                    self.assign_binding(env, n, v.clone())?;
                    Ok(v)
                }
                Expr::Member(r, k) => {
                    let r = self.eval(r, env)?;
                    self.protect(&r)?;
                    let v = self.eval(b, env)?;
                    self.protect(&v)?;
                    self.set_member(r, k, v.clone())?;
                    Ok(v)
                }
                Expr::Computed(r, k) => {
                    let r = self.eval(r, env)?;
                    self.protect(&r)?;
                    let k = self.eval(k, env)?;
                    self.protect(&k)?;
                    let v = self.eval(b, env)?;
                    self.protect(&v)?;
                    let k = self.member_key(&r, k)?;
                    self.set_member(r, &k, v.clone())?;
                    Ok(v)
                }
                _ => Err(ScriptError::Runtime("invalid assignment")),
            },
        }
    }
    fn member_key(&mut self, receiver: &Value, key: Value) -> Result<Key> {
        if matches!(receiver, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "nullish property receiver");
        }
        self.to_key(key)
    }
    fn eval_object(&mut self, props: &[(String, Literal)], env: Id) -> Result<Value> {
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        for (k, p) in props {
            self.literal_property(id, k, p, env)?;
        }
        Ok(Value::Object(id))
    }
    pub(super) fn literal_property(&mut self, id: Id, k: &str, p: &Literal, env: Id) -> Result<()> {
        let d = match p {
            Literal::Prototype(x) => {
                let v = self.eval(x, env)?;
                match v {
                    Value::Object(p) => {
                        self.set_prototype(id, Some(p))?;
                    }
                    Value::Null => {
                        self.set_prototype(id, None)?;
                    }
                    _ => {}
                }
                return Ok(());
            }
            Literal::Value(x) => {
                let v = self.eval(x, env)?;
                self.protect(&v)?;
                Descriptor {
                    value: Some(v),
                    writable: Some(true),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Descriptor::default()
                }
            }
            Literal::Getter(b) => {
                let f = self.make_function(env, Some(k), &[], b, FunctionKind::Method, false)?;
                Descriptor {
                    get: Some(f),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Descriptor::default()
                }
            }
            Literal::Setter(n, b) => {
                let f = self.make_function(
                    env,
                    Some(k),
                    std::slice::from_ref(n),
                    b,
                    FunctionKind::Method,
                    false,
                )?;
                Descriptor {
                    set: Some(f),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Descriptor::default()
                }
            }
        };
        self.define(id, k, d)?;
        Ok(())
    }
    fn eval_call(&mut self, f: &Expr, args: &[Expr], env: Id, new: bool) -> Result<Value> {
        let (receiver, callee) = if !new {
            match f {
                Expr::Member(r, k) => {
                    let r = self.eval(r, env)?;
                    self.protect(&r)?;
                    let f = self.get_member(r.clone(), k)?;
                    (r, f)
                }
                Expr::Computed(r, k) => {
                    let r = self.eval(r, env)?;
                    self.protect(&r)?;
                    let k = self.eval(k, env)?;
                    self.protect(&k)?;
                    let k = self.member_key(&r, k)?;
                    let f = self.get_member(r.clone(), &k)?;
                    (r, f)
                }
                _ => (Value::Undefined, self.eval(f, env)?),
            }
        } else {
            (Value::Undefined, self.eval(f, env)?)
        };
        self.protect(&callee)?;
        let mut values = vec![];
        for x in args {
            let v = self.eval(x, env)?;
            self.protect(&v)?;
            values.push(v);
        }
        if new {
            self.construct(callee, values)
        } else {
            self.invoke_this(callee, receiver, values)
        }
    }
    pub(super) fn invoke(&mut self, f: Value, args: Vec<Value>) -> Result<Value> {
        self.invoke_this(f, Value::Undefined, args)
    }
    pub(super) fn invoke_this(
        &mut self,
        f: Value,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        self.tick()?;
        if self.call_depth >= 32 {
            return Err(ScriptError::Runtime("call depth limit"));
        }
        self.call_depth += 1;
        let r = self.frame(|e| {
            e.protect(&f)?;
            e.protect(&receiver)?;
            for a in &args {
                e.protect(a)?;
            }
            e.invoke_inner(f, receiver, args)
        });
        self.call_depth -= 1;
        match r {
            Err(ScriptError::Host(message)) if self.adapter_error.is_none() => {
                self.throw_error("Error", &message)
            }
            other => other,
        }
    }
    fn invoke_inner(&mut self, f: Value, receiver: Value, args: Vec<Value>) -> Result<Value> {
        if let Value::Native { receiver, name } = f {
            return self.call_native(receiver, &name, args);
        }
        let Value::Object(id) = f else {
            return self.throw_error("TypeError", "value is not callable");
        };
        match self.objects.get(&id).cloned() {
            Some(Object::Builtin { name, data }) => {
                if let Some(action) = data.internal {
                    self.promise_internal(action, args)
                } else {
                    self.builtin(&name, receiver, args)
                }
            }
            Some(Object::Bound {
                target,
                receiver,
                args: mut prefix,
                ..
            }) => {
                prefix.extend(args);
                self.invoke_this(target, receiver, prefix)
            }
            Some(Object::Closure {
                environment,
                params,
                body,
                kind,
                program,
                ..
            }) => {
                if let Some(program) = program {
                    return self.start_async(
                        environment,
                        params,
                        body,
                        kind,
                        receiver,
                        args,
                        program,
                    );
                }
                let local = self.new_environment(Some(environment), &body, true)?;
                self.protect(&Value::Object(local))?;
                if kind != FunctionKind::Arrow {
                    self.initialize(local, "this", receiver)?;
                    let a = self.arguments_object(&args)?;
                    self.initialize(local, "arguments", a)?;
                }
                for (i, n) in params.iter().enumerate() {
                    self.mutable_binding(
                        local,
                        n,
                        args.get(i).cloned().unwrap_or(Value::Undefined),
                    )?;
                }
                Ok(self.block(&body, local, true)?.unwrap_or(Value::Undefined))
            }
            _ => self.throw_error("TypeError", "value is not callable"),
        }
    }
    pub(super) fn constructible(&self, value: &Value) -> bool {
        let Value::Object(id) = value else {
            return false;
        };
        match self.objects.get(id) {
            Some(Object::Closure {
                kind: FunctionKind::Ordinary,
                ..
            }) => true,
            Some(Object::Bound { target, .. }) => self.constructible(target),
            Some(Object::Builtin { name, .. }) => {
                matches!(
                    name.as_str(),
                    "Promise"
                        | "Array"
                        | "Object"
                        | "Function"
                        | "AsyncFunction"
                        | "String"
                        | "Number"
                        | "Boolean"
                ) || self
                    .intrinsics
                    .as_ref()
                    .is_some_and(|i| i.errors.contains_key(name))
            }
            _ => false,
        }
    }
    pub(super) fn construct(&mut self, f: Value, args: Vec<Value>) -> Result<Value> {
        self.construct_with_new_target(f.clone(), args, f)
    }
    pub(super) fn construct_with_new_target(
        &mut self,
        f: Value,
        args: Vec<Value>,
        new_target: Value,
    ) -> Result<Value> {
        let Value::Object(id) = f.clone() else {
            return self.throw_error("TypeError", "not a constructor");
        };
        match self.objects.get(&id).cloned() {
            Some(Object::Bound {
                target,
                args: mut prefix,
                ..
            }) => {
                prefix.extend(args);
                self.construct_with_new_target(
                    target.clone(),
                    prefix,
                    if new_target == f { target } else { new_target },
                )
            }
            Some(Object::Builtin { name, .. }) => {
                if name == "Promise" {
                    return self.promise_constructor_target(
                        new_target,
                        args.first().cloned().unwrap_or(Value::Undefined),
                    );
                }
                if new_target != f {
                    return Err(ScriptError::Unsupported("custom newTarget for builtin"));
                }
                if matches!(name.as_str(), "String" | "Number" | "Boolean") {
                    let v = self.builtin(&name, Value::Undefined, args)?;
                    self.box_value(v).map(Value::Object)
                } else if name == "Function" || name == "AsyncFunction" {
                    Err(ScriptError::Unsupported("dynamic Function constructor"))
                } else if name == "Object"
                    || name == "Array"
                    || self.intrinsics.as_ref().unwrap().errors.contains_key(&name)
                {
                    self.builtin(&name, Value::Undefined, args)
                } else {
                    self.throw_error("TypeError", "builtin not a constructor")
                }
            }
            Some(Object::Closure {
                kind: FunctionKind::Ordinary,
                ..
            }) => {
                let p = self.get_member(new_target, "prototype")?;
                let p = if let Value::Object(p) = p {
                    p
                } else {
                    self.intrinsics.as_ref().unwrap().object
                };
                let id = self.new_object(Some(p))?;
                let receiver = Value::Object(id);
                self.protect(&receiver)?;
                let result = self.invoke_this(f, receiver.clone(), args)?;
                Ok(if matches!(result, Value::Object(_)) {
                    result
                } else {
                    receiver
                })
            }
            _ => self.throw_error("TypeError", "not a constructor"),
        }
    }
    fn eval_unary(&mut self, op: &str, x: &Expr, env: Id) -> Result<Value> {
        if op == "delete" {
            let target = match x {
                Expr::Member(r, k) => Some((self.eval(r, env)?, Key::from(k))),
                Expr::Computed(r, k) => {
                    let r = self.eval(r, env)?;
                    self.protect(&r)?;
                    let k = self.eval(k, env)?;
                    self.protect(&k)?;
                    {
                        let key = self.member_key(&r, k)?;
                        Some((r, key))
                    }
                }
                _ => {
                    self.eval(x, env)?;
                    None
                }
            };
            if let Some((Value::Object(id), k)) = target {
                if !self.delete(id, &k)? {
                    return self.throw_error("TypeError", "cannot delete property");
                }
            }
            return Ok(Value::Bool(true));
        }
        let v = if op == "typeof" {
            match self.eval(x, env) {
                Err(ScriptError::Thrown(ex))
                    if matches!(x, Expr::Name(_))
                        && ex.name.as_deref() == Some("ReferenceError")
                        && ex.message == "unknown identifier" =>
                {
                    Value::Undefined
                }
                r => r?,
            }
        } else {
            self.eval(x, env)?
        };
        self.protect(&v)?;
        match op {
            "!" => Ok(Value::Bool(!v.truthy())),
            "void" => Ok(Value::Undefined),
            "+" => Ok(Value::number(self.to_number(v)?)),
            "-" => Ok(Value::number(-self.to_number(v)?)),
            "typeof" => Ok(Value::String(
                match v {
                    Value::Undefined => "undefined",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Symbol(_) => "symbol",
                    _ if self.callable(&v) => "function",
                    _ => "object",
                }
                .into(),
            )),
            _ => Err(ScriptError::Unsupported("unary operator")),
        }
    }
    fn eval_binary(&mut self, op: &str, a: &Expr, b: &Expr, env: Id) -> Result<Value> {
        let a = self.eval(a, env)?;
        self.protect(&a)?;
        if op == "&&" && !a.truthy() || op == "||" && a.truthy() {
            return Ok(a);
        }
        let b = self.eval(b, env)?;
        self.protect(&b)?;
        match op {
            "&&" | "||" => Ok(b),
            "===" | "!==" => Ok(Value::Bool(a.strict_equal(&b) ^ (op == "!=="))),
            "==" | "!=" => {
                let equal = if a.strict_equal(&b)
                    || matches!(
                        (&a, &b),
                        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null)
                    ) {
                    true
                } else if matches!(
                    (&a, &b),
                    (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_))
                ) || matches!(a, Value::Bool(_))
                    || matches!(b, Value::Bool(_))
                {
                    self.to_number(a)? == self.to_number(b)?
                } else {
                    false
                };
                Ok(Value::Bool(equal ^ (op == "!=")))
            }
            "in" => {
                let Value::Object(id) = b else {
                    return self.throw_error("TypeError", "in requires object");
                };
                let k = self.to_key(a)?;
                Ok(Value::Bool(self.has(id, &k)?))
            }
            "instanceof" => self.instanceof(a, b).map(Value::Bool),
            _ => {
                let a = self.to_primitive(a, false)?;
                self.protect(&a)?;
                let b = self.to_primitive(b, false)?;
                self.protect(&b)?;
                if op == "+" && (matches!(a, Value::String(_)) || matches!(b, Value::String(_))) {
                    return Ok(Value::String(self.to_string(a)? + &self.to_string(b)?));
                }
                if matches!(op, "<" | ">" | "<=" | ">=") {
                    if let (Value::String(a), Value::String(b)) = (&a, &b) {
                        let order = a.encode_utf16().cmp(b.encode_utf16());
                        return Ok(Value::Bool(match op {
                            "<" => order.is_lt(),
                            ">" => order.is_gt(),
                            "<=" => !order.is_gt(),
                            _ => !order.is_lt(),
                        }));
                    }
                }
                let a = self.to_number(a)?;
                let b = self.to_number(b)?;
                Ok(match op {
                    "+" => Value::number(a + b),
                    "-" => Value::number(a - b),
                    "*" => Value::number(a * b),
                    "/" => Value::number(a / b),
                    "%" => Value::number(a % b),
                    "<" => Value::Bool(a < b),
                    ">" => Value::Bool(a > b),
                    "<=" => Value::Bool(a <= b),
                    ">=" => Value::Bool(a >= b),
                    _ => return Err(ScriptError::Unsupported("binary operator")),
                })
            }
        }
    }
    fn instanceof(&mut self, value: Value, ctor: Value) -> Result<bool> {
        if !self.callable(&ctor) {
            return self.throw_error("TypeError", "instanceof requires callable");
        }
        if let Value::Object(id) = &ctor {
            if let Some(Object::Bound { target, .. }) = self.objects.get(id) {
                return self.instanceof(value, target.clone());
            }
        }
        let p = self.get_member(ctor, "prototype")?;
        let Value::Object(p) = p else {
            return self.throw_error("TypeError", "constructor prototype is not object");
        };
        let Value::Object(id) = value else {
            return Ok(false);
        };
        let mut at = self.data(id)?.prototype;
        for _ in 0..10_000 {
            match at {
                None => return Ok(false),
                Some(a) if a == p => return Ok(true),
                Some(a) => at = self.data(a)?.prototype,
            }
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn to_primitive(&mut self, v: Value, string: bool) -> Result<Value> {
        if !matches!(v, Value::Object(_)) {
            return Ok(v);
        }
        for name in if string {
            ["toString", "valueOf"]
        } else {
            ["valueOf", "toString"]
        } {
            let f = self.get_member(v.clone(), name)?;
            self.protect(&f)?;
            if self.callable(&f) {
                let r = self.invoke_this(f, v.clone(), vec![])?;
                if !matches!(r, Value::Object(_)) {
                    return Ok(r);
                }
            }
        }
        self.throw_error("TypeError", "cannot convert object to primitive")
    }
    pub(super) fn to_key(&mut self, value: Value) -> Result<Key> {
        let value = self.to_primitive(value, true)?;
        if let Value::Symbol(id) = value {
            Ok(Key::Symbol(id))
        } else {
            self.to_string(value).map(Key::String)
        }
    }
    pub(super) fn to_string(&mut self, v: Value) -> Result<String> {
        let v = self.to_primitive(v, true)?;
        Ok(match v {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => number_string(f64::from_bits(n)),
            Value::String(s) => s,
            Value::Symbol(_) => {
                return self.throw_error("TypeError", "cannot convert symbol to string")
            }
            _ => return Err(ScriptError::Unsupported("host primitive conversion")),
        })
    }
    pub(super) fn to_number(&mut self, v: Value) -> Result<f64> {
        let v = self.to_primitive(v, false)?;
        Ok(match v {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            Value::Number(n) => f64::from_bits(n),
            Value::Symbol(_) => {
                return self.throw_error("TypeError", "cannot convert symbol to number")
            }
            Value::String(s) => {
                let s =
                    s.trim_matches(|c: char| c == '\u{feff}' || c != '\u{85}' && c.is_whitespace());
                if s.is_empty() {
                    0.0
                } else if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    u64::from_str_radix(hex, 16)
                        .map(|n| n as f64)
                        .unwrap_or(f64::NAN)
                } else {
                    s.parse().unwrap_or(f64::NAN)
                }
            }
            _ => return Err(ScriptError::Unsupported("host numeric conversion")),
        })
    }
}
