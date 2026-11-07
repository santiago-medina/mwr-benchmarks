//! Explicit intrinsic table; unsupported features fail distinctly from JavaScript exceptions.
use super::{
    objects::{Data, Descriptor, Kind},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::{Domain, Id};
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct Intrinsics {
    pub object: Id,
    pub array: Id,
    pub promise: Id,
    pub array_iterator: Id,
    pub function: Id,
    pub global: Id,
    pub functions: HashMap<String, Id>,
    pub errors: HashMap<String, Id>,
}
const ERRORS: &[&str] = &[
    "Error",
    "EvalError",
    "URIError",
    "TypeError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "Test262Error",
];
const METHODS: &[(&str, &[&str])] = &[
    (
        "Object",
        &[
            "create",
            "defineProperty",
            "defineProperties",
            "getOwnPropertyDescriptor",
            "getPrototypeOf",
            "setPrototypeOf",
            "preventExtensions",
            "isExtensible",
            "is",
            "freeze",
            "seal",
            "isFrozen",
            "isSealed",
            "assign",
            "keys",
            "getOwnPropertyNames",
            "getOwnPropertySymbols",
        ],
    ),
    (
        "Reflect",
        &[
            "construct",
            "apply",
            "get",
            "set",
            "defineProperty",
            "getOwnPropertyDescriptor",
            "getPrototypeOf",
            "setPrototypeOf",
            "preventExtensions",
            "isExtensible",
            "has",
            "deleteProperty",
            "ownKeys",
        ],
    ),
    (
        "Object.prototype",
        &[
            "hasOwnProperty",
            "propertyIsEnumerable",
            "isPrototypeOf",
            "valueOf",
            "toString",
        ],
    ),
    ("Array", &["isArray"]),
    ("Promise", &["resolve", "reject"]),
    ("Promise.prototype", &["then", "catch", "finally"]),
    (
        "Array.prototype",
        &[
            "indexOf", "push", "pop", "join", "toString", "map", "filter", "forEach", "some",
            "every", "values", "keys", "entries",
        ],
    ),
    ("Function.prototype", &["call", "bind", "apply", "toString"]),
    ("String.prototype", &["valueOf", "toString"]),
    ("Number.prototype", &["valueOf", "toString"]),
    ("Boolean.prototype", &["valueOf", "toString"]),
    ("assert", &["sameValue", "notSameValue", "throws"]),
];
impl Engine<'_> {
    fn hidden(&mut self, id: Id, key: impl Into<super::objects::Key>, v: Value) -> Result<()> {
        self.define(
            id,
            key,
            Descriptor {
                value: Some(v),
                writable: Some(true),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(())
    }
    fn intrinsic_function(&mut self, global: Id, name: &str, prototype: Option<Id>) -> Result<Id> {
        let id = self.allocate(Domain::Js, -600)?;
        self.runtime.link(global, id)?;
        self.objects.insert(
            id,
            Object::Builtin {
                name: name.into(),
                data: Data::new(None),
            },
        );
        self.replace_data(id, Data::new(prototype))?;
        let short = if name == "%IteratorPrototype%.iterator" {
            "[Symbol.iterator]"
        } else if matches!(name, "Array.species" | "Promise.species") {
            "get [Symbol.species]"
        } else if name == "Function.prototype" {
            ""
        } else {
            name.rsplit('.').next().unwrap()
        };
        let arity = match name {
            "Reflect.construct"
            | "Function.prototype.apply"
            | "Promise.prototype.then"
            | "parseInt"
            | "Object.create"
            | "Object.defineProperties"
            | "Object.getOwnPropertyDescriptor"
            | "Object.setPrototypeOf"
            | "Reflect.get"
            | "Reflect.setPrototypeOf"
            | "Reflect.has"
            | "Reflect.deleteProperty"
            | "Reflect.getOwnPropertyDescriptor"
            | "Object.is"
            | "assert.throws"
            | "assert.sameValue"
            | "assert.notSameValue" => 2,
            "Reflect.apply"
            | "Object.defineProperty"
            | "Reflect.defineProperty"
            | "Reflect.set" => 3,
            "Array.prototype.values"
            | "Array.prototype.keys"
            | "Array.prototype.entries"
            | "Array.prototype.pop"
            | "Array.prototype.toString"
            | "%ArrayIteratorPrototype%.next"
            | "%IteratorPrototype%.iterator"
            | "Promise.species"
            | "Array.species"
            | "Symbol"
            | "Function.prototype"
            | "Object.prototype.toString"
            | "Object.prototype.valueOf"
            | "String.prototype.valueOf"
            | "String.prototype.toString"
            | "Number.prototype.valueOf"
            | "Boolean.prototype.valueOf"
            | "Boolean.prototype.toString" => 0,
            _ => 1,
        };
        self.define(
            id,
            "length",
            Descriptor {
                value: Some(Value::number(arity as f64)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        self.define(
            id,
            "name",
            Descriptor {
                value: Some(Value::String(short.into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(id)
    }
    pub(super) fn bootstrap(&mut self, env: Id) -> Result<()> {
        let object = self.new_object(None)?;
        self.runtime.link(env, object)?;
        let function = self.intrinsic_function(env, "Function.prototype", Some(object))?;
        let global = self.new_object(Some(object))?;
        self.runtime.link(env, global)?;
        self.intrinsics = Some(Intrinsics {
            object,
            array: object,
            promise: object,
            array_iterator: object,
            function,
            global,
            functions: HashMap::new(),
            errors: HashMap::new(),
        });
        let array = self.array_with_proto(0, object)?;
        self.runtime.link(env, array)?;
        self.intrinsics.as_mut().unwrap().array = array;
        let iterator = self.new_object(Some(object))?;
        self.runtime.link(env, iterator)?;
        let array_iterator = self.new_object(Some(iterator))?;
        self.runtime.link(env, array_iterator)?;
        self.intrinsics.as_mut().unwrap().array_iterator = array_iterator;
        let mut objects = HashMap::from([
            ("Object.prototype".to_string(), object),
            ("Function.prototype".to_string(), function),
        ]);
        let mut names = vec![
            "Object",
            "Array",
            "Symbol",
            "parseInt",
            "Promise",
            "queueMicrotask",
            "Function",
            "String",
            "Number",
            "Boolean",
            "%ThrowTypeError%",
        ];
        names.extend(ERRORS.iter().copied());
        if self.test_harness {
            names.extend(["assert", "$ERROR", "$DONE"]);
        }
        for name in names {
            let id = self.intrinsic_function(env, name, Some(function))?;
            self.intrinsics
                .as_mut()
                .unwrap()
                .functions
                .insert(name.into(), id);
            objects.insert(name.into(), id);
        }
        let promise = self.new_object(Some(object))?;
        self.runtime.link(env, promise)?;
        self.intrinsics.as_mut().unwrap().promise = promise;
        objects.insert("Promise.prototype".into(), promise);
        objects.insert("Array.prototype".into(), array);
        let reflect = self.new_object(Some(object))?;
        self.runtime.link(env, reflect)?;
        objects.insert("Reflect".into(), reflect);
        for name in ["String", "Number", "Boolean"] {
            let p = self.new_object(Some(object))?;
            self.runtime.link(env, p)?;
            let mut d = self.data(p)?.clone();
            d.boxed = Some(match name {
                "String" => Value::String(String::new()),
                "Number" => Value::number(0.0),
                _ => Value::Bool(false),
            });
            self.replace_data(p, d)?;
            if name == "String" {
                self.define(
                    p,
                    "length",
                    Descriptor {
                        value: Some(Value::number(0.0)),
                        ..Descriptor::default()
                    },
                )?;
            }
            objects.insert(format!("{name}.prototype"), p);
        }
        objects.insert("Object.prototype".into(), object);
        objects.insert("Function.prototype".into(), function);
        for name in ERRORS {
            let parent = if *name == "Error" {
                object
            } else {
                self.intrinsics.as_ref().unwrap().errors["Error"]
            };
            let p = self.new_object(Some(parent))?;
            self.runtime.link(env, p)?;
            self.hidden(p, "name", Value::String((*name).into()))?;
            self.hidden(p, "message", Value::String(String::new()))?;
            self.intrinsics
                .as_mut()
                .unwrap()
                .errors
                .insert((*name).into(), p);
            objects.insert(format!("{name}.prototype"), p);
            if *name != "Error" {
                let ctor = objects[*name];
                let mut d = self.data(ctor)?.clone();
                d.prototype = Some(objects["Error"]);
                self.replace_data(ctor, d)?;
            }
        }
        let constructors: Vec<_> = self
            .intrinsics
            .as_ref()
            .unwrap()
            .functions
            .iter()
            .map(|(n, id)| (n.clone(), *id))
            .collect();
        for (n, id) in &constructors {
            if let Some(p) = objects.get(&format!("{n}.prototype")) {
                self.define(
                    *id,
                    "prototype",
                    Descriptor {
                        value: Some(Value::Object(*p)),
                        ..Descriptor::default()
                    },
                )?;
                self.hidden(*p, "constructor", Value::Object(*id))?;
            }
        }
        for (owner, methods) in METHODS {
            if *owner == "assert" && !self.test_harness {
                continue;
            }
            for method in *methods {
                let name = format!("{owner}.{method}");
                let id = self.intrinsic_function(env, &name, Some(function))?;
                self.intrinsics.as_mut().unwrap().functions.insert(name, id);
                self.hidden(objects[*owner], *method, Value::Object(id))?;
            }
        }
        let self_iter =
            self.intrinsic_function(env, "%IteratorPrototype%.iterator", Some(function))?;
        self.hidden(
            iterator,
            super::objects::Key::Symbol(0),
            Value::Object(self_iter),
        )?;
        let next = self.intrinsic_function(env, "%ArrayIteratorPrototype%.next", Some(function))?;
        self.hidden(array_iterator, "next", Value::Object(next))?;
        let values = self.intrinsics.as_ref().unwrap().functions["Array.prototype.values"];
        self.hidden(array, super::objects::Key::Symbol(0), Value::Object(values))?;
        let species = self.intrinsic_function(env, "Array.species", Some(function))?;
        self.define(
            objects["Array"],
            super::objects::Key::Symbol(2),
            Descriptor {
                get: Some(Value::Object(species)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        self.define(
            array_iterator,
            super::objects::Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Array Iterator".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        let species = self.intrinsic_function(env, "Promise.species", Some(function))?;
        self.define(
            objects["Promise"],
            super::objects::Key::Symbol(2),
            Descriptor {
                get: Some(Value::Object(species)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        self.define(
            promise,
            super::objects::Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Promise".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        let symbol = objects["Symbol"];
        for (key, id) in [("iterator", 0), ("toStringTag", 1), ("species", 2)] {
            self.define(
                symbol,
                key,
                Descriptor {
                    value: Some(Value::Symbol(id)),
                    ..Descriptor::default()
                },
            )?;
        }
        let entries: Vec<_> = objects
            .iter()
            .filter(|(n, _)| !n.contains('.'))
            .map(|(n, id)| (n.clone(), Value::Object(*id)))
            .chain([
                ("this".into(), Value::Object(global)),
                ("globalThis".into(), Value::Object(global)),
                ("undefined".into(), Value::Undefined),
                ("NaN".into(), Value::number(f64::NAN)),
                ("Infinity".into(), Value::number(f64::INFINITY)),
            ])
            .collect();
        for (n, v) in entries {
            let declared = matches!(self.objects.get(&env),Some(Object::Environment{bindings,..})if bindings.contains_key(&n));
            if !declared {
                if matches!(n.as_str(), "undefined" | "NaN" | "Infinity" | "this") {
                    self.initialize(env, &n, v.clone())?;
                } else {
                    self.mutable_binding(env, &n, v.clone())?;
                }
            }
            if n != "this" {
                if matches!(n.as_str(), "undefined" | "NaN" | "Infinity") {
                    self.define(
                        global,
                        &n,
                        Descriptor {
                            value: Some(v),
                            ..Descriptor::default()
                        },
                    )?;
                } else {
                    self.hidden(global, &n, v)?;
                }
            }
        }
        let number = objects["Number"];
        for (key, n) in [
            ("MAX_VALUE", f64::MAX),
            ("MIN_VALUE", f64::from_bits(1)),
            ("NaN", f64::NAN),
            ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("POSITIVE_INFINITY", f64::INFINITY),
            ("EPSILON", f64::EPSILON),
            ("MAX_SAFE_INTEGER", 9007199254740991.0),
            ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ] {
            self.define(
                number,
                key,
                Descriptor {
                    value: Some(Value::number(n)),
                    ..Descriptor::default()
                },
            )?;
        }
        let vars = if let Some(Object::Environment {
            object_bindings, ..
        }) = self.objects.get(&env)
        {
            object_bindings.clone()
        } else {
            unreachable!()
        };
        for name in vars {
            if self.own(global, &name)?.is_none() {
                self.define(
                    global,
                    &name,
                    Descriptor {
                        value: Some(Value::Undefined),
                        writable: Some(true),
                        enumerable: Some(true),
                        ..Descriptor::default()
                    },
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn error_value(&mut self, name: &str, message: &str) -> Result<Value> {
        let Some(i) = &self.intrinsics else {
            return Err(ScriptError::Runtime("exception without realm"));
        };
        let p = *i.errors.get(name).unwrap_or(&i.errors["Error"]);
        let id = self.new_object(Some(p))?;
        self.protect(&Value::Object(id))?;
        let mut d = self.data(id)?.clone();
        d.error_class = Some(name.into());
        self.replace_data(id, d)?;
        self.hidden(id, "message", Value::String(message.into()))?;
        Ok(Value::Object(id))
    }
    pub(super) fn throw_error<T>(&mut self, name: &str, message: &str) -> Result<T> {
        let v = self.error_value(name, message)?;
        Err(self.thrown(v))
    }
    fn object_argument(&mut self, v: Value) -> Result<Id> {
        if let Value::Object(id) = v {
            self.data(id)?;
            Ok(id)
        } else {
            self.throw_error("TypeError", "expected object")
        }
    }
    pub(super) fn object_coercible(&mut self, v: Value) -> Result<Id> {
        match v {
            Value::Object(id) => {
                self.data(id)?;
                Ok(id)
            }
            Value::Null | Value::Undefined => self.throw_error("TypeError", "null or undefined"),
            v => self.box_value(v),
        }
    }
    pub(super) fn box_value(&mut self, v: Value) -> Result<Id> {
        let name = match &v {
            Value::String(_) => "String",
            Value::Number(_) => "Number",
            Value::Bool(_) => "Boolean",
            Value::Object(id) => return Ok(*id),
            Value::Symbol(_) => return Err(ScriptError::Unsupported("symbol boxing")),
            _ => return self.throw_error("TypeError", "cannot box value"),
        };
        let ctor = self.intrinsics.as_ref().unwrap().functions[name];
        let p = self.get_member(Value::Object(ctor), "prototype")?;
        let Value::Object(p) = p else { unreachable!() };
        let id = self.new_object(Some(p))?;
        self.protect(&Value::Object(id))?;
        let mut d = self.data(id)?.clone();
        d.boxed = Some(v.clone());
        self.replace_data(id, d)?;
        if let Value::String(s) = v {
            self.define(
                id,
                "length",
                Descriptor {
                    value: Some(Value::number(s.encode_utf16().count() as f64)),
                    ..Descriptor::default()
                },
            )?;
            for (i, c) in s.encode_utf16().enumerate() {
                let Some(c) = char::from_u32(c as u32) else {
                    return Err(ScriptError::Unsupported("surrogate string wrapper"));
                };
                self.define(
                    id,
                    &i.to_string(),
                    Descriptor {
                        value: Some(Value::String(c.to_string())),
                        enumerable: Some(true),
                        ..Descriptor::default()
                    },
                )?;
            }
        }
        Ok(id)
    }
    pub(super) fn arguments_object(&mut self, args: &[Value]) -> Result<Value> {
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        let mut d = self.data(id)?.clone();
        d.arguments = true;
        self.replace_data(id, d)?;
        for (i, v) in args.iter().enumerate() {
            self.put_data(id, &i.to_string(), v.clone())?;
        }
        self.hidden(id, "length", Value::number(args.len() as f64))?;
        let f = Value::Object(self.intrinsics.as_ref().unwrap().functions["%ThrowTypeError%"]);
        self.define(
            id,
            "callee",
            Descriptor {
                get: Some(f.clone()),
                set: Some(f),
                ..Descriptor::default()
            },
        )?;
        Ok(Value::Object(id))
    }
    fn bind_function(&mut self, target: Value, mut args: Vec<Value>) -> Result<Value> {
        if !self.callable(&target) {
            return self.throw_error("TypeError", "bind target not callable");
        }
        let receiver = if args.is_empty() {
            Value::Undefined
        } else {
            args.remove(0)
        };
        let p = if let Value::Object(id) = target {
            self.data(id)?.prototype
        } else {
            Some(self.intrinsics.as_ref().unwrap().function)
        };
        let name = self.get_member(target.clone(), "name")?;
        let length = self.get_member(target.clone(), "length")?;
        let n = if let Value::Number(n) = length {
            (f64::from_bits(n).trunc() - args.len() as f64).max(0.0)
        } else {
            0.0
        };
        let name = if let Value::String(s) = name {
            s
        } else {
            String::new()
        };
        let id = self.allocate(Domain::Js, -700)?;
        for v in std::iter::once(&target)
            .chain(std::iter::once(&receiver))
            .chain(args.iter())
        {
            if let Some(child) = v.id() {
                self.runtime.link(id, child)?;
            }
        }
        self.objects.insert(
            id,
            Object::Bound {
                target,
                receiver,
                args,
                data: Data::new(None),
            },
        );
        self.protect(&Value::Object(id))?;
        self.replace_data(id, Data::new(p))?;
        self.define(
            id,
            "name",
            Descriptor {
                value: Some(Value::String(format!("bound {name}"))),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        self.define(
            id,
            "length",
            Descriptor {
                value: Some(Value::number(n)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(Value::Object(id))
    }
    fn define_properties(&mut self, target: Id, descs: Value) -> Result<()> {
        let id = self.object_coercible(descs)?;
        self.protect(&Value::Object(id))?;
        let mut converted = vec![];
        for k in self.own_keys(id)? {
            if self.own(id, &k)?.is_some_and(|p| p.enumerable) {
                let v = self.get_member(Value::Object(id), &k)?;
                self.protect(&v)?;
                let d = self.descriptor(v)?;
                converted.push((k, d));
            }
        }
        for (k, d) in converted {
            if !self.define(target, &k, d)? {
                return self.throw_error("TypeError", "property definition refused");
            }
        }
        Ok(())
    }
    fn argument_list(&mut self, value: Value, nullable: bool) -> Result<Vec<Value>> {
        if nullable && matches!(value, Value::Null | Value::Undefined) {
            return Ok(vec![]);
        }
        let id = self.object_argument(value)?;
        let length = self.length_of(id)?;
        let mut args = vec![];
        for index in 0..length {
            self.tick()?;
            let value = self.get_member(Value::Object(id), index.to_string())?;
            self.protect(&value)?;
            args.push(value);
        }
        Ok(args)
    }
    pub(super) fn builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if name == "Promise"
            || name.starts_with("Promise.")
            || name == "queueMicrotask"
            || name == "$DONE"
        {
            return self.promise_builtin(name, receiver, args);
        }
        if name == "Array"
            || name == "Symbol"
            || name.starts_with("Array.")
            || name.starts_with("%ArrayIteratorPrototype%")
            || name.starts_with("%IteratorPrototype%")
        {
            return self.array_builtin(name, receiver, args);
        }
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        if ERRORS.contains(&name) {
            let message = if arg(0) == Value::Undefined {
                String::new()
            } else {
                self.to_string(arg(0))?
            };
            return self.error_value(name, &message);
        }
        if name == "assert" || name.starts_with("assert.") || name == "$ERROR" {
            return self.assert_builtin(name, args);
        }
        match name {
            "Reflect.construct" => {
                let target = arg(0);
                let new_target = if args.len() > 2 {
                    arg(2)
                } else {
                    target.clone()
                };
                if !self.constructible(&target) || !self.constructible(&new_target) {
                    return self
                        .throw_error("TypeError", "Reflect.construct requires constructors");
                }
                let arguments = self.argument_list(arg(1), false)?;
                return self.construct_with_new_target(target, arguments, new_target);
            }
            "Reflect.apply" | "Function.prototype.apply" => {
                let (target, this, arguments) = if name == "Reflect.apply" {
                    (arg(0), arg(1), arg(2))
                } else {
                    (receiver, arg(0), arg(1))
                };
                if !self.callable(&target) {
                    return self.throw_error("TypeError", "apply target is not callable");
                }
                let values = self.argument_list(arguments, name == "Function.prototype.apply")?;
                return self.invoke_this(target, this, values);
            }
            "parseInt" => {
                let text = self.to_string(arg(0))?;
                let mut s = text.trim_start_matches(|c: char| {
                    c == '\u{feff}' || c != '\u{85}' && c.is_whitespace()
                });
                let sign = if let Some(t) = s.strip_prefix('-') {
                    s = t;
                    -1.0
                } else {
                    if let Some(t) = s.strip_prefix('+') {
                        s = t;
                    }
                    1.0
                };
                let radix = self.to_number(arg(1))?;
                let radix = if !radix.is_finite() {
                    0
                } else {
                    radix.trunc().rem_euclid(4294967296.0) as u32 as i32
                };
                if radix != 0 && !(2..=36).contains(&radix) {
                    return Ok(Value::number(f64::NAN));
                }
                let strip = radix == 0 || radix == 16;
                let mut radix = if radix == 0 { 10 } else { radix as u32 };
                if strip && (s.starts_with("0x") || s.starts_with("0X")) {
                    s = &s[2..];
                    radix = 16;
                }
                let mut number = 0.0;
                let mut count = 0;
                for ch in s.chars() {
                    let Some(digit) = ch.to_digit(radix) else {
                        break;
                    };
                    count += 1;
                    number = number * radix as f64 + digit as f64;
                }
                return Ok(Value::number(if count == 0 {
                    f64::NAN
                } else {
                    sign * number
                }));
            }
            "Function.prototype" => return Ok(Value::Undefined),
            "%ThrowTypeError%" => {
                return self.throw_error("TypeError", "restricted function property")
            }
            "Function.prototype.call" => {
                return self.invoke_this(receiver, arg(0), args.iter().skip(1).cloned().collect())
            }
            "Function.prototype.bind" => return self.bind_function(receiver, args),
            "Function" | "Function.prototype.toString" => {
                return Err(ScriptError::Unsupported(
                    "dynamic function or function source",
                ))
            }
            "String" => {
                return Ok(Value::String(if args.is_empty() {
                    String::new()
                } else {
                    self.to_string(arg(0))?
                }))
            }
            "Number" => {
                return Ok(Value::number(if args.is_empty() {
                    0.0
                } else {
                    self.to_number(arg(0))?
                }))
            }
            "Boolean" => return Ok(Value::Bool(arg(0).truthy())),
            "Object" => {
                return match arg(0) {
                    Value::Null | Value::Undefined => self
                        .new_object(Some(self.intrinsics.as_ref().unwrap().object))
                        .map(Value::Object),
                    Value::Object(id) => Ok(Value::Object(id)),
                    v => self.box_value(v).map(Value::Object),
                }
            }
            "Object.is" => return Ok(Value::Bool(arg(0).same_value(&arg(1)))),
            "Object.create" => {
                let p = match arg(0) {
                    Value::Null => None,
                    Value::Object(id) => {
                        self.data(id)?;
                        Some(id)
                    }
                    _ => return self.throw_error("TypeError", "prototype must be object or null"),
                };
                let id = self.new_object(p)?;
                self.protect(&Value::Object(id))?;
                if arg(1) != Value::Undefined {
                    self.define_properties(id, arg(1))?;
                }
                return Ok(Value::Object(id));
            }
            "Object.prototype.valueOf" => {
                return self.object_coercible(receiver).map(Value::Object)
            }
            "Object.prototype.toString" => {
                let brand = match &receiver {
                    Value::Undefined => "Undefined",
                    Value::Null => "Null",
                    Value::String(_) => "String",
                    Value::Number(_) => "Number",
                    Value::Bool(_) => "Boolean",
                    _ if self.callable(&receiver) => "Function",
                    Value::Object(id) => {
                        let d = self.data(*id)?;
                        if d.array {
                            "Array"
                        } else if d.iterator.is_some() {
                            "Array Iterator"
                        } else if d.arguments {
                            "Arguments"
                        } else if d.error_class.is_some() {
                            "Error"
                        } else {
                            match &d.boxed {
                                Some(Value::String(_)) => "String",
                                Some(Value::Number(_)) => "Number",
                                Some(Value::Bool(_)) => "Boolean",
                                _ => "Object",
                            }
                        }
                    }
                    _ => return Err(ScriptError::Unsupported("host object brand")),
                };
                return Ok(Value::String(format!("[object {brand}]")));
            }
            "Object.prototype.hasOwnProperty" | "Object.prototype.propertyIsEnumerable" => {
                let k = self.to_key(arg(0))?;
                let id = self.object_coercible(receiver)?;
                let p = self.own(id, &k)?;
                return Ok(Value::Bool(if name.ends_with("hasOwnProperty") {
                    p.is_some()
                } else {
                    p.is_some_and(|p| p.enumerable)
                }));
            }
            "Object.prototype.isPrototypeOf" => {
                if !matches!(arg(0), Value::Object(_)) {
                    return Ok(Value::Bool(false));
                }
                let id = self.object_coercible(receiver)?;
                let Value::Object(mut at) = arg(0) else {
                    unreachable!()
                };
                for _ in 0..10_000 {
                    match self.data(at)?.prototype {
                        Some(p) if p == id => return Ok(Value::Bool(true)),
                        Some(p) => at = p,
                        None => return Ok(Value::Bool(false)),
                    }
                }
                return Err(ScriptError::Runtime("prototype traversal limit"));
            }
            _ => {}
        }
        if name.starts_with("String.prototype.")
            || name.starts_with("Number.prototype.")
            || name.starts_with("Boolean.prototype.")
        {
            let primitive = if let Value::Object(id) = receiver {
                self.data(id)?.boxed.clone().unwrap_or(Value::Undefined)
            } else {
                receiver
            };
            let valid = matches!(
                (name.split('.').next().unwrap(), &primitive),
                ("String", Value::String(_))
                    | ("Number", Value::Number(_))
                    | ("Boolean", Value::Bool(_))
            );
            if !valid {
                return self.throw_error("TypeError", "invalid primitive receiver");
            }
            if name.ends_with("valueOf") {
                return Ok(primitive);
            }
            if name == "Number.prototype.toString"
                && arg(0) != Value::Undefined
                && self.to_number(arg(0))? != 10.0
            {
                return Err(ScriptError::Unsupported("number radix"));
            }
            return self.to_string(primitive).map(Value::String);
        }
        let reflect = name.starts_with("Reflect.");
        let method = name
            .split('.')
            .nth(1)
            .ok_or(ScriptError::Unsupported("builtin"))?;

        if !reflect
            && matches!(
                method,
                "isExtensible" | "isFrozen" | "isSealed" | "preventExtensions" | "freeze" | "seal"
            )
            && !matches!(arg(0), Value::Object(_))
        {
            return Ok(if method.starts_with("is") {
                Value::Bool(method != "isExtensible")
            } else {
                arg(0)
            });
        }
        if !reflect && method == "setPrototypeOf" {
            if matches!(arg(0), Value::Null | Value::Undefined) {
                return self.throw_error("TypeError", "null or undefined");
            }
            if !matches!(arg(1), Value::Null | Value::Object(_)) {
                return self.throw_error("TypeError", "invalid prototype");
            }
            if !matches!(arg(0), Value::Object(_)) {
                return Ok(arg(0));
            }
        }
        let id = if !reflect
            && matches!(
                method,
                "getPrototypeOf"
                    | "getOwnPropertyDescriptor"
                    | "keys"
                    | "getOwnPropertyNames"
                    | "getOwnPropertySymbols"
            ) {
            self.object_coercible(arg(0))?
        } else if !reflect && method == "assign" {
            self.object_coercible(arg(0))?
        } else {
            self.object_argument(arg(0))?
        };
        self.protect(&Value::Object(id))?;
        match method {
            "keys" | "getOwnPropertyNames" | "getOwnPropertySymbols" | "ownKeys" => {
                let mut values = vec![];
                for key in self.own_keys(id)? {
                    let include = match &key {
                        super::objects::Key::String(_) => {
                            method != "getOwnPropertySymbols"
                                && (method != "keys"
                                    || self.own(id, &key)?.is_some_and(|p| p.enumerable))
                        }
                        super::objects::Key::Symbol(_) => {
                            matches!(method, "getOwnPropertySymbols" | "ownKeys")
                        }
                    };
                    if include {
                        values.push(match key {
                            super::objects::Key::String(s) => Value::String(s),
                            super::objects::Key::Symbol(id) => Value::Symbol(id),
                        });
                    }
                }
                let result = self.new_array(values.len() as u32)?;
                self.protect(&Value::Object(result))?;
                for (i, v) in values.into_iter().enumerate() {
                    self.put_data(result, i.to_string(), v)?;
                }
                Ok(Value::Object(result))
            }
            "defineProperty" => {
                let key = self.to_key(arg(1))?;
                let d = self.descriptor(arg(2))?;
                let ok = self.define(id, &key, d)?;
                if reflect {
                    Ok(Value::Bool(ok))
                } else if ok {
                    Ok(Value::Object(id))
                } else {
                    self.throw_error("TypeError", "property definition refused")
                }
            }
            "defineProperties" => {
                self.define_properties(id, arg(1))?;
                Ok(Value::Object(id))
            }
            "getOwnPropertyDescriptor" => {
                let key = self.to_key(arg(1))?;
                match self.own(id, &key)? {
                    None => Ok(Value::Undefined),
                    Some(p) => self.descriptor_object(p),
                }
            }
            "getPrototypeOf" => Ok(self
                .data(id)?
                .prototype
                .map(Value::Object)
                .unwrap_or(Value::Null)),
            "setPrototypeOf" => {
                let p = match arg(1) {
                    Value::Null => None,
                    Value::Object(p) => {
                        self.data(p)?;
                        Some(p)
                    }
                    _ => return self.throw_error("TypeError", "invalid prototype"),
                };
                let ok = self.set_prototype(id, p)?;
                if reflect {
                    Ok(Value::Bool(ok))
                } else if ok {
                    Ok(Value::Object(id))
                } else {
                    self.throw_error("TypeError", "prototype change refused")
                }
            }
            "isExtensible" => Ok(Value::Bool(self.data(id)?.extensible)),
            "preventExtensions" | "seal" | "freeze" => {
                let mut d = self.data(id)?.clone();
                d.extensible = false;
                if method != "preventExtensions" {
                    for (_, p) in &mut d.properties {
                        p.configurable = false;
                        if method == "freeze" {
                            if let Kind::Data { writable, .. } = &mut p.kind {
                                *writable = false;
                            }
                        }
                    }
                }
                self.replace_data(id, d)?;
                Ok(if reflect {
                    Value::Bool(true)
                } else {
                    Value::Object(id)
                })
            }
            "isSealed" | "isFrozen" => {
                let d = self.data(id)?;
                Ok(Value::Bool(
                    !d.extensible
                        && d.properties.iter().all(|(_, p)| {
                            !p.configurable
                                && (method != "isFrozen"
                                    || !matches!(p.kind, Kind::Data { writable: true, .. }))
                        }),
                ))
            }
            "get" => {
                let key = self.to_key(arg(1))?;
                self.ordinary_get(
                    id,
                    &key,
                    if args.len() > 2 {
                        arg(2)
                    } else {
                        Value::Object(id)
                    },
                )
            }
            "set" => {
                let key = self.to_key(arg(1))?;
                self.ordinary_set(
                    id,
                    &key,
                    arg(2),
                    if args.len() > 3 {
                        arg(3)
                    } else {
                        Value::Object(id)
                    },
                )
                .map(Value::Bool)
            }
            "has" => {
                let key = self.to_key(arg(1))?;
                self.has(id, &key).map(Value::Bool)
            }
            "deleteProperty" => {
                let key = self.to_key(arg(1))?;
                self.delete(id, &key).map(Value::Bool)
            }
            "assign" => {
                for v in args.iter().skip(1) {
                    if matches!(v, Value::Null | Value::Undefined) {
                        continue;
                    }
                    let src = self.object_coercible(v.clone())?;
                    self.protect(&Value::Object(src))?;
                    for k in self.own_keys(src)? {
                        if self.own(src, &k)?.is_some_and(|p| p.enumerable) {
                            let v = self.get_member(Value::Object(src), &k)?;
                            self.protect(&v)?;
                            self.set_member(Value::Object(id), &k, v)?;
                        }
                    }
                }
                Ok(Value::Object(id))
            }
            _ => Err(ScriptError::Unsupported("builtin operation")),
        }
    }
    fn assert_builtin(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        self.assertions += 1;
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let ok = match name {
            "assert" => arg(0) == Value::Bool(true),
            "assert.sameValue" => arg(0).same_value(&arg(1)),
            "assert.notSameValue" => !arg(0).same_value(&arg(1)),
            "assert.throws" => {
                if !self.callable(&arg(0)) || !self.callable(&arg(1)) {
                    return self.throw_error(
                        "Test262Error",
                        "assert.throws needs constructors and callable",
                    );
                }
                match self.invoke(arg(1), vec![]) {
                    Err(ScriptError::Thrown(ex)) => {
                        self.protect(&ex.value)?;
                        if !matches!(ex.value, Value::Object(_)) {
                            false
                        } else {
                            self.get_member(ex.value, "constructor")?
                                .same_value(&arg(0))
                        }
                    }
                    Ok(_) => false,
                    Err(e) => return Err(e),
                }
            }
            _ => false,
        };
        if ok {
            Ok(Value::Undefined)
        } else {
            self.throw_error("Test262Error", "assertion failed")
        }
    }
}
