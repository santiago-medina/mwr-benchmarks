//! Evaluation and completion propagation. Every allocating operation runs under a root frame.
use super::js_string::JsString;
use super::{
    objects::{Data, Descriptor, Key, Kind},
    parser::{Expr, Param, Pattern, Property as Literal, Stmt},
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
            Self::BigInt(n) => !n.is_zero(),
            _ => true,
        }
    }
    pub(super) fn same_value_zero(&self, b: &Self) -> bool {
        self.strict_equal(b)
            || matches!((self,b),(Self::Number(a),Self::Number(b))if f64::from_bits(*a).is_nan()&&f64::from_bits(*b).is_nan())
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
    fn temporal_constructor(name: &str) -> bool {
        #[cfg(feature = "temporal")]
        return super::temporal::is_constructor(name);
        #[cfg(not(feature = "temporal"))]
        {
            let _ = name;
            false
        }
    }
    pub(super) fn callable(&self, v: &Value) -> bool {
        matches!(v, Value::Native { .. })
            || matches!(v,Value::Object(id) if matches!(self.objects.get(id),Some(Object::Closure{..}|Object::Builtin{..}|Object::Bound{..}|Object::Proxy{callable:true,..})))
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
                        }) => s.lossy().into_owned(),
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
        params: &[Param],
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
            FunctionKind::Async
                | FunctionKind::AsyncArrow
                | FunctionKind::AsyncMethod
                | FunctionKind::Generator
                | FunctionKind::GeneratorMethod
        ) {
            Some(super::asynchronous::compile(body)?)
        } else if matches!(
            kind,
            FunctionKind::AsyncGenerator | FunctionKind::AsyncGeneratorMethod
        ) {
            Some(super::asynchronous::compile_kind(body, true)?)
        } else {
            None
        };
        self.objects.insert(
            id,
            Object::Closure {
                environment: env,
                params: std::rc::Rc::new(params.to_vec()),
                body: std::rc::Rc::new(body.to_vec()),
                needs_arguments: arguments_needed(params, body),
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
                    FunctionKind::Generator | FunctionKind::GeneratorMethod
                ) {
                    self.intrinsics.as_ref().unwrap().collection["GeneratorFunction"]
                } else if matches!(
                    kind,
                    FunctionKind::AsyncGenerator | FunctionKind::AsyncGeneratorMethod
                ) {
                    self.intrinsics.as_ref().unwrap().collection["AsyncGeneratorFunction"]
                } else if matches!(
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
                value: Some(Value::number(
                    params
                        .iter()
                        .take_while(|p| p.default.is_none() && !p.rest)
                        .count() as f64,
                )),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        if matches!(
            kind,
            FunctionKind::Generator
                | FunctionKind::GeneratorMethod
                | FunctionKind::AsyncGenerator
                | FunctionKind::AsyncGeneratorMethod
        ) {
            let prototype = self.intrinsics.as_ref().unwrap().collection[if matches!(
                kind,
                FunctionKind::Generator | FunctionKind::GeneratorMethod
            ) {
                "Generator"
            } else {
                "AsyncGenerator"
            }];
            let prototype = self.new_object(Some(prototype))?;
            self.protect(&Value::Object(prototype))?;
            self.define(
                id,
                "prototype",
                Descriptor {
                    value: Some(Value::Object(prototype)),
                    writable: Some(true),
                    ..Descriptor::default()
                },
            )?;
        }
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
        for s in body.iter().map(super::parser::strip_labels) {
            if let Stmt::Function(n, p, b)
            | Stmt::AsyncFunction(n, p, b)
            | Stmt::GeneratorFunction(n, p, b, _) = s
            {
                let kind = if matches!(s, Stmt::GeneratorFunction(.., true)) {
                    FunctionKind::AsyncGenerator
                } else if matches!(s, Stmt::GeneratorFunction(..)) {
                    FunctionKind::Generator
                } else if matches!(s, Stmt::AsyncFunction(..)) {
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
        self.using_scope(body, env, |e| {
            for s in body {
                e.tick()?;
                let completion = e.frame(|e| e.statement(s, env, can_return))?;
                if completion.is_some() {
                    return Ok(completion);
                }
            }
            Ok(None)
        })
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
        if self.completion.is_some()
            && matches!(
                s,
                Stmt::If(..)
                    | Stmt::While { .. }
                    | Stmt::For { .. }
                    | Stmt::ForOf { .. }
                    | Stmt::ForIn { .. }
                    | Stmt::Switch(..)
                    | Stmt::With(..)
                    | Stmt::Try { .. }
            )
        {
            // These statements complete with UpdateEmpty(result, undefined).
            self.set_completion(Value::Undefined)?;
        }
        match s {
            Stmt::Sloppy => {}
            Stmt::With(object, body) => {
                let object = self.eval(object, env)?;
                self.protect(&object)?;
                let object = self.object_coercible(object)?;
                let scope = self.with_environment(env, object)?;
                self.protect(&Value::Object(scope))?;
                return self.statement(body, scope, can_return);
            }
            Stmt::Declarations(items) => {
                for item in items {
                    self.statement(item, env, can_return)?;
                }
            }
            Stmt::Bind {
                kind,
                pattern,
                init,
            } => {
                if kind == "await using" {
                    return Err(ScriptError::Unsupported(
                        "await using in a synchronous body",
                    ));
                }
                if let Some(init) = init {
                    let value = if let super::parser::Pattern::Name(n) = pattern {
                        self.eval_named(init, env, n)?
                    } else {
                        self.eval(init, env)?
                    };
                    self.protect(&value)?;
                    let target = if kind == "var" {
                        self.var_environment(env)?
                    } else {
                        env
                    };
                    self.bind_pattern(pattern, value.clone(), env, target)?;
                    if kind == "using" {
                        // §14.3.1.2: AddDisposableResource(scope, value, sync-dispose).
                        let scope = self.lookup(env, "%dispose")?;
                        self.add_using_resource(&scope, value, false)?;
                    }
                }
            }
            Stmt::Empty => {}
            Stmt::Switch(value, cases) => {
                return self.switch_statement(value, cases, env, can_return)
            }
            Stmt::While { test, body, post } => {
                return self.while_loop(test, body, *post, env, can_return)
            }
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                return self.for_loop(
                    init.as_deref(),
                    test.as_ref(),
                    update.as_ref(),
                    body,
                    env,
                    can_return,
                )
            }

            Stmt::Const(n, x) | Stmt::Let(n, x) => {
                let v = self.eval_named(x, env, n)?;
                self.initialize(env, n, v)?;
            }
            Stmt::Var(n, x) => {
                if let Some(x) = x {
                    let v = self.eval_named(x, env, n)?;
                    let local = self.var_environment(env)?;
                    self.initialize(local, n, v)?;
                }
            }
            Stmt::Break(None) => return Err(ScriptError::LoopBreak),
            Stmt::Break(Some(label)) => return Err(ScriptError::LabelBreak(label.clone())),
            Stmt::Labeled(label, body) => {
                return match self.statement(body, env, can_return) {
                    Err(ScriptError::LabelBreak(target)) if target == *label => Ok(None),
                    other => other,
                };
            }
            Stmt::Continue => return Err(ScriptError::LoopContinue),
            Stmt::ForOf {
                asynchronous: true, ..
            } => {
                return Err(ScriptError::Unsupported(
                    "for-await requires an async function",
                ))
            }
            Stmt::ForOf {
                kind,
                binding,
                iterable,
                body,
                ..
            } => return self.for_of(kind, binding, iterable, body, env, can_return, false),
            Stmt::ForIn {
                kind,
                binding,
                iterable,
                body,
            } => return self.for_of(kind, binding, iterable, body, env, can_return, true),
            Stmt::Function(..) | Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(..) => {}
            Stmt::Expr(x) => {
                let value = self.eval(x, env)?;
                self.set_completion(value)?;
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
                let strict = self.env_strict(env);
                self.assign_member(a, k.into(), b.clone(), strict)?;
                self.set_completion(b)?;
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
                if let (Err(ScriptError::Thrown(ex)), Some((pattern, b))) = (&pending, catch) {
                    let value = ex.value.clone();
                    self.protect(&value)?;
                    pending = (|| {
                        let declarations = pattern
                            .iter()
                            .map(|pattern| Stmt::Bind {
                                kind: "let".into(),
                                pattern: pattern.clone(),
                                init: Some(Expr::Undefined),
                            })
                            .collect::<Vec<_>>();
                        let local = self.new_environment(Some(env), &declarations, false)?;
                        self.protect(&Value::Object(local))?;
                        if let Some(pattern) = pattern {
                            self.bind_pattern(pattern, value, local, local)?;
                        }
                        self.scoped(b, local, can_return)
                    })();
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
                    // A normally completing finally block keeps the try/catch value.
                    let kept = self.completion_value();
                    if let Some(v) = &kept {
                        self.protect(v)?;
                    }
                    match self.scoped(b, env, can_return) {
                        Ok(None) => {
                            if let Some(v) = kept {
                                self.set_completion(v)?;
                            }
                        }
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
        binding: &Pattern,
        source: &Expr,
        body: &Stmt,
        env: Id,
        can_return: bool,
        enumeration: bool,
    ) -> Result<Option<Value>> {
        let scope = if kind == "var" || kind == "assignment" {
            env
        } else {
            let declaration = Stmt::Bind {
                kind: "const".into(),
                pattern: binding.clone(),
                init: Some(Expr::Undefined),
            };
            let id = self.new_environment(Some(env), &[declaration], false)?;
            self.protect(&Value::Object(id))?;
            id
        };
        let value = self.eval(source, scope)?;
        self.protect(&value)?;
        let (iterator, next) = if enumeration {
            (
                self.enumeration(value)?,
                Value::Object(
                    self.intrinsics.as_ref().unwrap().functions["%ForInIteratorPrototype%.next"],
                ),
            )
        } else {
            let method = self.get_member(value.clone(), Key::Symbol(0))?;
            self.protect(&method)?;
            self.open_iterator(value, method)?
        };
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
                if kind == "await using" {
                    return Err(ScriptError::Unsupported(
                        "await using in a synchronous loop",
                    ));
                }
                let pending = e.frame(|e| {
                    let local = if kind == "var" || kind == "assignment" {
                        env
                    } else {
                        let declaration = Stmt::Bind {
                            kind: kind.into(),
                            pattern: binding.clone(),
                            init: Some(Expr::Undefined),
                        };
                        let id = e.new_environment(Some(scope), &[declaration], false)?;
                        e.protect(&Value::Object(id))?;
                        id
                    };
                    if kind == "assignment" {
                        e.assign_pattern(binding, value.clone(), local)?;
                    } else {
                        let target = if kind == "var" {
                            e.var_environment(env)?
                        } else {
                            local
                        };
                        e.bind_pattern(binding, value.clone(), local, target)?;
                    }
                    if kind != "using" {
                        return e.statement(body, local, can_return);
                    }
                    // §14.7.5.7 ForIn/OfBodyEvaluation: each iteration's using binding is
                    // disposed before the next step (and before IteratorClose).
                    let scope = e.new_using_scope()?;
                    e.initialize(local, "%dispose", scope.clone())?;
                    let out = e
                        .add_using_resource(&scope, value, false)
                        .and_then(|_| e.statement(body, local, can_return));
                    e.dispose_scope_outcome(&scope, out)
                });
                match pending {
                    Ok(None) | Err(ScriptError::LoopContinue) => Ok((false, None)),
                    Err(ScriptError::LoopBreak) if enumeration => Ok((true, None)),
                    other if enumeration => other.map(|v| (true, v)),
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
    pub(super) fn close_iterator(
        &mut self,
        iterator: Value,
        pending: Result<Option<Value>>,
    ) -> Result<Option<Value>> {
        match &pending {
            Ok(Some(v)) => self.protect(v)?,
            Err(ScriptError::Thrown(e)) => self.protect(&e.value)?,
            Err(ScriptError::LabelBreak(_)) => {}
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
            Expr::Template(t) => self.eval_template(t, env),
            Expr::Class(c) => self.eval_class(c, env, None),
            Expr::PrivateName(n) => self.lookup(env, n),
            Expr::NewTarget => self.lookup(env, "%newtarget"),
            // §13.3.10.1 step 2: the specifier's evaluation throws; ToString rejects.
            Expr::ImportCall(specifier, phase) => {
                let specifier = self.eval(specifier, env)?;
                self.import_call(env, specifier, *phase)
            }
            Expr::ImportMeta => self.import_meta(env),
            Expr::ClassInit(init) => self.eval_class_init(init, env),
            Expr::SuperCall(args) => self.super_call(args, env),
            Expr::SuperMember(_) | Expr::SuperComputed(_) => {
                let (this, target, key) = self.super_key(x, env)?;
                self.super_get(this, target, &key)
            }
            Expr::Name(n) => self.lookup(env, n),
            Expr::This => self.lookup(env, "this"),
            Expr::String(s) => Ok(Value::String(s.clone().into())),
            Expr::Units(u) => Ok(Value::String(JsString::from_units(u))),
            Expr::BigInt(s) => self.big_literal(s),
            Expr::RegExp { pattern, flags } => self.regexp_literal(pattern, flags),
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
            Expr::Yield(..) => Err(ScriptError::Unsupported(
                "yield requires a generator activation",
            )),
            Expr::GeneratorFunction {
                name,
                params,
                body,
                method,
                asynchronous,
            } => self.make_function(
                env,
                name.as_deref(),
                params,
                body,
                match (*method, *asynchronous) {
                    (true, false) => FunctionKind::GeneratorMethod,
                    (false, false) => FunctionKind::Generator,
                    (true, true) => FunctionKind::AsyncGeneratorMethod,
                    (false, true) => FunctionKind::AsyncGenerator,
                },
                name.is_some(),
            ),
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
            Expr::Array(elements) => self.array_literal(elements, env),
            Expr::Spread(_) => Err(ScriptError::Runtime("spread outside accumulation")),
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
            // §13.3.9.1 OptionalExpression evaluation.
            Expr::OptionalChain(chain) => match self.eval(chain, env) {
                Err(ScriptError::ShortCircuit) => Ok(Value::Undefined),
                other => other,
            },
            Expr::Optional(base) => {
                let value = self.eval(base, env)?;
                if matches!(value, Value::Undefined | Value::Null) {
                    return Err(ScriptError::ShortCircuit);
                }
                Ok(value)
            }
            Expr::Unary(op, a) => self.eval_unary(op, a, env),
            Expr::Binary(op, a, b) => self.eval_binary(op, a, b, env),
            Expr::Conditional(condition, yes, no) => {
                let c = self.eval(condition, env)?;
                self.eval(if c.truthy() { yes } else { no }, env)
            }
            Expr::Compound(op, target, rhs) => self.compound(op, target, rhs, env),
            Expr::Update(target, increment, prefix) => {
                self.update(target, *increment, *prefix, env)
            }
            Expr::Sequence(items) => {
                let mut value = Value::Undefined;
                for item in items {
                    value = self.frame(|e| e.eval(item, env))?;
                }
                Ok(value)
            }
            Expr::Destructure(pattern, rhs) => {
                let value = self.eval(rhs, env)?;
                self.protect(&value)?;
                self.assign_pattern(pattern, value.clone(), env)?;
                Ok(value)
            }
            Expr::Assign(a, b) => {
                let reference = self.eval_reference(a, env)?;
                let value = if let Expr::Name(n) = &**a {
                    self.eval_named(b, env, n)?
                } else {
                    self.eval(b, env)?
                };
                self.reference_set(reference, value)
            }
        }
    }
    pub(super) fn member_key(&mut self, receiver: &Value, key: Value) -> Result<Key> {
        if matches!(receiver, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "nullish property receiver");
        }
        self.to_key(key)
    }
    fn eval_object(&mut self, props: &[(String, Literal)], env: Id) -> Result<Value> {
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        let env = if props.iter().any(|(_, p)| literal_method(p)) {
            let home = self.new_environment(Some(env), &[], false)?;
            self.protect(&Value::Object(home))?;
            self.initialize(home, "%home", Value::Object(id))?;
            home
        } else {
            env
        };
        for (k, p) in props {
            self.literal_property(id, k, p, env)?;
        }
        Ok(Value::Object(id))
    }
    pub(super) fn literal_property(&mut self, id: Id, k: &str, p: &Literal, env: Id) -> Result<()> {
        self.literal_property_key(id, Key::from(k), p, env)
    }
    fn literal_property_key(&mut self, id: Id, key: Key, p: &Literal, env: Id) -> Result<()> {
        let display = match &key {
            Key::String(name) => name.clone(),
            Key::Symbol(id) => self
                .symbol_description(*id)
                .map(|s| format!("[{s}]"))
                .unwrap_or_default(),
        };
        let k = display.as_str();
        let d = match p {
            Literal::CoverDefault(..) => {
                return Err(ScriptError::Unsupported(
                    "unconverted initialized shorthand",
                ))
            }
            Literal::Computed(expression, property) => {
                let value = self.eval(expression, env)?;
                self.protect(&value)?;
                let key = self.to_key(value)?;
                return self.literal_property_key(id, key, property, env);
            }
            Literal::Spread(expression) => {
                let source = self.eval(expression, env)?;
                return self.copy_properties(id, source, &[]);
            }

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
                let v = self.eval_named(x, env, &display)?;
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
                let f = self.make_function(
                    env,
                    Some(&format!("get {k}")),
                    &[],
                    b,
                    FunctionKind::Method,
                    false,
                )?;
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
                    Some(&format!("set {k}")),
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
        self.define(id, key, d)?;
        Ok(())
    }
    fn eval_call(&mut self, f: &Expr, args: &[Expr], env: Id, new: bool) -> Result<Value> {
        let (receiver, callee) = if new {
            (Value::Undefined, self.eval(f, env)?)
        } else {
            self.call_target(f, env)?
        };
        self.protect(&receiver)?;
        self.protect(&callee)?;
        let values = if args.iter().any(|e| matches!(e, Expr::Spread(_))) {
            self.spread_arguments(args, env)?
        } else {
            let mut values = vec![];
            for expression in args {
                let value = self.eval(expression, env)?;
                self.protect(&value)?;
                values.push(value);
            }
            values
        };
        if !new && matches!(f, Expr::Name(n) if n == "eval") && self.is_intrinsic_eval(&callee) {
            let source = values.into_iter().next().unwrap_or(Value::Undefined);
            return self.perform_eval(source, Some(env));
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
        // Function bodies do not contribute to an enclosing eval's completion value.
        let completion = self.completion.take();
        let r = self.frame(|e| {
            e.protect(&f)?;
            e.protect(&receiver)?;
            for a in &args {
                e.protect(a)?;
            }
            e.invoke_inner(f, receiver, args)
        });
        self.call_depth -= 1;
        self.completion = completion;
        match r {
            Err(ScriptError::Host(message)) if self.adapter_error.is_none() => {
                self.throw_error("Error", &message)
            }
            other => other,
        }
    }
    /// Copy out only what a call needs: cloning the whole object copied its property
    /// map on every call (BYTECODE_VM.md, stage 1). Kept out of line so its temporaries
    /// stay off the recursive `invoke_inner` frame (debug builds do not share slots).
    #[inline(never)]
    fn callee(&self, id: Id) -> Option<Object> {
        match self.objects.get(&id) {
            Some(Object::Builtin { name, data }) => Some(Object::Builtin {
                name: name.clone(),
                data: Data {
                    internal: data.internal.clone(),
                    ..Data::new(None)
                },
            }),
            Some(Object::Closure {
                kind,
                program,
                environment,
                params,
                body,
                needs_arguments,
                ..
            }) => Some(Object::Closure {
                kind: *kind,
                program: program.clone(),
                environment: *environment,
                params: params.clone(),
                body: body.clone(),
                needs_arguments: *needs_arguments,
                data: Data::new(None),
            }),
            other => other.cloned(),
        }
    }
    fn invoke_inner(&mut self, f: Value, receiver: Value, args: Vec<Value>) -> Result<Value> {
        if let Value::Native { receiver, name } = f {
            return self.call_native(receiver, &name, args);
        }
        let Value::Object(id) = f else {
            return self.throw_error("TypeError", "value is not callable");
        };
        let callee = self.callee(id);
        match callee {
            Some(Object::Proxy { .. }) => self.proxy_call(id, receiver, args),
            Some(Object::Builtin { name, data }) => {
                if name == "%ProxyRevoke%" {
                    return self.proxy_revoke(id);
                }
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
                if matches!(
                    kind,
                    FunctionKind::Generator
                        | FunctionKind::GeneratorMethod
                        | FunctionKind::AsyncGenerator
                        | FunctionKind::AsyncGeneratorMethod
                ) {
                    return self.start_generator(
                        id,
                        environment,
                        params,
                        body,
                        kind,
                        receiver,
                        args,
                        program.unwrap(),
                    );
                }
                if let Some(program) = program {
                    return self.start_async(
                        id,
                        environment,
                        params,
                        body,
                        kind,
                        receiver,
                        args,
                        program,
                    );
                }
                let class = matches!(
                    kind,
                    FunctionKind::ClassConstructor | FunctionKind::DerivedConstructor
                );
                if class && self.pending_construct.is_none() {
                    return self.throw_error("TypeError", "class constructor requires new");
                }
                let local = self.function_environment(
                    id,
                    environment,
                    &params,
                    &body,
                    kind,
                    receiver,
                    &args,
                )?;
                let result = self.block(&body, local, true)?;
                if kind == FunctionKind::DerivedConstructor {
                    return match result {
                        Some(v @ Value::Object(_)) => Ok(v),
                        Some(Value::Undefined) | None => self.lookup(local, "this"),
                        Some(_) => self
                            .throw_error("TypeError", "derived constructor returned a non-object"),
                    };
                }
                Ok(result.unwrap_or(Value::Undefined))
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
                kind:
                    FunctionKind::Ordinary
                    | FunctionKind::ClassConstructor
                    | FunctionKind::DerivedConstructor,
                ..
            }) => true,
            Some(Object::Bound { target, .. }) => self.constructible(target),
            Some(Object::Proxy { constructor, .. }) => *constructor,
            Some(Object::Builtin { name, .. }) => {
                super::typed_arrays::kind(name).is_some()
                    || name == "%TypedArray%"
                    || name == "%AbstractModuleSource%"
                    || Self::temporal_constructor(name)
                    || super::disposal::is_constructor(name)
                    || matches!(
                        name.as_str(),
                        "Promise"
                            | "Iterator"
                            | "Proxy"
                            | "ArrayBuffer"
                            | "SharedArrayBuffer"
                            | "DataView"
                            | "Array"
                            | "Map"
                            | "Set"
                            | "WeakMap"
                            | "WeakSet"
                            | "WeakRef"
                            | "FinalizationRegistry"
                            | "Object"
                            | "Function"
                            | "AsyncFunction"
                            | "GeneratorFunction"
                            | "AsyncGeneratorFunction"
                            | "Date"
                            | "RegExp"
                            | "String"
                            | "Number"
                            | "Boolean"
                    )
                    || self
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
            Some(Object::Proxy {
                constructor: true, ..
            }) => self.proxy_construct(id, args, new_target),
            Some(Object::Builtin { name, .. }) => {
                if name == "Proxy" {
                    let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
                    return self.proxy_create(arg(0), arg(1)).map(Value::Object);
                }
                if name == "Iterator" {
                    return self.construct_iterator(f, new_target);
                }
                if super::disposal::is_constructor(&name) {
                    return self.construct_disposal(&name, new_target);
                }
                if name == "Date" {
                    return self.construct_date(new_target, args);
                }
                #[cfg(feature = "temporal")]
                if super::temporal::is_constructor(&name) {
                    return self.construct_temporal(&name, new_target, args);
                }
                if name == "RegExp" {
                    return self.construct_regexp(new_target, args, false);
                }
                if name == "GeneratorFunction" || name == "AsyncGeneratorFunction" {
                    return self.dynamic_function_kind(
                        if name == "GeneratorFunction" {
                            FunctionKind::Generator
                        } else {
                            FunctionKind::AsyncGenerator
                        },
                        args,
                        Some(new_target),
                    );
                }
                if name == "%AbstractModuleSource%" {
                    return self
                        .throw_error("TypeError", "AbstractModuleSource is an abstract class");
                }
                if name == "%TypedArray%" {
                    return self.throw_error("TypeError", "abstract TypedArray constructor");
                }
                if super::typed_arrays::kind(&name).is_some() {
                    return self.construct_typed(&name, new_target, args);
                }

                if matches!(
                    name.as_str(),
                    "ArrayBuffer" | "SharedArrayBuffer" | "DataView"
                ) {
                    return self.construct_buffer(&name, new_target, args);
                }

                if self.intrinsics.as_ref().unwrap().errors.contains_key(&name) {
                    return self.error_constructor(&name, args, Some(new_target));
                }
                if name == "WeakRef" || name == "FinalizationRegistry" {
                    return self.construct_weak(&name, new_target, args);
                }
                if matches!(name.as_str(), "Map" | "Set" | "WeakMap" | "WeakSet") {
                    return self.construct_collection(
                        &name,
                        new_target,
                        args.first().cloned().unwrap_or(Value::Undefined),
                    );
                }
                if name == "Promise" {
                    return self.promise_constructor_target(
                        new_target,
                        args.first().cloned().unwrap_or(Value::Undefined),
                    );
                }
                if name == "Function" || name == "AsyncFunction" {
                    return self.dynamic_function(name == "AsyncFunction", args, Some(new_target));
                }
                if new_target != f
                    && matches!(
                        name.as_str(),
                        "Object" | "Array" | "String" | "Number" | "Boolean"
                    )
                {
                    let p = self.get_member(new_target.clone(), "prototype")?;
                    self.protect(&p)?;
                    if name == "Object" {
                        let p = match p {
                            Value::Object(p) => p,
                            _ => {
                                self.function_realm_check(&new_target)?;
                                self.intrinsics.as_ref().unwrap().object
                            }
                        };
                        return self.new_object(Some(p)).map(Value::Object);
                    }
                    let v = if name == "Array" {
                        self.builtin(&name, Value::Undefined, args)?
                    } else {
                        let v = self.builtin(&name, Value::Undefined, args)?;
                        Value::Object(self.box_value(v)?)
                    };
                    self.protect(&v)?;
                    if let (Value::Object(id), Value::Object(p)) = (&v, p) {
                        self.set_prototype(*id, Some(p))?;
                    }
                    return Ok(v);
                }
                if new_target != f {
                    return Err(ScriptError::Unsupported("custom newTarget for builtin"));
                }
                if matches!(name.as_str(), "String" | "Number" | "Boolean") {
                    let v = self.builtin(&name, Value::Undefined, args)?;
                    self.box_value(v).map(Value::Object)
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
                let p = self.get_member(new_target.clone(), "prototype")?;
                let p = if let Value::Object(p) = p {
                    p
                } else {
                    // §10.1.14 GetPrototypeFromConstructor step 3.a: GetFunctionRealm(constructor).
                    self.function_realm_check(&new_target)?;
                    self.intrinsics.as_ref().unwrap().object
                };
                let id = self.new_object(Some(p))?;
                let receiver = Value::Object(id);
                self.protect(&receiver)?;
                self.pending_construct = Some((new_target, f.clone()));
                let result = self.invoke_this(f, receiver.clone(), args);
                self.pending_construct = None;
                let result = result?;
                Ok(if matches!(result, Value::Object(_)) {
                    result
                } else {
                    receiver
                })
            }
            Some(Object::Closure {
                kind: kind @ (FunctionKind::ClassConstructor | FunctionKind::DerivedConstructor),
                ..
            }) => self.construct_class(
                f,
                args,
                new_target,
                kind == FunctionKind::DerivedConstructor,
            ),
            _ => self.throw_error("TypeError", "not a constructor"),
        }
    }
    fn eval_unary(&mut self, op: &str, x: &Expr, env: Id) -> Result<Value> {
        if op == "delete" && matches!(x, Expr::SuperMember(_) | Expr::SuperComputed(_)) {
            self.lookup(env, "this")?;
            if let Expr::SuperComputed(k) = x {
                self.eval(k, env)?;
            }
            return self.throw_error("ReferenceError", "delete of a super property");
        }
        // §13.5.1.2: `delete a?.b` is true when the chain short-circuits.
        if let Expr::OptionalChain(chain) = x {
            return match self.eval_unary(op, chain, env) {
                Err(ScriptError::ShortCircuit) if op == "delete" => Ok(Value::Bool(true)),
                Err(ScriptError::ShortCircuit) => self.eval_unary(op, &Expr::Undefined, env),
                other => other,
            };
        }
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
                Expr::Name(name) => return Ok(Value::Bool(self.delete_binding(env, name)?)),
                _ => {
                    self.eval(x, env)?;
                    None
                }
            };
            if let Some((Value::Object(id), k)) = target {
                if !self.delete(id, &k)? {
                    if !self.env_strict(env) {
                        return Ok(Value::Bool(false));
                    }
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
                        && ex.message.ends_with(" is not defined") =>
                {
                    Value::Undefined
                }
                r => r?,
            }
        } else {
            self.eval(x, env)?
        };
        self.protect(&v)?;
        let v = if matches!(op, "-" | "~") {
            self.to_primitive(v, false)?
        } else {
            v
        };
        if let Value::BigInt(value) = &v {
            match op {
                "-" => return self.big_value(value.neg()),
                "~" => return self.big_value(value.not()),
                _ => {}
            }
        }
        match op {
            "!" => Ok(Value::Bool(!v.truthy())),
            "~" => Ok(Value::number(
                (!(super::numeric::uint32(self.to_number(v)?) as i32)) as f64,
            )),
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
                    Value::BigInt(_) => "bigint",
                    _ if self.callable(&v) => "function",
                    _ => "object",
                }
                .into(),
            )),
            _ => Err(ScriptError::Unsupported("unary operator")),
        }
    }
    fn eval_binary(&mut self, op: &str, a: &Expr, b: &Expr, env: Id) -> Result<Value> {
        if let ("in", Expr::PrivateName(n)) = (op, a) {
            let object = self.eval(b, env)?;
            self.protect(&object)?;
            return self.private_in(n, object, env);
        }
        let a = self.eval(a, env)?;
        self.protect(&a)?;
        if super::references::assignment_skips(op, &a) {
            return Ok(a);
        }
        let b = self.eval(b, env)?;
        self.protect(&b)?;
        self.binary_values(op, a, b)
    }
    pub(super) fn binary_values(&mut self, op: &str, a: Value, b: Value) -> Result<Value> {
        match op {
            "&&" | "||" | "??" => Ok(b),
            "===" | "!==" => Ok(Value::Bool(a.strict_equal(&b) ^ (op == "!=="))),
            "==" | "!=" => Ok(Value::Bool(self.abstract_equal(a, b)? ^ (op == "!="))),
            "in" => {
                let Value::Object(id) = b else {
                    return self.throw_error("TypeError", "in requires object");
                };
                let k = self.to_key(a)?;
                Ok(Value::Bool(self.has(id, &k)?))
            }
            "instanceof" => self.instanceof(a, b).map(Value::Bool),
            _ => {
                let hint = if op == "+" { "default" } else { "number" };
                let a = self.to_primitive_hint(a, hint)?;
                self.protect(&a)?;
                let b = self.to_primitive_hint(b, hint)?;
                self.protect(&b)?;
                if op == "+" && (matches!(a, Value::String(_)) || matches!(b, Value::String(_))) {
                    // §13.15.3 step 1.b.iii: the concatenation of the code units.
                    let a = self.to_js_string(a)?;
                    return Ok(Value::String(a.concat(&self.to_js_string(b)?)));
                }
                if matches!(op, "<" | ">" | "<=" | ">=") {
                    if let (Value::String(a), Value::String(b)) = (&a, &b) {
                        let order = a.units().cmp(&b.units());
                        return Ok(Value::Bool(match op {
                            "<" => order.is_lt(),
                            ">" => order.is_gt(),
                            "<=" => !order.is_gt(),
                            _ => !order.is_lt(),
                        }));
                    }
                }
                if matches!(a, Value::BigInt(_)) || matches!(b, Value::BigInt(_)) {
                    if matches!(op, "<" | ">" | "<=" | ">=") {
                        let order = self.big_order(a, b)?;
                        return Ok(Value::Bool(match (op, order) {
                            ("<", Some(order)) => order.is_lt(),
                            (">", Some(order)) => order.is_gt(),
                            ("<=", Some(order)) => !order.is_gt(),
                            (">=", Some(order)) => !order.is_lt(),
                            _ => false,
                        }));
                    }
                    let (Value::BigInt(a), Value::BigInt(b)) = (a, b) else {
                        return self.throw_error("TypeError", "cannot mix BigInt and Number");
                    };
                    return self.big_binary(op, a, b);
                }
                let a = self.to_number(a)?;
                let b = self.to_number(b)?;
                Ok(match op {
                    "+" => Value::number(a + b),
                    "-" => Value::number(a - b),
                    "*" => Value::number(a * b),
                    "/" => Value::number(a / b),
                    "%" => Value::number(a % b),
                    "**" => Value::number(super::numeric::pow(a, b)),
                    "&" => Value::number(
                        (super::numeric::uint32(a) as i32 & super::numeric::uint32(b) as i32)
                            as f64,
                    ),
                    "|" => Value::number(
                        (super::numeric::uint32(a) as i32 | super::numeric::uint32(b) as i32)
                            as f64,
                    ),
                    "^" => Value::number(
                        (super::numeric::uint32(a) as i32 ^ super::numeric::uint32(b) as i32)
                            as f64,
                    ),
                    "<<" => Value::number(
                        (super::numeric::uint32(a) as i32)
                            .wrapping_shl(super::numeric::uint32(b) & 31)
                            as f64,
                    ),
                    ">>" => Value::number(
                        ((super::numeric::uint32(a) as i32) >> (super::numeric::uint32(b) & 31))
                            as f64,
                    ),
                    ">>>" => Value::number(
                        (super::numeric::uint32(a) >> (super::numeric::uint32(b) & 31)) as f64,
                    ),
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
        if !matches!(ctor, Value::Object(_)) {
            return self.throw_error("TypeError", "instanceof requires an object");
        }
        let method = self.get_member(ctor.clone(), Key::Symbol(super::symbols::HAS_INSTANCE))?;
        self.protect(&method)?;
        if !matches!(method, Value::Null | Value::Undefined) {
            if !self.callable(&method) {
                return self.throw_error("TypeError", "Symbol.hasInstance is not callable");
            }
            return self
                .invoke_this(method, ctor, vec![value])
                .map(|v| v.truthy());
        }
        if !self.callable(&ctor) {
            return self.throw_error("TypeError", "instanceof requires callable");
        }
        self.ordinary_has_instance(value, ctor)
    }
    pub(super) fn ordinary_has_instance(&mut self, value: Value, ctor: Value) -> Result<bool> {
        if !self.callable(&ctor) {
            return Ok(false);
        }
        if let Value::Object(id) = &ctor {
            if let Some(Object::Bound { target, .. }) = self.objects.get(id) {
                return self.instanceof(value, target.clone());
            }
        }
        // §7.3.21 OrdinaryHasInstance step 3: a primitive O is false before C.prototype is read.
        let Value::Object(id) = value else {
            return Ok(false);
        };
        let p = self.get_member(ctor, "prototype")?;
        let Value::Object(p) = p else {
            return self.throw_error("TypeError", "constructor prototype is not object");
        };
        // OrdinaryHasInstance step 6 walks [[GetPrototypeOf]] (proxies run their trap).
        let mut at = self.get_prototype_of(id)?;
        for _ in 0..10_000 {
            match at {
                None => return Ok(false),
                Some(a) if a == p => return Ok(true),
                Some(a) => at = self.get_prototype_of(a)?,
            }
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn to_primitive(&mut self, v: Value, string: bool) -> Result<Value> {
        self.to_primitive_hint(v, if string { "string" } else { "number" })
    }
    pub(super) fn to_primitive_hint(&mut self, v: Value, hint: &str) -> Result<Value> {
        if !matches!(v, Value::Object(_)) {
            return Ok(v);
        }
        self.protect(&v)?;
        let exotic = self.get_member(v.clone(), Key::Symbol(super::symbols::PRIMITIVE))?;
        self.protect(&exotic)?;
        if !matches!(exotic, Value::Null | Value::Undefined) {
            if !self.callable(&exotic) {
                return self.throw_error("TypeError", "Symbol.toPrimitive is not callable");
            }
            let value = self.invoke_this(exotic, v.clone(), vec![Value::String(hint.into())])?;
            if matches!(value, Value::Object(_)) {
                return self.throw_error("TypeError", "Symbol.toPrimitive returned an object");
            }
            return Ok(value);
        }
        self.ordinary_to_primitive(v, hint == "string")
    }
    pub(super) fn ordinary_to_primitive(&mut self, v: Value, string: bool) -> Result<Value> {
        self.protect(&v)?;
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
    /// ToString (§7.1.17) as scalar text; an ill-formed result reports Unsupported.
    /// Sites that handle lone surrogates call `to_js_string`.
    pub(super) fn to_string(&mut self, v: Value) -> Result<String> {
        Ok(self.to_js_string(v)?.text()?.to_owned())
    }
    /// ToString (§7.1.17).
    pub(super) fn to_js_string(&mut self, v: Value) -> Result<JsString> {
        let v = self.to_primitive(v, true)?;
        Ok(match v {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string().into(),
            Value::Number(n) => number_string(f64::from_bits(n)).into(),
            Value::String(s) => s,
            Value::BigInt(value) => self.big_string(&value, 10)?.into(),
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
            Value::BigInt(_) => {
                return self.throw_error("TypeError", "cannot implicitly convert BigInt to Number")
            }
            Value::Symbol(_) => {
                return self.throw_error("TypeError", "cannot convert symbol to number")
            }
            Value::String(s) => {
                // A lone surrogate is neither whitespace nor numeric (§7.1.4.1.1), so
                // the lossy U+FFFD stand-in yields the same NaN.
                let s = s.lossy();
                let s = s.trim_matches(super::strings::whitespace);
                if s.is_empty() {
                    0.0
                } else if ["0x", "0X", "0o", "0O", "0b", "0B"]
                    .iter()
                    .any(|prefix| s.starts_with(*prefix))
                {
                    self.big_parse(s)?
                        .map(|value| value.to_float())
                        .unwrap_or(f64::NAN)
                } else {
                    super::numeric::decimal_number(s)
                }
            }
            _ => return Err(ScriptError::Unsupported("host numeric conversion")),
        })
    }
}

fn literal_method(p: &Literal) -> bool {
    match p {
        Literal::Getter(_) | Literal::Setter(..) => true,
        Literal::Computed(_, p) => literal_method(p),
        Literal::Value(
            Expr::Function { method: true, .. }
            | Expr::GeneratorFunction { method: true, .. }
            | Expr::AsyncFunction { method: true, .. },
        ) => true,
        _ => false,
    }
}

/// Conservative argumentsObjectNeeded (§10.2.11 FunctionDeclarationInstantiation steps 15-18):
/// the arguments object is unobservable unless the parameters or body name `arguments`
/// (also through a nested arrow, `var arguments`, or the implicit derived constructor's
/// `%forward`) or contain a direct `eval` that could name it. Scanning the derived `Debug`
/// text covers every AST variant; any match (string literals included) keeps the eager
/// object. The scope-analysis pass of BYTECODE_VM.md stage 2 replaces this.
pub(super) fn arguments_needed(params: &[Param], body: &[Stmt]) -> bool {
    super::ast_scan::mentions(
        &[&params, &body],
        &["\"arguments\"", "\"eval\"", "%forward"],
    )
}
