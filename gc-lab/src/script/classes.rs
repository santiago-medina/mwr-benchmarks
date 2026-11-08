//! Class definitions, `super`, `new.target` and private names.
//! Private names are engine symbols listed in `private_symbols`: they never
//! appear in own-key enumeration and are only reached through `#x` syntax.
use super::{
    objects::{Descriptor, Key, Kind, Property},
    parser::{Class, ClassInit, ClassKey, Expr, MemberKind, Param, Stmt},
    vm::{Engine, FunctionKind, Object, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

fn key_value(key: &Key) -> Value {
    match key {
        Key::String(s) => Value::String(s.clone()),
        Key::Symbol(id) => Value::Symbol(*id),
    }
}

impl Engine<'_> {
    /// The value of a binding visible from `env`, or None when it is absent.
    pub(super) fn binding_opt(&self, mut env: Id, name: &str) -> Option<Value> {
        loop {
            let Some(Object::Environment {
                parent, bindings, ..
            }) = self.objects.get(&env)
            else {
                return None;
            };
            if let Some(v) = bindings.get(name) {
                return v.clone();
            }
            env = (*parent)?;
        }
    }
    fn define_or_throw(&mut self, id: Id, key: Key, d: Descriptor) -> Result<()> {
        if self.define(id, key, d)? {
            Ok(())
        } else {
            self.throw_error("TypeError", "class element definition refused")
        }
    }
    fn key_display(&self, key: &Key) -> String {
        match key {
            Key::String(s) => s.clone(),
            Key::Symbol(id) if self.private_symbols.contains(id) => {
                self.symbol_description(*id).unwrap_or_default()
            }
            Key::Symbol(id) => self
                .symbol_description(*id)
                .map(|s| format!("[{s}]"))
                .unwrap_or_default(),
        }
    }
    /// Evaluate `x`, naming an anonymous function or class after `name`.
    pub(super) fn eval_named(&mut self, x: &Expr, env: Id, name: &str) -> Result<Value> {
        if let Expr::Class(class) = x {
            if class.name.is_none() {
                return self.eval_class(class, env, Some(name));
            }
        }
        let value = self.eval(x, env)?;
        if x.anonymous_function() {
            if let Value::Object(function) = &value {
                self.protect(&value)?;
                self.define(
                    *function,
                    "name",
                    Descriptor {
                        value: Some(Value::String(name.into())),
                        ..Descriptor::default()
                    },
                )?;
            }
        }
        Ok(value)
    }
    fn class_key(&mut self, key: &ClassKey, scope: Id) -> Result<Key> {
        match key {
            ClassKey::Named(s) => Ok(Key::from(s.as_str())),
            ClassKey::Private(n) => {
                let v = self.lookup(scope, n)?;
                self.to_key(v)
            }
            ClassKey::Computed(e) => {
                let v = self.eval(e, scope)?;
                self.protect(&v)?;
                self.to_key(v)
            }
        }
    }
    fn method_function(&mut self, env: Id, name: &str, method: &Expr) -> Result<Value> {
        let (params, body, kind) = match method {
            Expr::Function { params, body, .. } => (params, body, FunctionKind::Method),
            Expr::GeneratorFunction { params, body, .. } => {
                (params, body, FunctionKind::GeneratorMethod)
            }
            Expr::AsyncFunction { params, body, .. } => (params, body, FunctionKind::AsyncMethod),
            _ => return Err(ScriptError::Runtime("invalid class method")),
        };
        self.make_function(env, Some(name), params, body, kind, false)
    }
    pub(super) fn eval_class(
        &mut self,
        class: &Class,
        env: Id,
        inferred: Option<&str>,
    ) -> Result<Value> {
        let declarations: Vec<_> = class
            .name
            .iter()
            .map(|n| Stmt::Const(n.clone(), Expr::Undefined))
            .collect();
        let scope = self.new_environment(Some(env), &declarations, false)?;
        self.protect(&Value::Object(scope))?;
        for member in &class.members {
            if let ClassKey::Private(n) = &member.key {
                if self.binding_opt(scope, n).is_none() {
                    let symbol = self.new_symbol(Some(n.clone()))?;
                    if let Value::Symbol(id) = symbol {
                        self.private_symbols.insert(id);
                    }
                    self.initialize(scope, n, symbol)?;
                }
            }
        }
        let intrinsics = self.intrinsics.as_ref().unwrap();
        let (object_proto, function_proto) = (intrinsics.object, intrinsics.function);
        let (proto_parent, ctor_parent) = match &class.heritage {
            None => (Some(object_proto), function_proto),
            Some(heritage) => {
                let parent = self.eval(heritage, scope)?;
                self.protect(&parent)?;
                match parent {
                    Value::Null => (None, function_proto),
                    Value::Object(p) if self.constructible(&parent) => {
                        let pp = self.get_member(parent.clone(), "prototype")?;
                        match pp {
                            Value::Object(pp) => (Some(pp), p),
                            Value::Null => (None, p),
                            _ => {
                                return self
                                    .throw_error("TypeError", "class heritage prototype invalid")
                            }
                        }
                    }
                    _ => return self.throw_error("TypeError", "class heritage not a constructor"),
                }
            }
        };
        let proto = self.new_object(proto_parent)?;
        self.protect(&Value::Object(proto))?;
        self.initialize(scope, "%home", Value::Object(proto))?;
        let derived = class.heritage.is_some();
        let forward = vec![Stmt::Expr(Expr::SuperCall(vec![Expr::Name(
            "%forward".into(),
        )]))];
        let (params, body): (&[Param], &[Stmt]) = match &class.constructor {
            Some((p, b)) => (p, b),
            None if derived => (&[], &forward),
            None => (&[], &[]),
        };
        let name = class.name.as_deref().or(inferred).unwrap_or("");
        let kind = if derived {
            FunctionKind::DerivedConstructor
        } else {
            FunctionKind::ClassConstructor
        };
        let f = self.make_function(scope, Some(name), params, body, kind, false)?;
        self.protect(&f)?;
        let Value::Object(fid) = f else {
            unreachable!()
        };
        self.set_prototype(fid, Some(ctor_parent))?;
        self.define_or_throw(
            fid,
            Key::from("prototype"),
            Descriptor {
                value: Some(Value::Object(proto)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..Descriptor::default()
            },
        )?;
        self.define_or_throw(
            proto,
            Key::from("constructor"),
            Descriptor {
                value: Some(f.clone()),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        let static_scope = self.new_environment(Some(scope), &[], false)?;
        self.protect(&Value::Object(static_scope))?;
        self.initialize(static_scope, "%home", f.clone())?;
        let mut instance: Vec<Stmt> = vec![];
        let mut instance_fields: Vec<Stmt> = vec![];
        let mut statics: Vec<(Option<Key>, &MemberKind)> = vec![];
        let mut static_private: Vec<(Key, Property)> = vec![];
        for (index, member) in class.members.iter().enumerate() {
            self.tick()?;
            let (target, menv) = if member.is_static {
                (fid, static_scope)
            } else {
                (proto, scope)
            };
            if let MemberKind::Block(_) = member.kind {
                statics.push((None, &member.kind));
                continue;
            }
            let key = self.class_key(&member.key, scope)?;
            let private = matches!(member.key, ClassKey::Private(_));
            let display = self.key_display(&key);
            let (function, slot) = match &member.kind {
                MemberKind::Field(init) => {
                    if member.is_static {
                        statics.push((Some(key), &member.kind));
                    } else {
                        let ckey = match &member.key {
                            ClassKey::Computed(_) => {
                                let binding = format!("%k{index}");
                                self.initialize(scope, &binding, key_value(&key))?;
                                ClassKey::Computed(Expr::Name(binding))
                            }
                            other => other.clone(),
                        };
                        instance_fields.push(Stmt::Expr(Expr::ClassInit(Box::new(
                            ClassInit::Field(ckey, init.clone()),
                        ))));
                    }
                    continue;
                }
                MemberKind::Block(_) => unreachable!(),
                MemberKind::Method(m) => (self.method_function(menv, &display, m)?, "%pm:"),
                MemberKind::Getter(b) => (
                    self.make_function(
                        menv,
                        Some(&format!("get {display}")),
                        &[],
                        b,
                        FunctionKind::Method,
                        false,
                    )?,
                    "%pg:",
                ),
                MemberKind::Setter(p, b) => (
                    self.make_function(
                        menv,
                        Some(&format!("set {display}")),
                        std::slice::from_ref(p),
                        b,
                        FunctionKind::Method,
                        false,
                    )?,
                    "%ps:",
                ),
            };
            self.protect(&function)?;
            if private {
                let ClassKey::Private(n) = &member.key else {
                    unreachable!()
                };
                if member.is_static {
                    let property = match slot {
                        "%pm:" => Property::data(function, false, false, false),
                        _ => {
                            let mut get = Value::Undefined;
                            let mut set = Value::Undefined;
                            if let Some((_, existing)) =
                                static_private.iter().find(|(k, _)| *k == key)
                            {
                                if let Kind::Accessor { get: g, set: s } = &existing.kind {
                                    get = g.clone();
                                    set = s.clone();
                                }
                            }
                            if slot == "%pg:" {
                                get = function;
                            } else {
                                set = function;
                            }
                            Property {
                                kind: Kind::Accessor { get, set },
                                enumerable: false,
                                configurable: false,
                            }
                        }
                    };
                    static_private.retain(|(k, _)| *k != key);
                    static_private.push((key, property));
                } else {
                    let binding = format!("{slot}{n}");
                    self.initialize(scope, &binding, function)?;
                    if !instance.iter().any(|s| {
                        matches!(s, Stmt::Expr(Expr::ClassInit(i)) if matches!(&**i, ClassInit::PrivateMethod(m) if m == n))
                    }) {
                        instance.push(Stmt::Expr(Expr::ClassInit(Box::new(
                            ClassInit::PrivateMethod(n.clone()),
                        ))));
                    }
                }
                continue;
            }
            let d = match slot {
                "%pm:" => Descriptor {
                    value: Some(function),
                    writable: Some(true),
                    enumerable: Some(false),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
                "%pg:" => Descriptor {
                    get: Some(function),
                    enumerable: Some(false),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
                _ => Descriptor {
                    set: Some(function),
                    enumerable: Some(false),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            };
            self.define_or_throw(target, key, d)?;
        }
        if let Some(n) = &class.name {
            self.initialize(scope, n, f.clone())?;
        }
        instance.extend(instance_fields);
        let fields = if instance.is_empty() {
            Value::Undefined
        } else {
            self.make_function(scope, None, &[], &instance, FunctionKind::Method, false)?
        };
        self.initialize(scope, "%fields", fields)?;
        for (key, property) in static_private {
            self.private_add(fid, key, property)?;
        }
        for (index, (key, kind)) in statics.into_iter().enumerate() {
            let body = match (key, kind) {
                (None, MemberKind::Block(b)) => b.clone(),
                (Some(key), MemberKind::Field(init)) => {
                    let ckey = if let Key::Symbol(id) = &key {
                        if self.private_symbols.contains(id) {
                            None
                        } else {
                            Some(key.clone())
                        }
                    } else {
                        Some(key.clone())
                    };
                    let ckey = match ckey {
                        None => {
                            let n = self.key_display(&key);
                            ClassKey::Private(n)
                        }
                        Some(k) => {
                            let binding = format!("%sk{index}");
                            self.initialize(static_scope, &binding, key_value(&k))?;
                            ClassKey::Computed(Expr::Name(binding))
                        }
                    };
                    vec![Stmt::Expr(Expr::ClassInit(Box::new(ClassInit::Field(
                        ckey,
                        init.clone(),
                    ))))]
                }
                _ => unreachable!(),
            };
            let init =
                self.make_function(static_scope, None, &[], &body, FunctionKind::Method, false)?;
            self.protect(&init)?;
            self.invoke_this(init, f.clone(), vec![])?;
        }
        Ok(f)
    }
    pub(super) fn eval_class_init(&mut self, init: &ClassInit, env: Id) -> Result<Value> {
        let this = self.lookup(env, "this")?;
        self.protect(&this)?;
        let Value::Object(target) = this else {
            return self.throw_error("TypeError", "class element receiver");
        };
        match init {
            ClassInit::Field(key, value) => {
                let key = match key {
                    ClassKey::Named(s) => Key::from(s.as_str()),
                    ClassKey::Private(n) => {
                        let v = self.lookup(env, n)?;
                        self.to_key(v)?
                    }
                    ClassKey::Computed(e) => {
                        let v = self.eval(e, env)?;
                        self.to_key(v)?
                    }
                };
                let display = self.key_display(&key);
                let v = match value {
                    Some(x) => self.eval_named(x, env, &display)?,
                    None => Value::Undefined,
                };
                self.protect(&v)?;
                if matches!(&key, Key::Symbol(id) if self.private_symbols.contains(id)) {
                    self.private_add(target, key, Property::data(v, true, false, false))?;
                } else {
                    self.define_or_throw(
                        target,
                        key,
                        Descriptor {
                            value: Some(v),
                            writable: Some(true),
                            enumerable: Some(true),
                            configurable: Some(true),
                            ..Descriptor::default()
                        },
                    )?;
                }
            }
            ClassInit::PrivateMethod(n) => {
                let symbol = self.lookup(env, n)?;
                let key = self.to_key(symbol)?;
                let property = if let Some(m) = self.binding_opt(env, &format!("%pm:{n}")) {
                    Property::data(m, false, false, false)
                } else {
                    Property {
                        kind: Kind::Accessor {
                            get: self
                                .binding_opt(env, &format!("%pg:{n}"))
                                .unwrap_or(Value::Undefined),
                            set: self
                                .binding_opt(env, &format!("%ps:{n}"))
                                .unwrap_or(Value::Undefined),
                        },
                        enumerable: false,
                        configurable: false,
                    }
                };
                self.private_add(target, key, property)?;
            }
        }
        Ok(Value::Undefined)
    }
    /// Run the instance field initializer of class constructor `f` on `this`.
    pub(super) fn initialize_fields(&mut self, f: &Value, this: Value) -> Result<()> {
        let Some(id) = f.id() else { return Ok(()) };
        let Some(Object::Closure { environment, .. }) = self.objects.get(&id) else {
            return Ok(());
        };
        let environment = *environment;
        if let Some(fields @ Value::Object(_)) = self.binding_opt(environment, "%fields") {
            self.invoke_this(fields, this, vec![])?;
        }
        Ok(())
    }
    /// Base class [[Construct]]: allocate from new.target, then fields, then the body.
    pub(super) fn construct_class(
        &mut self,
        f: Value,
        args: Vec<Value>,
        new_target: Value,
        derived: bool,
    ) -> Result<Value> {
        if derived {
            self.pending_construct = Some((new_target, f.clone()));
            let r = self.invoke_this(f, Value::Undefined, args);
            self.pending_construct = None;
            return r;
        }
        let p = self.get_member(new_target.clone(), "prototype")?;
        let p = if let Value::Object(p) = p {
            p
        } else {
            self.intrinsics.as_ref().unwrap().object
        };
        let id = self.new_object(Some(p))?;
        let receiver = Value::Object(id);
        self.protect(&receiver)?;
        self.initialize_fields(&f, receiver.clone())?;
        self.pending_construct = Some((new_target, f.clone()));
        let r = self.invoke_this(f, receiver.clone(), args);
        self.pending_construct = None;
        let result = r?;
        Ok(if matches!(result, Value::Object(_)) {
            result
        } else {
            receiver
        })
    }
    pub(super) fn super_call(&mut self, args: &[Expr], env: Id) -> Result<Value> {
        let func = self.lookup(env, "%func")?;
        self.protect(&func)?;
        let parent = match func.id() {
            Some(id) => self.data(id)?.prototype,
            None => None,
        };
        let parent = parent.map(Value::Object).unwrap_or(Value::Null);
        self.protect(&parent)?;
        let values = if matches!(args, [Expr::Name(n)] if n == "%forward") {
            let arguments = self.lookup(env, "arguments")?;
            self.protect(&arguments)?;
            let mut values = vec![];
            if let Value::Object(a) = arguments {
                let length = self.length_of(a)?;
                for i in 0..length {
                    let v = self.get_member(Value::Object(a), i.to_string())?;
                    self.protect(&v)?;
                    values.push(v);
                }
            }
            values
        } else if args.iter().any(|e| matches!(e, Expr::Spread(_))) {
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
        if !self.constructible(&parent) {
            return self.throw_error("TypeError", "super constructor is not a constructor");
        }
        let new_target = self.lookup(env, "%newtarget")?;
        self.protect(&new_target)?;
        let result = self.construct_with_new_target(parent, values, new_target)?;
        self.protect(&result)?;
        let mut at = env;
        let this_env = loop {
            let Some(Object::Environment {
                parent, bindings, ..
            }) = self.objects.get(&at)
            else {
                return Err(ScriptError::Runtime("missing this environment"));
            };
            if let Some(v) = bindings.get("this") {
                if v.is_some() {
                    return self.throw_error("ReferenceError", "super() called twice");
                }
                break at;
            }
            match parent {
                Some(p) => at = *p,
                None => return Err(ScriptError::Runtime("missing this environment")),
            }
        };
        self.initialize(this_env, "this", result.clone())?;
        self.initialize_fields(&func, result.clone())?;
        Ok(result)
    }
    /// Returns (this, target object of the super lookup).
    pub(super) fn super_base(&mut self, env: Id) -> Result<(Value, Option<Id>)> {
        let this = self.lookup(env, "this")?;
        self.protect(&this)?;
        let home = self.lookup(env, "%home")?;
        let target = match home {
            Value::Object(h) => self.data(h)?.prototype,
            _ => None,
        };
        Ok((this, target))
    }
    pub(super) fn super_get(
        &mut self,
        this: Value,
        target: Option<Id>,
        key: &Key,
    ) -> Result<Value> {
        let Some(target) = target else {
            return self.throw_error("TypeError", "super base is null");
        };
        self.ordinary_get(target, key, this)
    }
    pub(super) fn super_set(
        &mut self,
        this: Value,
        target: Option<Id>,
        key: Key,
        value: Value,
    ) -> Result<()> {
        let Some(target) = target else {
            return self.throw_error("TypeError", "super base is null");
        };
        if self.ordinary_set(target, key, value, this)? {
            Ok(())
        } else {
            self.throw_error("TypeError", "super assignment refused")
        }
    }
    pub(super) fn super_key(&mut self, x: &Expr, env: Id) -> Result<(Value, Option<Id>, Key)> {
        let this = self.lookup(env, "this")?;
        self.protect(&this)?;
        let key = match x {
            Expr::SuperMember(k) => Key::from(k.as_str()),
            Expr::SuperComputed(k) => {
                let k = self.eval(k, env)?;
                self.protect(&k)?;
                self.to_key(k)?
            }
            _ => return Err(ScriptError::Runtime("invalid super reference")),
        };
        let (_, target) = self.super_base(env)?;
        Ok((this, target, key))
    }
    pub(super) fn private_add(&mut self, id: Id, key: Key, property: Property) -> Result<()> {
        if self.data(id)?.own(&key).is_some() {
            return self.throw_error("TypeError", "private element already present");
        }
        let mut data = self.data(id)?.clone();
        data.properties.push((key, property));
        self.replace_data(id, data)
    }
    pub(super) fn private_get(&mut self, receiver: Value, symbol: u64) -> Result<Value> {
        let Value::Object(id) = receiver else {
            return self.throw_error("TypeError", "private member of non-object");
        };
        let key = Key::Symbol(symbol);
        let property = match self.objects.get(&id) {
            Some(Object::Environment { .. } | Object::Node { .. }) | None => None,
            _ => self.data(id)?.own(&key).cloned(),
        };
        match property.map(|p| p.kind) {
            None => self.throw_error("TypeError", "private member not present"),
            Some(Kind::Data { value, .. }) => Ok(value),
            Some(Kind::Accessor { get, .. }) => {
                if get == Value::Undefined {
                    self.throw_error("TypeError", "private accessor has no getter")
                } else {
                    self.invoke_this(get, receiver, vec![])
                }
            }
        }
    }
    pub(super) fn private_set(&mut self, receiver: Value, symbol: u64, value: Value) -> Result<()> {
        let Value::Object(id) = receiver else {
            return self.throw_error("TypeError", "private member of non-object");
        };
        let key = Key::Symbol(symbol);
        let property = match self.objects.get(&id) {
            Some(Object::Environment { .. } | Object::Node { .. }) | None => None,
            _ => self.data(id)?.own(&key).cloned(),
        };
        match property.map(|p| p.kind) {
            None => self.throw_error("TypeError", "private member not present"),
            Some(Kind::Data {
                writable: false, ..
            }) => self.throw_error("TypeError", "private method is not writable"),
            Some(Kind::Data { .. }) => {
                let mut data = self.data(id)?.clone();
                if let Some((_, p)) = data.properties.iter_mut().find(|(k, _)| *k == key) {
                    p.kind = Kind::Data {
                        value,
                        writable: true,
                    };
                }
                self.replace_data(id, data)
            }
            Some(Kind::Accessor { set, .. }) => {
                if set == Value::Undefined {
                    self.throw_error("TypeError", "private accessor has no setter")
                } else {
                    self.invoke_this(set, receiver, vec![value]).map(|_| ())
                }
            }
        }
    }
    pub(super) fn private_in(&mut self, name: &str, object: Value, env: Id) -> Result<Value> {
        let Value::Object(id) = object else {
            return self.throw_error("TypeError", "private in requires an object");
        };
        let symbol = self.lookup(env, name)?;
        let key = self.to_key(symbol)?;
        let present = match self.objects.get(&id) {
            Some(Object::Environment { .. } | Object::Node { .. }) | None => false,
            _ => self.data(id)?.own(&key).is_some(),
        };
        Ok(Value::Bool(present))
    }
    pub(super) fn is_private(&self, key: &Key) -> bool {
        matches!(key, Key::Symbol(id) if self.private_symbols.contains(id))
    }
}
