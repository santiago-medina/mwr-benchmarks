//! Ordinary objects: a single descriptor type and transactional graph updates.
use super::vm::{Engine, Object, ScriptError, Value};
use crate::dual::{Domain, Id};
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Key {
    String(String),
    Symbol(u64),
}
impl From<&str> for Key {
    fn from(v: &str) -> Self {
        Self::String(v.into())
    }
}
impl From<String> for Key {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}
impl From<&String> for Key {
    fn from(v: &String) -> Self {
        Self::String(v.clone())
    }
}
impl From<&Key> for Key {
    fn from(v: &Key) -> Self {
        v.clone()
    }
}
impl Key {
    pub fn index(&self) -> Option<u32> {
        if let Self::String(s) = self {
            s.parse::<u32>()
                .ok()
                .filter(|n| *n < u32::MAX && n.to_string() == *s)
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Data { value: Value, writable: bool },
    Accessor { get: Value, set: Value },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Property {
    pub kind: Kind,
    pub enumerable: bool,
    pub configurable: bool,
}
impl Property {
    pub fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        Self {
            kind: Kind::Data { value, writable },
            enumerable,
            configurable,
        }
    }
    fn edges(&self) -> Vec<Id> {
        match &self.kind {
            Kind::Data { value, .. } => value.id().into_iter().collect(),
            Kind::Accessor { get, set } => get.id().into_iter().chain(set.id()).collect(),
        }
    }
}
#[derive(Clone, Default)]
pub(super) struct Descriptor {
    pub value: Option<Value>,
    pub writable: Option<bool>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}
impl Descriptor {
    fn data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }
    fn accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }
}
#[derive(Clone)]
pub(super) struct Data {
    pub prototype: Option<Id>,
    pub properties: Vec<(Key, Property)>,
    pub extensible: bool,
    pub error_class: Option<String>,
    pub boxed: Option<Value>,
    pub source: Option<String>,
    pub date: Option<u64>,
    pub regexp: Option<std::rc::Rc<super::regexp_builtins::Matcher>>,
    pub regexp_cursor: Option<super::regexp_builtins::Cursor>,
    pub buffer: Option<super::buffers::Store>,
    pub view: Option<super::buffers::View>,
    pub typed: Option<super::typed_arrays::Typed>,
    pub enumeration: Option<super::enumeration::Cursor>,
    pub arguments: bool,
    /// Mapped arguments object: index → (environment, binding name) still aliased.
    pub parameter_map: Option<Vec<Option<(Id, String)>>>,
    pub array: bool,
    pub response: Option<super::network::Response>,
    pub request: Option<super::network::Request>,
    pub activation: Option<super::asynchronous::Activation>,
    pub generator: Option<super::generators::Generator>,
    pub promise: Option<super::promises::PromiseState>,
    pub internal: Option<super::promises::Internal>,
    pub cell: Option<super::promises::Cell>,
    pub job: Option<super::promises::Job>,
    pub collection: Option<super::collections::Table>,
    pub collection_cursor: Option<super::collections::Cursor>,
    pub string_cursor: Option<super::strings::Cursor>,
    pub iterator: Option<super::arrays::IteratorState>,
    pub iter_slot: Option<super::iterator_helpers::IterSlot>,
    pub weak: Option<super::weak::WeakSlot>,
    pub dispose: Option<super::disposal::DisposeSlot>,
    /// Module namespace exotic object (§10.4.6): the module record index.
    pub namespace: Option<usize>,
    #[cfg(feature = "temporal")]
    pub temporal: Option<std::rc::Rc<super::temporal::Slot>>,
}
impl Data {
    pub fn new(prototype: Option<Id>) -> Self {
        Self {
            prototype,
            properties: vec![],
            extensible: true,
            namespace: None,
            error_class: None,
            boxed: None,
            source: None,
            date: None,
            regexp: None,
            regexp_cursor: None,
            buffer: None,
            view: None,
            typed: None,
            enumeration: None,
            arguments: false,
            parameter_map: None,
            array: false,
            iterator: None,
            iter_slot: None,
            weak: None,
            dispose: None,
            collection: None,
            collection_cursor: None,
            string_cursor: None,
            activation: None,
            generator: None,
            response: None,
            request: None,
            promise: None,
            internal: None,
            cell: None,
            job: None,
            #[cfg(feature = "temporal")]
            temporal: None,
        }
    }
    fn edges(&self) -> Vec<Id> {
        self.prototype
            .into_iter()
            .chain(self.collection.as_ref().into_iter().flat_map(|t| t.edges()))
            .chain(self.collection_cursor.as_ref().and_then(|c| c.target))
            .chain(self.enumeration.as_ref().and_then(|c| c.current))
            .chain(self.view.as_ref().map(|v| v.buffer))
            .chain(self.typed.as_ref().map(|t| t.view.buffer))
            .chain(self.boxed.as_ref().and_then(Value::id))
            .chain(
                self.parameter_map
                    .iter()
                    .flatten()
                    .flatten()
                    .map(|(env, _)| *env),
            )
            .chain(self.iterator.as_ref().and_then(|i| i.target))
            .chain(self.regexp_cursor.as_ref().map(|c| c.regexp))
            .chain(self.iter_slot.as_ref().into_iter().flat_map(|s| s.edges()))
            .chain(self.weak.as_ref().into_iter().flat_map(|w| w.edges()))
            .chain(self.dispose.as_ref().into_iter().flat_map(|d| d.edges()))
            .chain(self.request.as_ref().into_iter().flat_map(|r| r.edges()))
            .chain(self.activation.as_ref().into_iter().flat_map(|a| a.edges()))
            .chain(self.generator.as_ref().into_iter().flat_map(|g| g.edges()))
            .chain(self.promise.as_ref().into_iter().flat_map(|p| p.edges()))
            .chain(self.internal.as_ref().into_iter().flat_map(|p| p.edges()))
            .chain(self.cell.as_ref().into_iter().flat_map(|p| p.edges()))
            .chain(self.job.as_ref().into_iter().flat_map(|p| p.edges()))
            .chain(self.properties.iter().flat_map(|(_, p)| p.edges()))
            .collect()
    }
    pub fn own(&self, key: impl Into<Key>) -> Option<&Property> {
        let key = key.into();
        self.properties
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, p)| p)
    }
}
/// IsCompatiblePropertyDescriptor (§10.1.6.2): the validation half of
/// ValidateAndApplyPropertyDescriptor (§10.1.6.3).
pub(super) fn compatible_descriptor(
    extensible: bool,
    d: &Descriptor,
    current: Option<&Property>,
) -> bool {
    let Some(old) = current else {
        return extensible;
    };
    if old.configurable {
        return true;
    }
    if d.configurable == Some(true) || d.enumerable.is_some_and(|v| v != old.enumerable) {
        return false;
    }
    let accessor = matches!(old.kind, Kind::Accessor { .. });
    if d.data() && accessor || d.accessor() && !accessor {
        return false;
    }
    match &old.kind {
        Kind::Data {
            value,
            writable: false,
        } => !(d.writable == Some(true) || d.value.as_ref().is_some_and(|v| !v.same_value(value))),
        Kind::Accessor { get, set } => {
            !(d.get.as_ref().is_some_and(|v| !v.same_value(get))
                || d.set.as_ref().is_some_and(|v| !v.same_value(set)))
        }
        _ => true,
    }
}
impl Data {
    pub(super) fn apply(&mut self, key: Key, d: Descriptor) -> bool {
        let old = self.own(&key).cloned();
        if !compatible_descriptor(self.extensible, &d, old.as_ref()) {
            return false;
        }
        let mut p = old.unwrap_or_else(|| Property::data(Value::Undefined, false, false, false));
        if d.accessor() && !matches!(p.kind, Kind::Accessor { .. }) {
            p.kind = Kind::Accessor {
                get: Value::Undefined,
                set: Value::Undefined,
            };
        }
        if d.data() && !matches!(p.kind, Kind::Data { .. }) {
            p.kind = Kind::Data {
                value: Value::Undefined,
                writable: false,
            };
        }
        match &mut p.kind {
            Kind::Data { value, writable } => {
                if let Some(v) = d.value {
                    *value = v;
                }
                if let Some(v) = d.writable {
                    *writable = v;
                }
            }
            Kind::Accessor { get, set } => {
                if let Some(v) = d.get {
                    *get = v;
                }
                if let Some(v) = d.set {
                    *set = v;
                }
            }
        }
        if let Some(v) = d.enumerable {
            p.enumerable = v;
        }
        if let Some(v) = d.configurable {
            p.configurable = v;
        }
        if let Some((_, v)) = self.properties.iter_mut().find(|(k, _)| *k == key) {
            *v = p;
        } else {
            self.properties.push((key, p));
        }
        true
    }
}

impl Engine<'_> {
    pub(super) fn data(&self, id: Id) -> Result<&Data> {
        self.runtime.value(id)?;
        match self.objects.get(&id) {
            Some(
                Object::Ordinary(d)
                | Object::Builtin { data: d, .. }
                | Object::Bound { data: d, .. }
                | Object::Closure { data: d, .. },
            ) => Ok(d),
            _ => Err(ScriptError::Unsupported(
                "ordinary reflection on host objects",
            )),
        }
    }
    pub(super) fn replace_data(&mut self, id: Id, next: Data) -> Result<()> {
        let old = self.data(id)?.edges();
        // Weak column first (WEAK_REFS.md); it fails without mutation.
        self.sync_weak(id, &next)?;
        let mut added = vec![];
        for child in next.edges() {
            if let Err(error) = self.runtime.link(id, child) {
                for a in added {
                    self.runtime.unlink(id, a)?;
                }
                let previous = self.data(id)?.clone();
                self.sync_weak(id, &previous)?;
                return Err(error.into());
            }
            added.push(child);
        }
        for child in old {
            self.runtime.unlink(id, child)?;
        }
        match self.objects.get_mut(&id) {
            Some(
                Object::Ordinary(d)
                | Object::Builtin { data: d, .. }
                | Object::Bound { data: d, .. }
                | Object::Closure { data: d, .. },
            ) => *d = next,
            _ => unreachable!(),
        }
        Ok(())
    }
    pub(super) fn new_object(&mut self, prototype: Option<Id>) -> Result<Id> {
        if let Some(prototype) = prototype {
            self.protect(&Value::Object(prototype))?;
        }
        let id = self.allocate(Domain::Js, -500)?;
        self.objects.insert(id, Object::Ordinary(Data::new(None)));
        self.replace_data(id, Data::new(prototype))?;
        Ok(id)
    }
    /// [[GetOwnProperty]].
    pub(super) fn own(&mut self, id: Id, key: impl Into<Key>) -> Result<Option<Property>> {
        let key = key.into();
        if self.is_proxy(id) {
            return self.proxy_own(id, &key);
        }
        if let Some((m, name)) = self.namespace_key(id, &key)? {
            return self.namespace_own(m, &name);
        }
        if let Some(typed) = &self.data(id)?.typed {
            if let Some(index) = super::typed_arrays::numeric_index(&key) {
                let value = self.typed_get(typed, index)?;
                return Ok(if value == Value::Undefined {
                    None
                } else {
                    Some(Property::data(value, true, true, true))
                });
            }
        }
        let data = self.data(id)?;
        let mut property = data.own(&key).cloned();
        if let (Some(p), Some((env, name))) = (&mut property, self.mapped_slot(data, &key)) {
            if let (Kind::Data { value, .. }, Some(Object::Environment { bindings, .. })) =
                (&mut p.kind, self.objects.get(&env))
            {
                if let Some(Some(v)) = bindings.get(&name) {
                    *value = v.clone();
                }
            }
        }
        Ok(property)
    }
    fn mapped_slot(&self, data: &Data, key: &Key) -> Option<(Id, String)> {
        let index = key.index()? as usize;
        data.parameter_map.as_ref()?.get(index)?.clone()
    }
    fn unmap(&mut self, id: Id, key: &Key) -> Result<()> {
        let mut d = self.data(id)?.clone();
        if let (Some(map), Some(index)) = (&mut d.parameter_map, key.index()) {
            if let Some(slot) = map.get_mut(index as usize) {
                *slot = None;
            }
        }
        self.replace_data(id, d)
    }
    pub(super) fn has(&mut self, mut id: Id, key: impl Into<Key>) -> Result<bool> {
        let key = key.into();
        for _ in 0..10_000 {
            if self.is_proxy(id) {
                return self.proxy_has(id, &key);
            }
            if let Some((m, name)) = self.namespace_key(id, &key)? {
                return Ok(self.namespace_has(m, &name));
            }
            if let Some(typed) = &self.data(id)?.typed {
                if let Some(index) = super::typed_arrays::numeric_index(&key) {
                    return self.typed_valid(typed, index);
                }
            }

            if self.own(id, &key)?.is_some() {
                return Ok(true);
            }
            match self.data(id)?.prototype {
                Some(p) => id = p,
                None => return Ok(false),
            }
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn ordinary_get(
        &mut self,
        id: Id,
        key: impl Into<Key>,
        receiver: Value,
    ) -> Result<Value> {
        let key = key.into();
        let mut at = Some(id);
        for _ in 0..10_000 {
            let Some(id) = at else {
                return Ok(Value::Undefined);
            };
            if self.is_proxy(id) {
                return self.proxy_get(id, &key, receiver);
            }
            if let Some((m, name)) = self.namespace_key(id, &key)? {
                return self.namespace_get(m, &name);
            }
            if let Some(typed) = &self.data(id)?.typed {
                if let Some(index) = super::typed_arrays::numeric_index(&key) {
                    return self.typed_get(typed, index);
                }
            }
            if let Some(p) = self.own(id, &key)? {
                return match p.kind {
                    Kind::Data { value, .. } => Ok(value),
                    Kind::Accessor { get, .. } => {
                        if get == Value::Undefined {
                            Ok(Value::Undefined)
                        } else {
                            self.invoke_this(get, receiver, vec![])
                        }
                    }
                };
            }
            at = self.data(id)?.prototype;
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn define(&mut self, id: Id, key: impl Into<Key>, d: Descriptor) -> Result<bool> {
        let key = key.into();
        if d.data() && d.accessor() {
            return self.throw_error("TypeError", "mixed descriptor");
        }
        if self.is_proxy(id) {
            return self.proxy_define(id, &key, d);
        }
        if let Some((m, name)) = self.namespace_key(id, &key)? {
            return self.namespace_define(m, &name, d);
        }
        if let Some(typed) = self.data(id)?.typed.clone() {
            if let Some(index) = super::typed_arrays::numeric_index(&key) {
                return self.typed_define(&typed, index, d);
            }
        }
        if self.data(id)?.array {
            return self.array_define(id, key, d);
        }
        if let Some((env, name)) = self.mapped_slot(self.data(id)?, &key) {
            // [[DefineOwnProperty]] of a mapped arguments object (10.4.4.2).
            let mut next = d.clone();
            if d.data() && d.value.is_none() && d.writable == Some(false) {
                next.value = Some(
                    self.own(id, &key)?
                        .and_then(|p| match p.kind {
                            Kind::Data { value, .. } => Some(value),
                            _ => None,
                        })
                        .unwrap_or(Value::Undefined),
                );
            }
            let mut data = self.data(id)?.clone();
            if !data.apply(key.clone(), next) {
                return Ok(false);
            }
            self.replace_data(id, data)?;
            if d.accessor() {
                self.unmap(id, &key)?;
            } else {
                if let Some(value) = d.value.clone() {
                    self.initialize(env, &name, value)?;
                }
                if d.writable == Some(false) {
                    self.unmap(id, &key)?;
                }
            }
            return Ok(true);
        }
        let mut data = self.data(id)?.clone();
        let accepted = data.apply(key, d);
        if accepted {
            self.replace_data(id, data)?;
        }
        Ok(accepted)
    }

    pub(super) fn put_data(&mut self, id: Id, key: impl Into<Key>, value: Value) -> Result<()> {
        let key = key.into();
        if self.define(
            id,
            &key,
            Descriptor {
                value: Some(value),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )? {
            Ok(())
        } else {
            self.throw_error("TypeError", "property definition refused")
        }
    }
    pub(super) fn ordinary_set(
        &mut self,
        id: Id,
        key: impl Into<Key>,
        value: Value,
        receiver: Value,
    ) -> Result<bool> {
        let key = key.into();
        let mut at = Some(id);
        let mut found = None;
        for _ in 0..10_000 {
            let Some(id) = at else { break };
            if self.is_proxy(id) {
                return self.proxy_set(id, &key, value, receiver);
            }
            if self.data(id)?.namespace.is_some() {
                return Ok(false); // §10.4.6.9
            }
            if let Some(typed) = self.data(id)?.typed.clone() {
                if let Some(index) = super::typed_arrays::numeric_index(&key) {
                    if receiver == Value::Object(id) {
                        self.typed_write(&typed, index, value)?;
                        return Ok(true);
                    }
                    if !self.typed_valid(&typed, index)? {
                        return Ok(true);
                    }
                }
            }
            if let Some(p) = self.own(id, &key)? {
                found = Some(p);
                break;
            }
            at = self.data(id)?.prototype;
        }
        if found.is_none() && at.is_some() {
            return Err(ScriptError::Runtime("prototype traversal limit"));
        }
        match found.map(|p| p.kind).unwrap_or(Kind::Data {
            value: Value::Undefined,
            writable: true,
        }) {
            Kind::Accessor { set, .. } => {
                if set == Value::Undefined {
                    Ok(false)
                } else {
                    self.invoke_this(set, receiver, vec![value])?;
                    Ok(true)
                }
            }
            Kind::Data {
                writable: false, ..
            } => Ok(false),
            Kind::Data { .. } => {
                let Value::Object(target) = receiver else {
                    return Ok(false);
                };
                if let Some(p) = self.own(target, &key)? {
                    if !matches!(p.kind, Kind::Data { writable: true, .. }) {
                        return Ok(false);
                    }
                    self.define(
                        target,
                        &key,
                        Descriptor {
                            value: Some(value),
                            ..Descriptor::default()
                        },
                    )
                } else {
                    self.define(
                        target,
                        &key,
                        Descriptor {
                            value: Some(value),
                            writable: Some(true),
                            enumerable: Some(true),
                            configurable: Some(true),
                            ..Descriptor::default()
                        },
                    )
                }
            }
        }
    }
    pub(super) fn delete(&mut self, id: Id, key: impl Into<Key>) -> Result<bool> {
        let key = key.into();
        if self.is_proxy(id) {
            return self.proxy_delete(id, &key);
        }
        if let Some((m, name)) = self.namespace_key(id, &key)? {
            return Ok(!self.namespace_has(m, &name));
        }
        if let Some(typed) = &self.data(id)?.typed {
            if let Some(index) = super::typed_arrays::numeric_index(&key) {
                return self.typed_valid(typed, index).map(|valid| !valid);
            }
        }
        if self.own(id, &key)?.is_some_and(|p| !p.configurable) {
            return Ok(false);
        }
        let mut d = self.data(id)?.clone();
        d.properties.retain(|(k, _)| *k != key);
        if let (Some(map), Some(index)) = (&mut d.parameter_map, key.index()) {
            if let Some(slot) = map.get_mut(index as usize) {
                *slot = None;
            }
        }
        self.replace_data(id, d)?;
        Ok(true)
    }
    /// [[SetPrototypeOf]] (OrdinarySetPrototypeOf, §10.1.2.1).
    pub(super) fn set_prototype(&mut self, id: Id, prototype: Option<Id>) -> Result<bool> {
        if self.is_proxy(id) {
            return self.proxy_set_prototype(id, prototype);
        }
        if self.data(id)?.prototype == prototype {
            return Ok(true);
        }
        // %Object.prototype% is an immutable prototype exotic object (§10.4.7.1
        // SetImmutablePrototype): only its current [[Prototype]] is accepted.
        if self.intrinsics.as_ref().is_some_and(|i| i.object == id) {
            return Ok(false);
        }
        if !self.data(id)?.extensible {
            return Ok(false);
        }
        let mut at = prototype;
        for _ in 0..10_000 {
            let Some(p) = at else {
                let mut d = self.data(id)?.clone();
                d.prototype = prototype;
                self.replace_data(id, d)?;
                return Ok(true);
            };
            if p == id {
                return Ok(false);
            }
            // Step 8.c.i: a proxy parent ends the cycle check.
            at = if self.is_proxy(p) {
                None
            } else {
                self.data(p)?.prototype
            };
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn descriptor(&mut self, v: Value) -> Result<Descriptor> {
        let Value::Object(id) = v else {
            return self.throw_error("TypeError", "descriptor must be an object");
        };
        let mut d = Descriptor::default();
        for key in [
            "enumerable",
            "configurable",
            "value",
            "writable",
            "get",
            "set",
        ] {
            if self.has(id, key)? {
                let v = self.get_member(Value::Object(id), key)?;
                self.protect(&v)?;
                match key {
                    "enumerable" => d.enumerable = Some(v.truthy()),
                    "configurable" => d.configurable = Some(v.truthy()),
                    "writable" => d.writable = Some(v.truthy()),
                    "value" => d.value = Some(v),
                    "get" | "set" => {
                        if v != Value::Undefined && !self.callable(&v) {
                            return self.throw_error("TypeError", "accessor is not callable");
                        }
                        if key == "get" {
                            d.get = Some(v);
                        } else {
                            d.set = Some(v);
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        if d.data() && d.accessor() {
            return self.throw_error("TypeError", "mixed descriptor");
        }
        Ok(d)
    }
    pub(super) fn descriptor_object(&mut self, p: Property) -> Result<Value> {
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        match p.kind {
            Kind::Data { value, writable } => {
                self.put_data(id, "value", value)?;
                self.put_data(id, "writable", Value::Bool(writable))?;
            }
            Kind::Accessor { get, set } => {
                self.put_data(id, "get", get)?;
                self.put_data(id, "set", set)?;
            }
        }
        self.put_data(id, "enumerable", Value::Bool(p.enumerable))?;
        self.put_data(id, "configurable", Value::Bool(p.configurable))?;
        Ok(Value::Object(id))
    }
    /// [[OwnPropertyKeys]].
    pub(super) fn own_keys(&mut self, id: Id) -> Result<Vec<Key>> {
        if self.is_proxy(id) {
            return self.proxy_own_keys(id);
        }
        if let Some(m) = self.data(id)?.namespace {
            return self.namespace_own_keys(id, m);
        }
        if let Some(typed) = &self.data(id)?.typed {
            let length = self.typed_length(typed)?.unwrap_or(0);
            let mut keys: Vec<_> = (0..length)
                .map(|index| Key::from(index.to_string()))
                .collect();
            keys.extend(
                self.data(id)?
                    .properties
                    .iter()
                    .filter(|(key, _)| matches!(key, Key::String(_)))
                    .map(|(key, _)| key.clone()),
            );
            keys.extend(
                self.data(id)?
                    .properties
                    .iter()
                    .filter(|(key, _)| matches!(key, Key::Symbol(_)) && !self.is_private(key))
                    .map(|(key, _)| key.clone()),
            );
            return Ok(keys);
        }
        let mut indices = vec![];
        let mut strings = vec![];
        let mut symbols = vec![];
        for (k, _) in &self.data(id)?.properties {
            if let Some(n) = k.index() {
                indices.push((n, k.clone()));
            } else {
                match k {
                    Key::String(_) => strings.push(k.clone()),
                    Key::Symbol(_) if self.is_private(k) => {}
                    Key::Symbol(_) => symbols.push(k.clone()),
                }
            }
        }
        indices.sort_by_key(|(n, _)| *n);
        Ok(indices
            .into_iter()
            .map(|(_, k)| k)
            .chain(strings)
            .chain(symbols)
            .collect())
    }
}
