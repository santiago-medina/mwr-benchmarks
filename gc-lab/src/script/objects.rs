//! Ordinary objects: a single descriptor type and transactional graph updates.
use super::vm::{Engine, Object, ScriptError, Value};
use crate::dual::{Domain, Id};
type Result<T> = std::result::Result<T, ScriptError>;
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
    pub properties: Vec<(String, Property)>,
    pub extensible: bool,
    pub error_class: Option<String>,
    pub boxed: Option<Value>,
    pub arguments: bool,
}
impl Data {
    pub fn new(prototype: Option<Id>) -> Self {
        Self {
            prototype,
            properties: vec![],
            extensible: true,
            error_class: None,
            boxed: None,
            arguments: false,
        }
    }
    fn edges(&self) -> Vec<Id> {
        self.prototype
            .into_iter()
            .chain(self.boxed.as_ref().and_then(Value::id))
            .chain(self.properties.iter().flat_map(|(_, p)| p.edges()))
            .collect()
    }
    pub fn own(&self, key: &str) -> Option<&Property> {
        self.properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, p)| p)
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
        let mut added = vec![];
        for child in next.edges() {
            if let Err(error) = self.runtime.link(id, child) {
                for a in added {
                    self.runtime.unlink(id, a)?;
                }
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
        let id = self.allocate(Domain::Js, -500)?;
        self.objects.insert(id, Object::Ordinary(Data::new(None)));
        self.replace_data(id, Data::new(prototype))?;
        Ok(id)
    }
    pub(super) fn own(&self, id: Id, key: &str) -> Result<Option<Property>> {
        Ok(self.data(id)?.own(key).cloned())
    }
    pub(super) fn has(&self, mut id: Id, key: &str) -> Result<bool> {
        for _ in 0..10_000 {
            if self.own(id, key)?.is_some() {
                return Ok(true);
            }
            match self.data(id)?.prototype {
                Some(p) => id = p,
                None => return Ok(false),
            }
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn ordinary_get(&mut self, id: Id, key: &str, receiver: Value) -> Result<Value> {
        let mut at = Some(id);
        for _ in 0..10_000 {
            let Some(id) = at else {
                return Ok(Value::Undefined);
            };
            if let Some(p) = self.own(id, key)? {
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
    pub(super) fn define(&mut self, id: Id, key: &str, d: Descriptor) -> Result<bool> {
        if d.data() && d.accessor() {
            return self.throw_error("TypeError", "mixed descriptor");
        }
        let mut data = self.data(id)?.clone();
        let old = data.own(key).cloned();
        if old.is_none() && !data.extensible {
            return Ok(false);
        }
        if let Some(old) = &old {
            if !old.configurable {
                if d.configurable == Some(true) || d.enumerable.is_some_and(|v| v != old.enumerable)
                {
                    return Ok(false);
                }
                let accessor = matches!(old.kind, Kind::Accessor { .. });
                if d.data() && accessor || d.accessor() && !accessor {
                    return Ok(false);
                }
                match &old.kind {
                    Kind::Data {
                        value,
                        writable: false,
                    } => {
                        if d.writable == Some(true)
                            || d.value.as_ref().is_some_and(|v| !v.same_value(value))
                        {
                            return Ok(false);
                        }
                    }
                    Kind::Accessor { get, set } => {
                        if d.get.as_ref().is_some_and(|v| !v.same_value(get))
                            || d.set.as_ref().is_some_and(|v| !v.same_value(set))
                        {
                            return Ok(false);
                        }
                    }
                    _ => {}
                }
            }
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
        if let Some((_, v)) = data.properties.iter_mut().find(|(k, _)| k == key) {
            *v = p;
        } else {
            data.properties.push((key.into(), p));
        }
        self.replace_data(id, data)?;
        Ok(true)
    }
    pub(super) fn put_data(&mut self, id: Id, key: &str, value: Value) -> Result<()> {
        if self.define(
            id,
            key,
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
        key: &str,
        value: Value,
        receiver: Value,
    ) -> Result<bool> {
        let mut at = Some(id);
        let mut found = None;
        for _ in 0..10_000 {
            let Some(id) = at else { break };
            if let Some(p) = self.own(id, key)? {
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
                if let Some(p) = self.own(target, key)? {
                    if !matches!(p.kind, Kind::Data { writable: true, .. }) {
                        return Ok(false);
                    }
                    self.define(
                        target,
                        key,
                        Descriptor {
                            value: Some(value),
                            ..Descriptor::default()
                        },
                    )
                } else {
                    self.define(
                        target,
                        key,
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
    pub(super) fn delete(&mut self, id: Id, key: &str) -> Result<bool> {
        if self.own(id, key)?.is_some_and(|p| !p.configurable) {
            return Ok(false);
        }
        let mut d = self.data(id)?.clone();
        d.properties.retain(|(k, _)| k != key);
        self.replace_data(id, d)?;
        Ok(true)
    }
    pub(super) fn set_prototype(&mut self, id: Id, prototype: Option<Id>) -> Result<bool> {
        if self.data(id)?.prototype == prototype {
            return Ok(true);
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
            at = self.data(p)?.prototype;
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
    pub(super) fn own_keys(&self, id: Id) -> Result<Vec<String>> {
        let mut numbers = vec![];
        let mut names = vec![];
        for (k, _) in &self.data(id)?.properties {
            if let Ok(n) = k.parse::<u32>() {
                if n < u32::MAX && n.to_string() == *k {
                    numbers.push((n, k.clone()));
                    continue;
                }
            }
            names.push(k.clone());
        }
        numbers.sort_by_key(|(n, _)| *n);
        Ok(numbers.into_iter().map(|(_, k)| k).chain(names).collect())
    }
}
