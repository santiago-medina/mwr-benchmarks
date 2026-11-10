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
    pub async_function: Id,
    pub global: Id,
    pub functions: HashMap<String, Id>,
    pub errors: HashMap<String, Id>,
    pub collection: HashMap<String, Id>,
}
const ERRORS: &[&str] = &[
    "Error",
    "AggregateError",
    "EvalError",
    "URIError",
    "TypeError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "SuppressedError",
    "Test262Error",
];
const METHODS: &[(&str, &[&str])] = &[
    ("Error.prototype", &["toString"]),
    ("Map", &["groupBy"]),
    ("Proxy", &["revocable"]),
    ("JSON", &["parse", "stringify"]),
    (
        "Number",
        &["isNaN", "isFinite", "isInteger", "isSafeInteger"],
    ),
    (
        "Object",
        &[
            "create",
            "hasOwn",
            "values",
            "entries",
            "fromEntries",
            "getOwnPropertyDescriptors",
            "groupBy",
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
            "toLocaleString",
            "__defineGetter__",
            "__defineSetter__",
            "__lookupGetter__",
            "__lookupSetter__",
        ],
    ),
    ("Array", &["isArray", "from", "of", "fromAsync"]),
    (
        "Promise",
        &[
            "resolve",
            "reject",
            "all",
            "race",
            "allSettled",
            "any",
            "withResolvers",
            "try",
            "allKeyed",
            "allSettledKeyed",
        ],
    ),
    ("Promise.prototype", &["then", "catch", "finally"]),
    (
        "Array.prototype",
        &[
            "indexOf",
            "push",
            "pop",
            "join",
            "toString",
            "toLocaleString",
            "map",
            "filter",
            "forEach",
            "some",
            "every",
            "values",
            "keys",
            "entries",
            "slice",
            "splice",
            "shift",
            "unshift",
            "reverse",
            "fill",
            "copyWithin",
            "at",
            "includes",
            "lastIndexOf",
            "find",
            "findIndex",
            "findLast",
            "findLastIndex",
            "reduce",
            "reduceRight",
            "sort",
            "concat",
            "flat",
            "flatMap",
            "toReversed",
            "toSorted",
            "toSpliced",
            "with",
        ],
    ),
    ("Function.prototype", &["call", "bind", "apply", "toString"]),
    ("String.prototype", &["valueOf", "toString"]),
    (
        "Number.prototype",
        &[
            "valueOf",
            "toString",
            "toFixed",
            "toExponential",
            "toPrecision",
            "toLocaleString",
        ],
    ),
    ("Boolean.prototype", &["valueOf", "toString"]),
    (
        "assert",
        &["sameValue", "notSameValue", "throws", "compareArray"],
    ),
    ("Test262Error", &["thrower"]),
];
impl Engine<'_> {
    pub(super) fn hidden(
        &mut self,
        id: Id,
        key: impl Into<super::objects::Key>,
        v: Value,
    ) -> Result<()> {
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
    pub(super) fn intrinsic_function(
        &mut self,
        global: Id,
        name: &str,
        prototype: Option<Id>,
    ) -> Result<Id> {
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
        let short = if name == "Symbol.prototype.description" {
            "get description"
        } else if name == "Symbol.prototype.toPrimitive" {
            "[Symbol.toPrimitive]"
        } else if name == "Function.prototype.hasInstance" {
            "[Symbol.hasInstance]"
        } else if matches!(
            name,
            "%IteratorPrototype%.iterator" | "String.prototype.iterator"
        ) {
            "[Symbol.iterator]"
        } else if name == "%AsyncIteratorPrototype%.asyncIterator" {
            "[Symbol.asyncIterator]"
        } else if name == "Object.prototype.__proto__.get" {
            "get __proto__"
        } else if name == "Object.prototype.__proto__.set" {
            "set __proto__"
        } else if matches!(name, "Array.species" | "Promise.species") {
            "get [Symbol.species]"
        } else if name == "Function.prototype" || name == "%ProxyRevoke%" {
            ""
        } else {
            name.rsplit('.').next().unwrap()
        };
        let arity = match name {
            "Symbol.prototype.description"
            | "Symbol.prototype.toString"
            | "Symbol.prototype.valueOf" => 0,
            "BigInt.asIntN" | "BigInt.asUintN" => 2,
            "BigInt.prototype.valueOf" | "%ProxyRevoke%" => 0,
            "Proxy" | "Proxy.revocable" => 2,
            "Object.prototype.__proto__.get" => 0,
            "Object.prototype.__defineGetter__" | "Object.prototype.__defineSetter__" => 2,
            "SuppressedError" => 3,
            "AggregateError"
            | "Object.hasOwn"
            | "Object.groupBy"
            | "Map.groupBy"
            | "Array.prototype.with"
            | "Array.prototype.toSpliced" => 2,
            "Array.prototype.toReversed"
            | "Uint8Array.prototype.toBase64"
            | "Uint8Array.prototype.toHex" => 0,
            "Promise.withResolvers"
            | "Error.prototype.toString"
            | "%ForInIteratorPrototype%.next" => 0,
            "Math.random"
            | "String.prototype.iterator"
            | "%StringIteratorPrototype%.next"
            | "String.prototype.trim"
            | "String.prototype.trimStart"
            | "String.prototype.trimEnd"
            | "String.prototype.toLowerCase"
            | "String.prototype.toUpperCase"
            | "String.prototype.toLocaleLowerCase"
            | "String.prototype.toLocaleUpperCase"
            | "String.prototype.isWellFormed"
            | "String.prototype.toWellFormed" => 0,
            "String.prototype.slice"
            | "String.prototype.substring"
            | "String.prototype.replace"
            | "String.prototype.replaceAll"
            | "String.prototype.split" => 2,
            "Math.atan2" | "Math.hypot" | "Math.imul" | "Math.max" | "Math.min" | "Math.pow" => 2,
            "Array.prototype.slice"
            | "Array.prototype.splice"
            | "Array.prototype.copyWithin"
            | "Array.prototype.lastIndexOf"
            | "Array.prototype.includes" => 2,
            "Array.from" | "Array.fromAsync" => 1,
            "Array.of" | "Array.prototype.shift" | "Array.prototype.reverse" => 0,
            "JSON.parse"
            | "Reflect.construct"
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
            | "assert.notSameValue"
            | "assert.compareArray" => 2,
            "JSON.stringify"
            | "Reflect.apply"
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
            | "%AsyncIteratorPrototype%.asyncIterator"
            | "Promise.species"
            | "Array.species"
            | "Symbol"
            | "Function.prototype"
            | "Object.prototype.toString"
            | "Object.prototype.valueOf"
            | "Object.prototype.toLocaleString"
            | "Array.prototype.toLocaleString"
            | "String.prototype.valueOf"
            | "String.prototype.toString"
            | "Number.prototype.valueOf"
            | "Number.prototype.toLocaleString"
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
            async_function: function,
            global,
            functions: HashMap::new(),
            errors: HashMap::new(),
            collection: HashMap::new(),
        });
        let async_function = self.new_object(Some(function))?;
        self.runtime.link(env, async_function)?;
        self.intrinsics.as_mut().unwrap().async_function = async_function;
        let async_constructor = self.intrinsic_function(env, "AsyncFunction", Some(function))?;
        self.hidden(
            async_function,
            "constructor",
            Value::Object(async_constructor),
        )?;
        self.define(
            async_constructor,
            "prototype",
            Descriptor {
                value: Some(Value::Object(async_function)),
                ..Descriptor::default()
            },
        )?;
        self.define(
            async_function,
            super::objects::Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("AsyncFunction".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
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
            "Map",
            "Set",
            "WeakMap",
            "WeakSet",
            "WeakRef",
            "FinalizationRegistry",
            "Symbol",
            "parseInt",
            "parseFloat",
            "encodeURI",
            "encodeURIComponent",
            "decodeURI",
            "decodeURIComponent",
            "isNaN",
            "isFinite",
            "eval",
            "Promise",
            "queueMicrotask",
            "fetch",
            "Function",
            "String",
            "Number",
            "Boolean",
            "BigInt",
            "ArrayBuffer",
            "SharedArrayBuffer",
            "DataView",
            "Date",
            "RegExp",
            "%ThrowTypeError%",
            "Proxy",
        ];
        names.extend(super::typed_arrays::NAMES.iter().map(|(name, _)| *name));
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
        // §28.1.14 Reflect [ %Symbol.toStringTag% ]: "Reflect", configurable only.
        self.define(
            reflect,
            super::objects::Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Reflect".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        let json = self.new_object(Some(object))?;
        self.runtime.link(env, json)?;
        objects.insert("JSON".into(), json);
        self.define(
            json,
            super::objects::Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("JSON".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
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
        self.bootstrap_collections(env, &mut objects, iterator)?;
        self.bootstrap_weak(env, &mut objects)?;
        self.bootstrap_math(env, &mut objects)?;
        self.bootstrap_strings(env, &mut objects, iterator)?;
        self.bootstrap_symbols(env, &mut objects)?;
        self.bootstrap_enumeration(env, iterator)?;
        self.bootstrap_bigint(env, &mut objects)?;
        self.bootstrap_buffers(env, &mut objects)?;
        self.bootstrap_module_source(env)?;
        self.bootstrap_atomics(env, &mut objects)?;
        self.bootstrap_typed(env, &mut objects)?;
        self.bootstrap_generators(env, &mut objects, iterator)?;
        self.bootstrap_dates(env, &mut objects)?;
        self.bootstrap_regexp(env, &mut objects, iterator)?;
        self.bootstrap_iterator_helpers(env, &mut objects, iterator)?;
        self.bootstrap_disposal(env, &mut objects)?;
        #[cfg(feature = "temporal")]
        self.bootstrap_temporal(env, &mut objects)?;
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
        for owner in [
            "GeneratorFunction.prototype",
            "AsyncGeneratorFunction.prototype",
        ] {
            self.define(
                objects[owner],
                "constructor",
                Descriptor {
                    writable: Some(false),
                    ..Descriptor::default()
                },
            )?;
        }
        for (owner, methods) in METHODS {
            if matches!(*owner, "assert" | "Test262Error") && !self.test_harness {
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
        let to_string = self.intrinsics.as_ref().unwrap().functions["Array.prototype.toString"];
        let typed_proto = self.intrinsics.as_ref().unwrap().collection["TypedArray"];
        self.hidden(typed_proto, "toString", Value::Object(to_string))?;
        let values = self.intrinsics.as_ref().unwrap().functions["Array.prototype.values"];
        self.hidden(array, super::objects::Key::Symbol(0), Value::Object(values))?;
        // Annex B §B.2.2.1 Object.prototype.__proto__ accessor.
        let proto_get =
            self.intrinsic_function(env, "Object.prototype.__proto__.get", Some(function))?;
        let proto_set =
            self.intrinsic_function(env, "Object.prototype.__proto__.set", Some(function))?;
        self.define(
            objects["Object.prototype"],
            "__proto__",
            Descriptor {
                get: Some(Value::Object(proto_get)),
                set: Some(Value::Object(proto_set)),
                enumerable: Some(false),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        // §23.1.3.41 Array.prototype[@@unscopables]: a null-prototype object of `true` values.
        let unscopables = self.new_object(None)?;
        self.runtime.link(env, unscopables)?;
        for name in [
            "at",
            "copyWithin",
            "entries",
            "fill",
            "find",
            "findIndex",
            "findLast",
            "findLastIndex",
            "flat",
            "flatMap",
            "includes",
            "keys",
            "toReversed",
            "toSorted",
            "toSpliced",
            "values",
        ] {
            self.put_data(unscopables, name, Value::Bool(true))?;
        }
        self.define(
            array,
            super::objects::Key::Symbol(11),
            Descriptor {
                value: Some(Value::Object(unscopables)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
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
        if let Some(host) = objects.get("$262").copied() {
            self.bootstrap_host_global(host, global)?;
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
            // Proxies have no Data but implement every internal method (§10.5).
            if !self.is_proxy(id) {
                self.data(id)?;
            }
            Ok(id)
        } else {
            self.throw_error("TypeError", "expected object")
        }
    }
    pub(super) fn object_coercible(&mut self, v: Value) -> Result<Id> {
        match v {
            Value::Object(id) => {
                if !self.is_proxy(id) {
                    self.data(id)?;
                }
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
            Value::Symbol(_) => "Symbol",
            Value::BigInt(_) => "BigInt",
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
        let values = self.intrinsics.as_ref().unwrap().functions["Array.prototype.values"];
        self.hidden(id, super::objects::Key::Symbol(0), Value::Object(values))?;
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
    /// CreateMappedArgumentsObject without the map (set by function_environment):
    /// `callee` is the function itself, as a writable, non-enumerable data property.
    pub(super) fn mapped_arguments_object(
        &mut self,
        function: Id,
        args: &[Value],
    ) -> Result<Value> {
        let value = self.arguments_object(args)?;
        let Value::Object(id) = value else {
            unreachable!()
        };
        let mut d = self.data(id)?.clone();
        d.properties
            .retain(|(k, _)| *k != super::objects::Key::from("callee"));
        self.replace_data(id, d)?;
        self.hidden(id, "callee", Value::Object(function))?;
        Ok(value)
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
        // §10.4.1.3 BoundFunctionCreate step 1: targetFunction.[[GetPrototypeOf]]().
        let p = if let Value::Object(id) = target {
            self.get_prototype_of(id)?
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
        if super::iterator_helpers::handles(name) {
            return self.iterator_builtin(name, receiver, args);
        }
        if super::disposal::handles(name) {
            return self.disposal_builtin(name, receiver, args);
        }
        if name == "Proxy" || name.starts_with("Proxy.") {
            return self.proxy_builtin(name, args);
        }
        if name == "eval" {
            // Indirect eval: global environment, sloppy unless the code says otherwise.
            let source = args.into_iter().next().unwrap_or(Value::Undefined);
            return self.perform_eval(source, None);
        }
        #[cfg(feature = "temporal")]
        if name.starts_with("Temporal.") {
            return self.temporal_builtin(name, receiver, args);
        }
        if name == "Date" || name.starts_with("Date.") {
            return self.date_builtin(name, receiver, args);
        }
        if name == "RegExp"
            || name.starts_with("RegExp.")
            || name == "%RegExpStringIteratorPrototype%.next"
        {
            return self.regexp_builtin(name, receiver, args);
        }
        if name.starts_with("%AsyncGenerator%.") {
            return self.async_generator_builtin(name, receiver, args);
        }
        if name == "%AsyncIteratorPrototype%.asyncIterator" {
            return Ok(receiver);
        }
        if name == "AsyncGeneratorFunction" {
            return self.dynamic_function_kind(super::vm::FunctionKind::AsyncGenerator, args, None);
        }
        if name.starts_with("%Generator%.") {
            return self.generator_builtin(name, receiver, args);
        }
        if name == "GeneratorFunction" {
            return self.dynamic_function_kind(super::vm::FunctionKind::Generator, args, None);
        }
        if name.starts_with("Atomics.") || name.starts_with("$262.") || name == "setTimeout" {
            return self.atomics_builtin(name, receiver, args);
        }
        if name.starts_with("Uint8Array.") {
            return self.uint8_base64_builtin(name, receiver, args);
        }
        if name.starts_with("%AbstractModuleSource%") {
            return self.module_source_builtin(name, receiver);
        }
        if super::typed_arrays::kind(name).is_some() || name.starts_with("%TypedArray%") {
            return self.typed_builtin(name, receiver, args);
        }
        if matches!(name, "ArrayBuffer" | "SharedArrayBuffer" | "DataView")
            || name.starts_with("ArrayBuffer.")
            || name.starts_with("SharedArrayBuffer.")
            || name.starts_with("DataView.")
        {
            return self.buffer_builtin(name, receiver, args);
        }
        if name == "BigInt" || name.starts_with("BigInt.") {
            return self.bigint_builtin(name, receiver, args);
        }
        if name == "%ForInIteratorPrototype%.next" {
            return self.enumeration_next(receiver);
        }
        if matches!(
            name,
            "encodeURI" | "encodeURIComponent" | "decodeURI" | "decodeURIComponent"
        ) {
            return self.uri_builtin(name, args);
        }
        if matches!(
            name,
            "Object.hasOwn"
                | "Object.values"
                | "Object.entries"
                | "Object.fromEntries"
                | "Object.getOwnPropertyDescriptors"
                | "Object.groupBy"
                | "Map.groupBy"
        ) {
            return self.object_extended(name, args);
        }
        if name == "Symbol" || name.starts_with("Symbol.") {
            return self.symbol_builtin(name, receiver, args);
        }
        if name == "Function.prototype.hasInstance" {
            return self
                .ordinary_has_instance(args.first().cloned().unwrap_or(Value::Undefined), receiver)
                .map(Value::Bool);
        }
        if name == "%StringIteratorPrototype%.next"
            || name.starts_with("String.")
                && !matches!(
                    name,
                    "String.prototype.toString" | "String.prototype.valueOf"
                )
        {
            return self.string_builtin(name, receiver, args);
        }
        if name.starts_with("Math.")
            || matches!(
                name,
                "parseFloat"
                    | "isNaN"
                    | "isFinite"
                    | "Number.isNaN"
                    | "Number.isFinite"
                    | "Number.isInteger"
                    | "Number.isSafeInteger"
            )
        {
            return self.numeric_builtin(name, args);
        }
        if name.starts_with("JSON.") {
            return self.json_builtin(name, args);
        }
        if name == "WeakRef"
            || name == "FinalizationRegistry"
            || name.starts_with("WeakRef.")
            || name.starts_with("FinalizationRegistry.")
        {
            return self.weak_builtin(name, receiver, args);
        }
        if name == "Map"
            || name == "Set"
            || name == "WeakMap"
            || name == "WeakSet"
            || name.starts_with("WeakMap.")
            || name.starts_with("WeakSet.")
            || name.starts_with("Map.")
            || name.starts_with("Set.")
            || name.starts_with("%MapIterator")
            || name.starts_with("%SetIterator")
        {
            return self.collection_builtin(name, receiver, args);
        }
        if name == "Promise"
            || name.starts_with("Promise.")
            || name == "queueMicrotask"
            || name == "$DONE"
        {
            return self.promise_builtin(name, receiver, args);
        }
        if name == "Array"
            || name.starts_with("Array.")
            || name.starts_with("%ArrayIteratorPrototype%")
            || name.starts_with("%IteratorPrototype%")
        {
            return self.array_builtin(name, receiver, args);
        }
        if name == "fetch" || name.starts_with("Response.") {
            return self.fetch_builtin(name, receiver, args);
        }
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        if ERRORS.contains(&name) {
            return self.error_constructor(name, args, None);
        }
        if name == "Error.prototype.toString" {
            return self.error_to_string(receiver);
        }
        if name == "Test262Error.thrower" {
            let message = match args.first() {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let error =
                self.error_constructor("Test262Error", vec![Value::String(message)], None)?;
            return Err(self.thrown(error));
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
            "AsyncFunction" | "Function" => {
                return self.dynamic_function(name == "AsyncFunction", args, None)
            }
            "Function.prototype.toString" => return self.function_source(receiver),
            "String" => {
                return Ok(Value::String(if args.is_empty() {
                    String::new()
                } else if let Value::Symbol(id) = arg(0) {
                    self.symbol_string(id)
                } else {
                    self.to_string(arg(0))?
                }))
            }
            "Number" => {
                let value = if args.is_empty() {
                    0.0
                } else {
                    let primitive = self.to_primitive(arg(0), false)?;
                    if let Value::BigInt(value) = primitive {
                        value.to_float()
                    } else {
                        self.to_number(primitive)?
                    }
                };
                return Ok(Value::number(value));
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
                        if !self.is_proxy(id) {
                            self.data(id)?;
                        }
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
            // §20.1.3.5: Invoke(this, "toString"), keeping a primitive receiver.
            "Object.prototype.toLocaleString" => {
                if matches!(receiver, Value::Undefined | Value::Null) {
                    return self.throw_error(
                        "TypeError",
                        "Object.prototype.toLocaleString called on null or undefined",
                    );
                }
                let f = self.get_member(receiver.clone(), "toString")?;
                return self.invoke_this(f, receiver, vec![]);
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
                    Value::Symbol(_) => "Symbol",
                    Value::BigInt(_) => "BigInt",
                    // §20.1.3.6 step 4: IsArray sees through proxies; other proxies have no brand slots.
                    Value::Object(id) if self.is_proxy(*id) => {
                        if self.is_array(&receiver)? {
                            "Array"
                        } else if self.callable(&receiver) {
                            "Function"
                        } else {
                            "Object"
                        }
                    }
                    _ if self.callable(&receiver) => "Function",
                    Value::Object(id) => {
                        let d = self.data(*id)?;
                        if d.date.is_some() {
                            "Date"
                        } else if d.regexp.is_some() {
                            "RegExp"
                        } else if d.array {
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
                let tag = if matches!(receiver, Value::Null | Value::Undefined) {
                    Value::Undefined
                } else {
                    self.get_member(receiver.clone(), super::objects::Key::Symbol(1))?
                };
                let brand = if let Value::String(tag) = tag {
                    tag
                } else {
                    brand.into()
                };
                return Ok(Value::String(format!("[object {brand}]")));
            }
            // Annex B §B.2.2.1.1 get Object.prototype.__proto__
            "Object.prototype.__proto__.get" => {
                let id = self.object_coercible(receiver)?;
                return Ok(match self.get_prototype_of(id)? {
                    Some(p) => Value::Object(p),
                    None => Value::Null,
                });
            }
            // Annex B §B.2.2.1.2 set Object.prototype.__proto__
            "Object.prototype.__proto__.set" => {
                if matches!(receiver, Value::Null | Value::Undefined) {
                    return self.throw_error("TypeError", "null or undefined");
                }
                let proto = match arg(0) {
                    Value::Object(p) => Some(p),
                    Value::Null => None,
                    _ => return Ok(Value::Undefined),
                };
                let Value::Object(id) = receiver else {
                    return Ok(Value::Undefined);
                };
                if !self.set_prototype(id, proto)? {
                    return self
                        .throw_error("TypeError", "Object.prototype.__proto__ setter failed");
                }
                return Ok(Value::Undefined);
            }
            // Annex B §B.2.2.2-3 __defineGetter__ / __defineSetter__
            "Object.prototype.__defineGetter__" | "Object.prototype.__defineSetter__" => {
                let id = self.object_coercible(receiver)?;
                self.protect(&Value::Object(id))?;
                let f = arg(1);
                if !self.callable(&f) {
                    return self.throw_error("TypeError", "accessor is not callable");
                }
                let key = self.to_key(arg(0))?;
                let getter = name.ends_with("Getter__");
                let desc = Descriptor {
                    get: getter.then(|| f.clone()),
                    set: (!getter).then(|| f.clone()),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Descriptor::default()
                };
                if !self.define(id, key, desc)? {
                    return self.throw_error("TypeError", "Cannot redefine property");
                }
                return Ok(Value::Undefined);
            }
            // Annex B §B.2.2.4-5 __lookupGetter__ / __lookupSetter__
            "Object.prototype.__lookupGetter__" | "Object.prototype.__lookupSetter__" => {
                let mut id = self.object_coercible(receiver)?;
                self.protect(&Value::Object(id))?;
                let key = self.to_key(arg(0))?;
                let getter = name.ends_with("Getter__");
                loop {
                    self.tick()?;
                    if let Some(p) = self.own(id, &key)? {
                        return Ok(match p.kind {
                            super::objects::Kind::Accessor { get, set } => {
                                if getter {
                                    get
                                } else {
                                    set
                                }
                            }
                            _ => Value::Undefined,
                        });
                    }
                    match self.get_prototype_of(id)? {
                        Some(p) => id = p,
                        None => return Ok(Value::Undefined),
                    }
                }
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
                    match self.get_prototype_of(at)? {
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
            if let Some(method) = name.strip_prefix("Number.prototype.to") {
                if matches!(
                    method,
                    "Fixed" | "Exponential" | "Precision" | "String" | "LocaleString"
                ) {
                    let x = self.to_number(primitive.clone())?;
                    return self.number_format(&format!("to{method}"), x, arg(0));
                }
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
                .get_prototype_of(id)?
                .map(Value::Object)
                .unwrap_or(Value::Null)),
            "setPrototypeOf" => {
                let p = match arg(1) {
                    Value::Null => None,
                    Value::Object(p) => Some(p),
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
            "isExtensible" => Ok(Value::Bool(self.is_extensible(id)?)),
            "preventExtensions" | "seal" | "freeze"
                if self.is_proxy(id) || self.data(id)?.namespace.is_some() =>
            {
                // SetIntegrityLevel (§7.3.15) through the proxy's internal methods.
                if !self.prevent_extensions(id)? {
                    return if reflect {
                        Ok(Value::Bool(false))
                    } else {
                        self.throw_error("TypeError", "preventExtensions refused")
                    };
                }
                if method != "preventExtensions" {
                    for key in self.own_keys(id)? {
                        let mut d = Descriptor {
                            configurable: Some(false),
                            ..Descriptor::default()
                        };
                        if method == "freeze" {
                            match self.own(id, &key)? {
                                None => continue,
                                Some(p) if matches!(p.kind, Kind::Data { .. }) => {
                                    d.writable = Some(false)
                                }
                                Some(_) => {}
                            }
                        }
                        if !self.define(id, &key, d)? {
                            return self.throw_error("TypeError", "property definition refused");
                        }
                    }
                }
                Ok(if reflect {
                    Value::Bool(true)
                } else {
                    Value::Object(id)
                })
            }
            "isSealed" | "isFrozen" if self.is_proxy(id) || self.data(id)?.namespace.is_some() => {
                // TestIntegrityLevel (§7.3.16).
                if self.is_extensible(id)? {
                    return Ok(Value::Bool(false));
                }
                for key in self.own_keys(id)? {
                    if let Some(p) = self.own(id, &key)? {
                        if p.configurable
                            || method == "isFrozen"
                                && matches!(p.kind, Kind::Data { writable: true, .. })
                        {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
                Ok(Value::Bool(true))
            }
            "preventExtensions" | "seal" | "freeze" => {
                if let Some(typed) = self.data(id)?.typed.clone() {
                    if !self.typed_fixed(&typed)? {
                        return if reflect {
                            Ok(Value::Bool(false))
                        } else {
                            self.throw_error(
                                "TypeError",
                                "resizable typed-array extensions cannot be prevented",
                            )
                        };
                    }
                    if method != "preventExtensions" && self.typed_length(&typed)?.unwrap_or(0) != 0
                    {
                        let mut data = self.data(id)?.clone();
                        data.extensible = false;
                        self.replace_data(id, data)?;
                        return self.throw_error(
                            "TypeError",
                            "typed-array elements cannot be sealed or frozen",
                        );
                    }
                }
                let mut d = self.data(id)?.clone();
                d.extensible = false;
                if method != "preventExtensions" {
                    let private = self.private_symbols.clone();
                    for (k, p) in &mut d.properties {
                        if matches!(k, super::objects::Key::Symbol(s) if private.contains(s)) {
                            continue;
                        }
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
                if let Some(typed) = &self.data(id)?.typed {
                    if self.typed_length(typed)?.unwrap_or(0) != 0 {
                        return Ok(Value::Bool(false));
                    }
                }
                let d = self.data(id)?;
                Ok(Value::Bool(
                    !d.extensible
                        && d.properties.iter().all(|(k, p)| {
                            self.is_private(k)
                                || !p.configurable
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
    /// assert.compareArray from test262's assert.js: same length and SameValue per index.
    fn compare_array(&mut self, actual: Value, expected: Value) -> Result<bool> {
        if !matches!(actual, Value::Object(_)) || !matches!(expected, Value::Object(_)) {
            return Ok(false);
        }
        let length = |engine: &mut Self, v: &Value| -> Result<f64> {
            let n = engine.get_member(v.clone(), "length")?;
            engine.to_number(n)
        };
        let (a, b) = (length(self, &actual)?, length(self, &expected)?);
        if a != b {
            return Ok(false);
        }
        for i in 0..a as usize {
            let x = self.get_member(actual.clone(), &i.to_string())?;
            self.protect(&x)?;
            let y = self.get_member(expected.clone(), &i.to_string())?;
            if !x.same_value(&y) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn assert_builtin(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        self.assertions += 1;
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let ok = match name {
            "assert" => arg(0) == Value::Bool(true),
            "assert.sameValue" => arg(0).same_value(&arg(1)),
            "assert.notSameValue" => !arg(0).same_value(&arg(1)),
            "assert.compareArray" => self.compare_array(arg(0), arg(1))?,
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
