//! Proxy exotic objects: ECMA-262 §10.5 internal methods, §28.2 constructor and
//! `Proxy.revocable`. Design: PROXY.md.
//!
//! A proxy has no `Data`: `Engine::data(proxy)` returns Unsupported, so any caller
//! still reading ordinary slots directly shows up as unsupported, never as a wrong
//! pass. To find such a caller, trace `data()` failures for a proxy id.
//!
//! Known gaps (2026-11-09):
//! - Private elements on a proxy (§7.3.31 PrivateFieldAdd works on any object):
//!   `Object::Proxy` has no storage for them yet (`classes.rs` private_add/get).
//! - GetFunctionRealm is `function_realm` in realms.rs (REALMS.md).
use super::{
    objects::{compatible_descriptor, Descriptor, Key, Kind, Property},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::{Domain, Id};
type Result<T> = std::result::Result<T, ScriptError>;

fn key_value(key: &Key) -> Value {
    match key {
        Key::String(s) => Value::String(s.clone().into()),
        Key::Symbol(id) => Value::Symbol(*id),
    }
}

/// CompletePropertyDescriptor (§6.2.6.6) of a validated descriptor, as a property.
fn complete(d: Descriptor) -> Property {
    let kind = if d.get.is_some() || d.set.is_some() {
        Kind::Accessor {
            get: d.get.unwrap_or(Value::Undefined),
            set: d.set.unwrap_or(Value::Undefined),
        }
    } else {
        Kind::Data {
            value: d.value.unwrap_or(Value::Undefined),
            writable: d.writable.unwrap_or(false),
        }
    };
    Property {
        kind,
        enumerable: d.enumerable.unwrap_or(false),
        configurable: d.configurable.unwrap_or(false),
    }
}

fn as_descriptor(p: &Property) -> Descriptor {
    match &p.kind {
        Kind::Data { value, writable } => Descriptor {
            value: Some(value.clone()),
            writable: Some(*writable),
            enumerable: Some(p.enumerable),
            configurable: Some(p.configurable),
            ..Descriptor::default()
        },
        Kind::Accessor { get, set } => Descriptor {
            get: Some(get.clone()),
            set: Some(set.clone()),
            enumerable: Some(p.enumerable),
            configurable: Some(p.configurable),
            ..Descriptor::default()
        },
    }
}

impl Engine<'_> {
    pub(super) fn is_proxy(&self, id: Id) -> bool {
        matches!(self.objects.get(&id), Some(Object::Proxy { .. }))
    }

    /// The [[ProxyTarget]] and [[ProxyHandler]] slots after ValidateNonRevokedProxy (§10.5.14).
    fn proxy_slots(&mut self, id: Id) -> Result<(Id, Id)> {
        let (target, handler) = match self.objects.get(&id) {
            Some(Object::Proxy {
                target: Some(target),
                handler: Some(handler),
                ..
            }) => (*target, *handler),
            _ => {
                return self.throw_error("TypeError", "Cannot perform operation on a revoked proxy")
            }
        };
        // A trap may revoke the proxy (unlinking both) while this internal method
        // still uses them: root them for the rest of the operation.
        self.protect(&Value::Object(target))?;
        self.protect(&Value::Object(handler))?;
        Ok((target, handler))
    }

    /// GetMethod(handler, name) (§7.3.11).
    fn proxy_trap(&mut self, handler: Id, name: &str) -> Result<Option<Value>> {
        let f = self.get_member(Value::Object(handler), name)?;
        match f {
            Value::Undefined | Value::Null => Ok(None),
            f if self.callable(&f) => {
                self.protect(&f)?;
                Ok(Some(f))
            }
            _ => self.throw_error(
                "TypeError",
                &format!("proxy trap '{name}' is not a function"),
            ),
        }
    }

    fn proxy_invariant<T>(&mut self, trap: &str) -> Result<T> {
        self.throw_error(
            "TypeError",
            &format!("proxy '{trap}' trap result violates an invariant of the target"),
        )
    }

    /// CreateArrayFromList (§7.3.17).
    pub(super) fn array_from_values(&mut self, values: Vec<Value>) -> Result<Value> {
        let id = self.new_array(0)?;
        self.protect(&Value::Object(id))?;
        for (index, value) in values.into_iter().enumerate() {
            self.put_data(id, index.to_string(), value)?;
        }
        Ok(Value::Object(id))
    }

    /// ProxyCreate (§10.5.15).
    pub(super) fn proxy_create(&mut self, target: Value, handler: Value) -> Result<Id> {
        let (Value::Object(target_id), Value::Object(handler_id)) = (&target, &handler) else {
            return self.throw_error(
                "TypeError",
                "Cannot create proxy with a non-object as target or handler",
            );
        };
        let (target_id, handler_id) = (*target_id, *handler_id);
        let callable = self.callable(&target);
        let constructor = self.constructible(&target);
        self.protect(&target)?;
        self.protect(&handler)?;
        let id = self.allocate(Domain::Js, -800)?;
        self.runtime.link(id, target_id)?;
        self.runtime.link(id, handler_id)?;
        self.objects.insert(
            id,
            Object::Proxy {
                target: Some(target_id),
                handler: Some(handler_id),
                callable,
                constructor,
            },
        );
        Ok(id)
    }

    /// Proxy.revocable (§28.2.2.1) and the revoke function (§28.2.2.1.1).
    pub(super) fn proxy_builtin(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "Proxy" => self.throw_error("TypeError", "Constructor Proxy requires 'new'"),
            "Proxy.revocable" => {
                let proxy = self.proxy_create(arg(0), arg(1))?;
                self.protect(&Value::Object(proxy))?;
                let object = self.intrinsics.as_ref().unwrap().object;
                let function = self.intrinsics.as_ref().unwrap().function;
                let global = self.intrinsics.as_ref().unwrap().global;
                let revoke = self.intrinsic_function(global, "%ProxyRevoke%", Some(function))?;
                self.runtime.unlink(global, revoke)?;
                self.protect(&Value::Object(revoke))?;
                let mut data = self.data(revoke)?.clone();
                data.boxed = Some(Value::Object(proxy));
                self.replace_data(revoke, data)?;
                let result = self.new_object(Some(object))?;
                self.protect(&Value::Object(result))?;
                self.put_data(result, "proxy", Value::Object(proxy))?;
                self.put_data(result, "revoke", Value::Object(revoke))?;
                Ok(Value::Object(result))
            }
            _ => Err(ScriptError::Unsupported("proxy builtin")),
        }
    }

    /// The revoke function of Proxy.revocable; `f` is the function object itself.
    pub(super) fn proxy_revoke(&mut self, f: Id) -> Result<Value> {
        let mut data = self.data(f)?.clone();
        let Some(Value::Object(proxy)) = data.boxed.take() else {
            return Ok(Value::Undefined);
        };
        self.replace_data(f, data)?;
        if let Some(Object::Proxy {
            target, handler, ..
        }) = self.objects.get_mut(&proxy)
        {
            let old = [target.take(), handler.take()];
            for child in old.into_iter().flatten() {
                self.runtime.unlink(proxy, child)?;
            }
        }
        Ok(Value::Undefined)
    }

    /// The [[NumberData]]/[[StringData]]/[[BooleanData]]/[[BigIntData]] slot, if any.
    /// Proxies have none of these slots (§10.5), so they read as plain objects.
    pub(super) fn boxed_slot(&mut self, id: Id) -> Result<Option<Value>> {
        if self.is_proxy(id) {
            return Ok(None);
        }
        Ok(self.data(id)?.boxed.clone())
    }

    /// IsArray (§7.2.2).
    pub(super) fn is_array(&mut self, value: &Value) -> Result<bool> {
        let Value::Object(mut id) = value else {
            return Ok(false);
        };
        for _ in 0..10_000 {
            if !self.is_proxy(id) {
                return Ok(self.data(id).is_ok_and(|d| d.array));
            }
            id = self.proxy_slots(id)?.0;
        }
        Err(ScriptError::Runtime("proxy chain limit"))
    }

    // ---- Essential internal methods that dispatch on proxies ----

    /// [[GetPrototypeOf]].
    pub(super) fn get_prototype_of(&mut self, id: Id) -> Result<Option<Id>> {
        if !self.is_proxy(id) {
            return Ok(self.data(id)?.prototype);
        }
        self.frame(|e| {
            // §10.5.1
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "getPrototypeOf")? else {
                return e.get_prototype_of(target);
            };
            let result =
                e.invoke_this(trap, Value::Object(handler), vec![Value::Object(target)])?;
            let proto = match result {
                Value::Object(p) => Some(p),
                Value::Null => None,
                _ => {
                    return e.throw_error(
                        "TypeError",
                        "proxy 'getPrototypeOf' trap returned neither object nor null",
                    )
                }
            };
            e.protect(&result)?;
            if e.is_extensible(target)? {
                return Ok(proto);
            }
            if e.get_prototype_of(target)? != proto {
                return e.proxy_invariant("getPrototypeOf");
            }
            Ok(proto)
        })
    }

    /// Proxy [[SetPrototypeOf]] (§10.5.2).
    pub(super) fn proxy_set_prototype(&mut self, id: Id, proto: Option<Id>) -> Result<bool> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "setPrototypeOf")? else {
                return e.set_prototype(target, proto);
            };
            let v = proto.map(Value::Object).unwrap_or(Value::Null);
            let result =
                e.invoke_this(trap, Value::Object(handler), vec![Value::Object(target), v])?;
            if !result.truthy() {
                return Ok(false);
            }
            if e.is_extensible(target)? {
                return Ok(true);
            }
            if e.get_prototype_of(target)? != proto {
                return e.proxy_invariant("setPrototypeOf");
            }
            Ok(true)
        })
    }

    /// [[IsExtensible]].
    pub(super) fn is_extensible(&mut self, id: Id) -> Result<bool> {
        if !self.is_proxy(id) {
            return Ok(self.data(id)?.extensible);
        }
        self.frame(|e| {
            // §10.5.3
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "isExtensible")? else {
                return e.is_extensible(target);
            };
            let result = e
                .invoke_this(trap, Value::Object(handler), vec![Value::Object(target)])?
                .truthy();
            if result != e.is_extensible(target)? {
                return e.proxy_invariant("isExtensible");
            }
            Ok(result)
        })
    }

    /// [[PreventExtensions]].
    pub(super) fn prevent_extensions(&mut self, id: Id) -> Result<bool> {
        if !self.is_proxy(id) {
            if let Some(typed) = self.data(id)?.typed.clone() {
                // §10.4.5.? TypedArray [[PreventExtensions]]: length-tracking views refuse.
                if !self.typed_fixed(&typed)? {
                    return Ok(false);
                }
            }
            let mut data = self.data(id)?.clone();
            data.extensible = false;
            self.replace_data(id, data)?;
            return Ok(true);
        }
        self.frame(|e| {
            // §10.5.4
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "preventExtensions")? else {
                return e.prevent_extensions(target);
            };
            let result = e
                .invoke_this(trap, Value::Object(handler), vec![Value::Object(target)])?
                .truthy();
            if result && e.is_extensible(target)? {
                return e.proxy_invariant("preventExtensions");
            }
            Ok(result)
        })
    }

    /// Proxy [[GetOwnProperty]] (§10.5.5).
    pub(super) fn proxy_own(&mut self, id: Id, key: &Key) -> Result<Option<Property>> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "getOwnPropertyDescriptor")? else {
                return e.own(target, key);
            };
            let result = e.invoke_this(
                trap,
                Value::Object(handler),
                vec![Value::Object(target), key_value(key)],
            )?;
            e.protect(&result)?;
            if !matches!(result, Value::Object(_) | Value::Undefined) {
                return e.throw_error(
                    "TypeError",
                    "proxy 'getOwnPropertyDescriptor' trap returned neither object nor undefined",
                );
            }
            let target_desc = e.own(target, key)?;
            if result == Value::Undefined {
                let Some(target_desc) = target_desc else {
                    return Ok(None);
                };
                if !target_desc.configurable || !e.is_extensible(target)? {
                    return e.proxy_invariant("getOwnPropertyDescriptor");
                }
                return Ok(None);
            }
            let extensible = e.is_extensible(target)?;
            let desc = e.descriptor(result)?;
            let result = complete(desc.clone());
            if !compatible_descriptor(extensible, &as_descriptor(&result), target_desc.as_ref()) {
                return e.proxy_invariant("getOwnPropertyDescriptor");
            }
            if !result.configurable {
                match &target_desc {
                    None => return e.proxy_invariant("getOwnPropertyDescriptor"),
                    Some(t) if t.configurable => {
                        return e.proxy_invariant("getOwnPropertyDescriptor")
                    }
                    Some(t) => {
                        if desc.writable == Some(false)
                            && matches!(t.kind, Kind::Data { writable: true, .. })
                        {
                            return e.proxy_invariant("getOwnPropertyDescriptor");
                        }
                    }
                }
            }
            Ok(Some(result))
        })
    }

    /// FromPropertyDescriptor (§6.2.6.4) of a possibly partial descriptor.
    fn partial_descriptor_object(&mut self, d: &Descriptor) -> Result<Value> {
        let object = self.intrinsics.as_ref().unwrap().object;
        let id = self.new_object(Some(object))?;
        self.protect(&Value::Object(id))?;
        let fields = [
            ("value", d.value.clone()),
            ("writable", d.writable.map(Value::Bool)),
            ("get", d.get.clone()),
            ("set", d.set.clone()),
            ("enumerable", d.enumerable.map(Value::Bool)),
            ("configurable", d.configurable.map(Value::Bool)),
        ];
        for (name, value) in fields {
            if let Some(value) = value {
                self.put_data(id, name, value)?;
            }
        }
        Ok(Value::Object(id))
    }

    /// Proxy [[DefineOwnProperty]] (§10.5.6).
    pub(super) fn proxy_define(&mut self, id: Id, key: &Key, d: Descriptor) -> Result<bool> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "defineProperty")? else {
                return e.define(target, key, d);
            };
            let object = e.partial_descriptor_object(&d)?;
            let result = e
                .invoke_this(
                    trap,
                    Value::Object(handler),
                    vec![Value::Object(target), key_value(key), object],
                )?
                .truthy();
            if !result {
                return Ok(false);
            }
            let target_desc = e.own(target, key)?;
            let extensible = e.is_extensible(target)?;
            let setting_config_false = d.configurable == Some(false);
            match target_desc {
                None => {
                    if !extensible || setting_config_false {
                        return e.proxy_invariant("defineProperty");
                    }
                }
                Some(t) => {
                    if !compatible_descriptor(extensible, &d, Some(&t))
                        || setting_config_false && t.configurable
                    {
                        return e.proxy_invariant("defineProperty");
                    }
                    if !t.configurable
                        && matches!(t.kind, Kind::Data { writable: true, .. })
                        && d.writable == Some(false)
                    {
                        return e.proxy_invariant("defineProperty");
                    }
                }
            }
            Ok(true)
        })
    }

    /// Proxy [[HasProperty]] (§10.5.7).
    pub(super) fn proxy_has(&mut self, id: Id, key: &Key) -> Result<bool> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "has")? else {
                return e.has(target, key);
            };
            let result = e
                .invoke_this(
                    trap,
                    Value::Object(handler),
                    vec![Value::Object(target), key_value(key)],
                )?
                .truthy();
            if !result {
                if let Some(t) = e.own(target, key)? {
                    if !t.configurable || !e.is_extensible(target)? {
                        return e.proxy_invariant("has");
                    }
                }
            }
            Ok(result)
        })
    }

    /// Proxy [[Get]] (§10.5.8).
    pub(super) fn proxy_get(&mut self, id: Id, key: &Key, receiver: Value) -> Result<Value> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "get")? else {
                return e.ordinary_get(target, key, receiver);
            };
            let result = e.invoke_this(
                trap,
                Value::Object(handler),
                vec![Value::Object(target), key_value(key), receiver],
            )?;
            e.protect(&result)?;
            if let Some(t) = e.own(target, key)? {
                if !t.configurable {
                    match &t.kind {
                        Kind::Data {
                            value,
                            writable: false,
                        } if !result.same_value(value) => return e.proxy_invariant("get"),
                        Kind::Accessor {
                            get: Value::Undefined,
                            ..
                        } if result != Value::Undefined => return e.proxy_invariant("get"),
                        _ => {}
                    }
                }
            }
            Ok(result)
        })
    }

    /// Proxy [[Set]] (§10.5.9).
    pub(super) fn proxy_set(
        &mut self,
        id: Id,
        key: &Key,
        value: Value,
        receiver: Value,
    ) -> Result<bool> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "set")? else {
                return e.ordinary_set(target, key, value, receiver);
            };
            let result = e
                .invoke_this(
                    trap,
                    Value::Object(handler),
                    vec![
                        Value::Object(target),
                        key_value(key),
                        value.clone(),
                        receiver,
                    ],
                )?
                .truthy();
            if !result {
                return Ok(false);
            }
            if let Some(t) = e.own(target, key)? {
                if !t.configurable {
                    match &t.kind {
                        Kind::Data {
                            value: current,
                            writable: false,
                        } if !value.same_value(current) => return e.proxy_invariant("set"),
                        Kind::Accessor {
                            set: Value::Undefined,
                            ..
                        } => return e.proxy_invariant("set"),
                        _ => {}
                    }
                }
            }
            Ok(true)
        })
    }

    /// Proxy [[Delete]] (§10.5.10).
    pub(super) fn proxy_delete(&mut self, id: Id, key: &Key) -> Result<bool> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "deleteProperty")? else {
                return e.delete(target, key);
            };
            let result = e
                .invoke_this(
                    trap,
                    Value::Object(handler),
                    vec![Value::Object(target), key_value(key)],
                )?
                .truthy();
            if !result {
                return Ok(false);
            }
            let Some(t) = e.own(target, key)? else {
                return Ok(true);
            };
            if !t.configurable || !e.is_extensible(target)? {
                return e.proxy_invariant("deleteProperty");
            }
            Ok(true)
        })
    }

    /// Proxy [[OwnPropertyKeys]] (§10.5.11).
    pub(super) fn proxy_own_keys(&mut self, id: Id) -> Result<Vec<Key>> {
        self.frame(|e| {
            let (target, handler) = e.proxy_slots(id)?;
            let Some(trap) = e.proxy_trap(handler, "ownKeys")? else {
                return e.own_keys(target);
            };
            let array = e.invoke_this(trap, Value::Object(handler), vec![Value::Object(target)])?;
            e.protect(&array)?;
            // CreateListFromArrayLike(trapResultArray, property-key) (§7.3.18).
            let Value::Object(array) = array else {
                return e.throw_error("TypeError", "proxy 'ownKeys' trap result is not an object");
            };
            let length = e.length_of(array)?;
            let mut keys: Vec<Key> = vec![];
            for index in 0..length {
                e.tick()?;
                let v = e.get_member(Value::Object(array), index.to_string())?;
                let key = match v {
                    Value::String(s) => Key::String(s.text()?.to_owned()),
                    Value::Symbol(s) => Key::Symbol(s),
                    _ => {
                        return e.throw_error(
                            "TypeError",
                            "proxy 'ownKeys' trap result element is not a property key",
                        )
                    }
                };
                if keys.contains(&key) {
                    return e.throw_error(
                        "TypeError",
                        "proxy 'ownKeys' trap result contains a duplicate key",
                    );
                }
                keys.push(key);
            }
            let extensible = e.is_extensible(target)?;
            let target_keys = e.own_keys(target)?;
            let mut configurable = vec![];
            let mut fixed = vec![];
            for key in target_keys {
                match e.own(target, &key)? {
                    Some(p) if !p.configurable => fixed.push(key),
                    _ => configurable.push(key),
                }
            }
            if extensible && fixed.is_empty() {
                return Ok(keys);
            }
            let mut unchecked = keys.clone();
            for key in fixed {
                let Some(at) = unchecked.iter().position(|k| *k == key) else {
                    return e.proxy_invariant("ownKeys");
                };
                unchecked.remove(at);
            }
            if extensible {
                return Ok(keys);
            }
            for key in configurable {
                let Some(at) = unchecked.iter().position(|k| *k == key) else {
                    return e.proxy_invariant("ownKeys");
                };
                unchecked.remove(at);
            }
            if !unchecked.is_empty() {
                return e.proxy_invariant("ownKeys");
            }
            Ok(keys)
        })
    }

    /// Proxy [[Call]] (§10.5.12).
    pub(super) fn proxy_call(&mut self, id: Id, this: Value, args: Vec<Value>) -> Result<Value> {
        let (target, handler) = self.proxy_slots(id)?;
        let Some(trap) = self.proxy_trap(handler, "apply")? else {
            return self.invoke_this(Value::Object(target), this, args);
        };
        let array = self.array_from_values(args)?;
        self.invoke_this(
            trap,
            Value::Object(handler),
            vec![Value::Object(target), this, array],
        )
    }

    /// Proxy [[Construct]] (§10.5.13).
    pub(super) fn proxy_construct(
        &mut self,
        id: Id,
        args: Vec<Value>,
        new_target: Value,
    ) -> Result<Value> {
        let (target, handler) = self.proxy_slots(id)?;
        let Some(trap) = self.proxy_trap(handler, "construct")? else {
            return self.construct_with_new_target(Value::Object(target), args, new_target);
        };
        let array = self.array_from_values(args)?;
        let result = self.invoke_this(
            trap,
            Value::Object(handler),
            vec![Value::Object(target), array, new_target],
        )?;
        if !matches!(result, Value::Object(_)) {
            return self.throw_error("TypeError", "proxy 'construct' trap returned a non-object");
        }
        Ok(result)
    }
}
