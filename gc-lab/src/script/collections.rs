//! Ordered collection slots use ordinary transactional data/GC edge updates.
use super::{
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct Table {
    pub set: bool,
    pub entries: Vec<Option<(Value, Value)>>,
}
impl Table {
    pub fn edges(&self) -> Vec<Id> {
        self.entries
            .iter()
            .flatten()
            .flat_map(|(k, v)| [k.id(), v.id()])
            .flatten()
            .collect()
    }
    fn find(&self, key: &Value) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|(k, _)| k.same_value_zero(key)))
    }
}
#[derive(Clone)]
pub(super) struct Cursor {
    pub target: Option<Id>,
    pub index: usize,
    pub set: bool,
    pub kind: u8,
}
impl Engine<'_> {
    pub(super) fn bootstrap_collections(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
        iterator: Id,
    ) -> Result<()> {
        let function = self.intrinsics.as_ref().unwrap().function;
        for name in ["Map", "Set"] {
            let proto = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
            self.runtime.link(env, proto)?;
            objects.insert(format!("{name}.prototype"), proto);
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert(name.into(), proto);
            let ctor = objects[name];
            self.define(
                ctor,
                "length",
                Descriptor {
                    value: Some(Value::number(0.0)),
                    ..Descriptor::default()
                },
            )?;
            self.define(
                proto,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String(name.into())),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
            let species =
                self.intrinsic_function(env, &format!("{name}.species"), Some(function))?;
            self.define(
                ctor,
                Key::Symbol(2),
                Descriptor {
                    get: Some(Value::Object(species)),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
            let methods: &[&str] = if name == "Map" {
                &[
                    "get", "set", "has", "delete", "clear", "forEach", "keys", "values", "entries",
                    "size",
                ]
            } else {
                &[
                    "add", "has", "delete", "clear", "forEach", "values", "entries", "size",
                ]
            };
            let mut functions = HashMap::new();
            for &method in methods {
                let f = self.intrinsic_function(
                    env,
                    &format!("{name}.prototype.{method}"),
                    Some(function),
                )?;
                let length = if method == "set" {
                    2
                } else if matches!(method, "clear" | "keys" | "values" | "entries" | "size") {
                    0
                } else {
                    1
                };
                self.define(
                    f,
                    "length",
                    Descriptor {
                        value: Some(Value::number(length as f64)),
                        ..Descriptor::default()
                    },
                )?;
                if method == "size" {
                    self.define(
                        f,
                        "name",
                        Descriptor {
                            value: Some(Value::String("get size".into())),
                            ..Descriptor::default()
                        },
                    )?;
                    self.define(
                        proto,
                        method,
                        Descriptor {
                            get: Some(Value::Object(f)),
                            configurable: Some(true),
                            ..Descriptor::default()
                        },
                    )?;
                } else {
                    self.hidden(proto, method, Value::Object(f))?;
                }
                functions.insert(method, f);
            }
            let default = functions[if name == "Map" { "entries" } else { "values" }];
            self.hidden(proto, Key::Symbol(0), Value::Object(default))?;
            if name == "Set" {
                self.hidden(proto, "keys", Value::Object(functions["values"]))?;
            }
            let p = self.new_object(Some(iterator))?;
            self.runtime.link(env, p)?;
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert(format!("{name}Iterator"), p);
            let next = self.intrinsic_function(
                env,
                &format!("%{name}IteratorPrototype%.next"),
                Some(function),
            )?;
            self.define(
                next,
                "length",
                Descriptor {
                    value: Some(Value::number(0.0)),
                    ..Descriptor::default()
                },
            )?;
            self.hidden(p, "next", Value::Object(next))?;
            self.define(
                p,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String(format!("{name} Iterator"))),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
        }
        Ok(())
    }
    pub(super) fn consume_iterable(
        &mut self,
        source: Value,
        accept: impl FnMut(&mut Self, Value) -> Result<()>,
    ) -> Result<()> {
        let method = self.get_member(source.clone(), Key::Symbol(0))?;
        self.protect(&method)?;
        self.consume_iterator(source, method, accept)
    }
    pub(super) fn consume_iterator(
        &mut self,
        source: Value,
        method: Value,
        mut accept: impl FnMut(&mut Self, Value) -> Result<()>,
    ) -> Result<()> {
        let iterator = self.invoke_this(method, source, vec![])?;
        self.protect(&iterator)?;
        if !matches!(iterator, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator is not an object");
        }
        let next = self.get_member(iterator.clone(), "next")?;
        self.protect(&next)?;
        loop {
            self.tick()?;
            let value = self.frame(|e| {
                let step = e.invoke_this(next.clone(), iterator.clone(), vec![])?;
                e.protect(&step)?;
                if !matches!(step, Value::Object(_)) {
                    return e.throw_error("TypeError", "iterator result is not an object");
                }
                if e.get_member(step.clone(), "done")?.truthy() {
                    return Ok(None);
                }
                let value = e.get_member(step, "value")?;
                e.protect(&value)?;
                match accept(e, value) {
                    Ok(()) => Ok(Some(())),
                    Err(error) => match e.close_iterator(iterator.clone(), Err(error)) {
                        Err(error) => Err(error),
                        _ => unreachable!(),
                    },
                }
            })?;
            if value.is_none() {
                return Ok(());
            }
        }
    }
    pub(super) fn construct_collection(
        &mut self,
        name: &str,
        new_target: Value,
        source: Value,
    ) -> Result<Value> {
        let p = self.get_member(new_target, "prototype")?;
        let proto = if let Value::Object(p) = p {
            p
        } else {
            self.intrinsics.as_ref().unwrap().collection[name]
        };
        let id = self.new_object(Some(proto))?;
        let result = Value::Object(id);
        self.protect(&result)?;
        let mut data = self.data(id)?.clone();
        data.collection = Some(Table {
            set: name == "Set",
            entries: vec![],
        });
        self.replace_data(id, data)?;
        if matches!(source, Value::Null | Value::Undefined) {
            return Ok(result);
        }
        let adder = self.get_member(result.clone(), if name == "Map" { "set" } else { "add" })?;
        self.protect(&adder)?;
        if !self.callable(&adder) {
            return self.throw_error("TypeError", "collection adder is not callable");
        }
        self.consume_iterable(source, |e, value| {
            let args = if name == "Map" {
                if !matches!(value, Value::Object(_)) {
                    return e.throw_error("TypeError", "Map entry is not an object");
                }
                let key = e.get_member(value.clone(), "0")?;
                e.protect(&key)?;
                let value = e.get_member(value, "1")?;
                vec![key, value]
            } else {
                vec![value]
            };
            e.invoke_this(adder.clone(), result.clone(), args)?;
            Ok(())
        })?;
        Ok(result)
    }
    pub(super) fn collection_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if name.ends_with(".species") {
            return Ok(receiver);
        }
        if name == "Map" || name == "Set" {
            return self.throw_error("TypeError", "collection constructor requires new");
        }
        let set = name.starts_with("Set.") || name.starts_with("%Set");
        let Value::Object(id) = receiver.clone() else {
            return self.throw_error("TypeError", "invalid collection receiver");
        };
        if name.starts_with('%') {
            let state = self.data(id).ok().and_then(|d| d.collection_cursor.clone());
            let Some(mut state) = state.filter(|s| s.set == set) else {
                return self.throw_error("TypeError", "invalid collection iterator receiver");
            };
            let Some(target) = state.target else {
                return self.iterator_result(Value::Undefined, true);
            };
            let mut selected = None;
            while let Some(entry) = self
                .data(target)?
                .collection
                .as_ref()
                .unwrap()
                .entries
                .get(state.index)
            {
                state.index += 1;
                if let Some(entry) = entry {
                    selected = Some(entry.clone());
                    break;
                }
            }
            if selected.is_none() {
                state.target = None;
            }
            let mut data = self.data(id)?.clone();
            data.collection_cursor = Some(state.clone());
            self.replace_data(id, data)?;
            let Some((key, value)) = selected else {
                return self.iterator_result(Value::Undefined, true);
            };
            self.protect(&key)?;
            self.protect(&value)?;
            let result = match state.kind {
                0 => key,
                1 => value,
                _ => {
                    let pair = self.new_array(2)?;
                    self.protect(&Value::Object(pair))?;
                    self.put_data(pair, "0", key)?;
                    self.put_data(pair, "1", value)?;
                    Value::Object(pair)
                }
            };
            return self.iterator_result(result, false);
        }
        let Some(table) = self
            .data(id)
            .ok()
            .and_then(|d| d.collection.as_ref())
            .filter(|t| t.set == set)
        else {
            return self.throw_error("TypeError", "incompatible collection receiver");
        };
        let op = name.rsplit('.').next().unwrap();
        let key = args.first().cloned().unwrap_or(Value::Undefined);
        let at = table.find(&key);
        match op {
            "size" => Ok(Value::number(table.entries.iter().flatten().count() as f64)),
            "has" => Ok(Value::Bool(at.is_some())),
            "get" => Ok(at
                .map(|i| table.entries[i].as_ref().unwrap().1.clone())
                .unwrap_or(Value::Undefined)),
            "set" | "add" => {
                let key = if matches!(&key,Value::Number(n)if f64::from_bits(*n)==0.0) {
                    Value::number(0.0)
                } else {
                    key
                };
                let value = if set {
                    key.clone()
                } else {
                    args.get(1).cloned().unwrap_or(Value::Undefined)
                };
                let mut data = self.data(id)?.clone();
                let table = data.collection.as_mut().unwrap();
                if let Some(at) = at {
                    table.entries[at] = Some((key, value));
                } else {
                    table.entries.push(Some((key, value)));
                }
                self.replace_data(id, data)?;
                Ok(receiver)
            }
            "delete" | "clear" => {
                let mut data = self.data(id)?.clone();
                let table = data.collection.as_mut().unwrap();
                if op == "clear" {
                    for entry in &mut table.entries {
                        *entry = None;
                    }
                } else if let Some(at) = at {
                    table.entries[at] = None;
                }
                self.replace_data(id, data)?;
                Ok(if op == "clear" {
                    Value::Undefined
                } else {
                    Value::Bool(at.is_some())
                })
            }
            "keys" | "values" | "entries" => {
                let proto = self.intrinsics.as_ref().unwrap().collection
                    [if set { "SetIterator" } else { "MapIterator" }];
                let cursor = self.new_object(Some(proto))?;
                let mut data = self.data(cursor)?.clone();
                data.collection_cursor = Some(Cursor {
                    target: Some(id),
                    index: 0,
                    set,
                    kind: if op == "keys" {
                        0
                    } else if op == "values" {
                        1
                    } else {
                        2
                    },
                });
                self.replace_data(cursor, data)?;
                Ok(Value::Object(cursor))
            }
            "forEach" => {
                let callback = key;
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "collection callback is not callable");
                }
                let this = args.get(1).cloned().unwrap_or(Value::Undefined);
                let mut index = 0;
                loop {
                    let entry = self
                        .data(id)?
                        .collection
                        .as_ref()
                        .unwrap()
                        .entries
                        .get(index)
                        .cloned();
                    let Some(entry) = entry else {
                        break;
                    };
                    index += 1;
                    self.tick()?;
                    if let Some((key, value)) = entry {
                        self.frame(|e| {
                            e.protect(&key)?;
                            e.protect(&value)?;
                            e.invoke_this(
                                callback.clone(),
                                this.clone(),
                                vec![value, key, receiver.clone()],
                            )?;
                            Ok(())
                        })?;
                    }
                }
                Ok(Value::Undefined)
            }
            _ => Err(ScriptError::Unsupported("collection operation")),
        }
    }
}
