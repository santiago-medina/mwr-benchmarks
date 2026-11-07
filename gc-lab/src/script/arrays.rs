//! Array exotics share descriptor validation and transactional GC edges with ordinary objects.
use super::{
    objects::{Descriptor, Key, Kind, Property},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct IteratorState {
    pub target: Option<Id>,
    pub index: u64,
    pub kind: u8,
}
impl Engine<'_> {
    pub(super) fn new_array(&mut self, length: u32) -> Result<Id> {
        let proto = self.intrinsics.as_ref().unwrap().array;
        self.array_with_proto(length, proto)
    }
    pub(super) fn array_with_proto(&mut self, length: u32, proto: Id) -> Result<Id> {
        let id = self.new_object(Some(proto))?;
        let mut d = self.data(id)?.clone();
        d.array = true;
        d.properties.push((
            Key::from("length"),
            Property::data(Value::number(length as f64), true, false, false),
        ));
        self.replace_data(id, d)?;
        Ok(id)
    }
    pub(super) fn array_define(&mut self, id: Id, key: Key, mut desc: Descriptor) -> Result<bool> {
        // Coercion may execute arbitrary script: obtain the current descriptor AFTER it.
        if key == Key::from("length") {
            if let Some(value) = &desc.value {
                let a = self.to_number(value.clone())?;
                let b = self.to_number(value.clone())?;
                let uint = if !a.is_finite() || a == 0.0 {
                    0
                } else {
                    a.trunc().rem_euclid(4294967296.0) as u32
                };
                if b != uint as f64 {
                    return self.throw_error("RangeError", "invalid array length");
                }
                desc.value = Some(Value::number(uint as f64));
            }
        }
        let mut data = self.data(id)?.clone();
        let length = data.own("length").unwrap().clone();
        let Kind::Data {
            value: Value::Number(old),
            writable,
        } = length.kind
        else {
            unreachable!()
        };
        let old = f64::from_bits(old) as u32;
        let mut accepted = true;
        if key == Key::from("length") {
            let new = if let Some(Value::Number(n)) = desc.value {
                f64::from_bits(n) as u32
            } else {
                old
            };
            if new < old {
                if !writable {
                    return Ok(false);
                }
                let final_readonly = desc.writable == Some(false);
                if final_readonly {
                    desc.writable = Some(true);
                }
                if !data.apply(key.clone(), desc) {
                    return Ok(false);
                }
                let mut indices: Vec<_> = data
                    .properties
                    .iter()
                    .filter_map(|(k, _)| k.index().filter(|i| *i >= new).map(|i| (i, k.clone())))
                    .collect();
                indices.sort_by(|a, b| b.0.cmp(&a.0));
                for (index, k) in indices {
                    if !data.own(&k).unwrap().configurable {
                        data.apply(
                            Key::from("length"),
                            Descriptor {
                                value: Some(Value::number(index as f64 + 1.0)),
                                ..Descriptor::default()
                            },
                        );
                        accepted = false;
                        break;
                    }
                    data.properties.retain(|(key, _)| *key != k);
                }
                if final_readonly {
                    data.apply(
                        Key::from("length"),
                        Descriptor {
                            writable: Some(false),
                            ..Descriptor::default()
                        },
                    );
                }
            } else if !data.apply(key, desc) {
                return Ok(false);
            }
        } else {
            if let Some(index) = key.index() {
                if index >= old && !writable {
                    return Ok(false);
                }
                if !data.apply(key, desc) {
                    return Ok(false);
                }
                if index >= old {
                    data.apply(
                        Key::from("length"),
                        Descriptor {
                            value: Some(Value::number(index as f64 + 1.0)),
                            ..Descriptor::default()
                        },
                    );
                }
            } else if !data.apply(key, desc) {
                return Ok(false);
            }
        }
        self.replace_data(id, data)?;
        Ok(accepted)
    }
    pub(super) fn length_of(&mut self, id: Id) -> Result<u64> {
        let v = self.get_member(Value::Object(id), "length")?;
        self.protect(&v)?;
        let n = self.to_number(v)?;
        Ok(if n.is_nan() || n <= 0.0 {
            0
        } else {
            n.floor().min(9007199254740991.0) as u64
        })
    }
    fn array_receiver(&mut self, v: Value) -> Result<Id> {
        self.object_coercible(v)
    }
    pub(super) fn array_iterator(&mut self, target: Value, kind: u8) -> Result<Value> {
        let target = self.array_receiver(target)?;
        self.protect(&Value::Object(target))?;
        let p = self.intrinsics.as_ref().unwrap().array_iterator;
        let id = self.new_object(Some(p))?;
        let mut d = self.data(id)?.clone();
        d.iterator = Some(IteratorState {
            target: Some(target),
            index: 0,
            kind,
        });
        self.replace_data(id, d)?;
        Ok(Value::Object(id))
    }
    fn iterator_result(&mut self, value: Value, done: bool) -> Result<Value> {
        self.protect(&value)?;
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        self.put_data(id, "value", value)?;
        self.put_data(id, "done", Value::Bool(done))?;
        Ok(Value::Object(id))
    }
    fn array_species_create(&mut self, original: Id, len: u32) -> Result<Id> {
        let mut constructor = Value::Undefined;
        if self.data(original)?.array {
            constructor = self.get_member(Value::Object(original), "constructor")?;
            self.protect(&constructor)?;
            if matches!(constructor, Value::Object(_)) {
                constructor = self.get_member(constructor, Key::Symbol(2))?;
                self.protect(&constructor)?;
                if constructor == Value::Null {
                    constructor = Value::Undefined;
                }
            }
        }
        if constructor == Value::Undefined {
            return self.new_array(len);
        }
        let value = self.construct(constructor, vec![Value::number(len as f64)])?;
        let Value::Object(id) = value else {
            unreachable!()
        };
        Ok(id)
    }
    fn next_present_index(&self, mut id: Id, start: u64, end: u64) -> Result<Option<u64>> {
        let mut found = None;
        for _ in 0..10000 {
            let data = self.data(id)?;
            for (key, _) in &data.properties {
                if let Key::String(s) = key {
                    if let Ok(index) = s.parse::<u64>() {
                        if index >= start && index < end && index.to_string() == *s {
                            found = Some(found.map_or(index, |old: u64| old.min(index)));
                        }
                    }
                }
            }
            match data.prototype {
                Some(p) => id = p,
                None => return Ok(found),
            }
        }
        Err(ScriptError::Runtime("prototype traversal limit"))
    }
    pub(super) fn array_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "Symbol" => {
                if !args.is_empty() && arg(0) != Value::Undefined {
                    return Err(ScriptError::Unsupported("symbol descriptions"));
                }
                let id = self.next_symbol;
                self.next_symbol += 1;
                return Ok(Value::Symbol(id));
            }
            "Array" => {
                let single = matches!(args.as_slice(), [Value::Number(_)]);
                let length = if single {
                    let Value::Number(n) = arg(0) else {
                        unreachable!()
                    };
                    let n = f64::from_bits(n);
                    if n < 0.0 || n > u32::MAX as f64 || n.fract() != 0.0 || !n.is_finite() {
                        return self.throw_error("RangeError", "invalid array length");
                    }
                    n as u32
                } else {
                    args.len() as u32
                };
                let id = self.new_array(length)?;
                self.protect(&Value::Object(id))?;
                if !single {
                    for (i, v) in args.into_iter().enumerate() {
                        self.put_data(id, i.to_string(), v)?;
                    }
                }
                return Ok(Value::Object(id));
            }
            "Array.species" => return Ok(receiver),
            "Array.isArray" => {
                return Ok(Value::Bool(
                    matches!(arg(0),Value::Object(id)if self.data(id)?.array),
                ))
            }
            "Array.prototype.values" => return self.array_iterator(receiver, 1),
            "Array.prototype.keys" => return self.array_iterator(receiver, 0),
            "Array.prototype.entries" => return self.array_iterator(receiver, 2),
            "%IteratorPrototype%.iterator" => return Ok(receiver),
            "%ArrayIteratorPrototype%.next" => {
                let Value::Object(id) = receiver else {
                    return self.throw_error("TypeError", "invalid iterator receiver");
                };
                let Some(state) = self.data(id)?.iterator.clone() else {
                    return self.throw_error("TypeError", "invalid iterator receiver");
                };
                let Some(target) = state.target else {
                    return self.iterator_result(Value::Undefined, true);
                };
                let len = self.length_of(target)?;
                let mut d = self.data(id)?.clone();
                let next = d.iterator.as_mut().unwrap();
                if state.index >= len {
                    next.target = None;
                    self.replace_data(id, d)?;
                    return self.iterator_result(Value::Undefined, true);
                }
                next.index = state.index + 1;
                self.replace_data(id, d)?;
                let value = if state.kind == 0 {
                    Value::number(state.index as f64)
                } else {
                    let v = self.get_member(Value::Object(target), state.index.to_string())?;
                    self.protect(&v)?;
                    if state.kind == 1 {
                        v
                    } else {
                        let pair = self.new_array(2)?;
                        self.protect(&Value::Object(pair))?;
                        self.put_data(pair, "0", Value::number(state.index as f64))?;
                        self.put_data(pair, "1", v)?;
                        Value::Object(pair)
                    }
                };
                return self.iterator_result(value, false);
            }
            "Array.prototype.toString" => {
                let id = self.array_receiver(receiver)?;
                self.protect(&Value::Object(id))?;
                let f = self.get_member(Value::Object(id), "join")?;
                self.protect(&f)?;
                return if self.callable(&f) {
                    self.invoke_this(f, Value::Object(id), vec![])
                } else {
                    self.builtin("Object.prototype.toString", Value::Object(id), vec![])
                };
            }
            _ => {}
        }
        let id = self.array_receiver(receiver)?;
        self.protect(&Value::Object(id))?;
        let length = self.length_of(id)?;
        match name {
            "Array.prototype.push" => {
                if length + args.len() as u64 > 9007199254740991 {
                    return self.throw_error("TypeError", "length exceeds safe integer");
                }
                let mut len = length;
                for value in args {
                    self.set_member(Value::Object(id), len.to_string(), value)?;
                    len += 1;
                }
                self.set_member(Value::Object(id), "length", Value::number(len as f64))?;
                Ok(Value::number(len as f64))
            }
            "Array.prototype.pop" => {
                if length == 0 {
                    self.set_member(Value::Object(id), "length", Value::number(0.0))?;
                    return Ok(Value::Undefined);
                }
                let key = (length - 1).to_string();
                let value = self.get_member(Value::Object(id), &key)?;
                self.protect(&value)?;
                if !self.delete(id, &key)? {
                    return self.throw_error("TypeError", "cannot delete array element");
                }
                self.set_member(
                    Value::Object(id),
                    "length",
                    Value::number((length - 1) as f64),
                )?;
                Ok(value)
            }
            "Array.prototype.join" => {
                let separator = if arg(0) == Value::Undefined {
                    ",".into()
                } else {
                    self.to_string(arg(0))?
                };
                if self.join_stack.contains(&id) {
                    return Ok(Value::String(String::new()));
                }
                self.join_stack.push(id);
                let result = (|| {
                    let mut output = String::new();
                    for index in 0..length {
                        self.tick()?;
                        if index > 0 {
                            output.push_str(&separator);
                        }
                        let v = self.get_member(Value::Object(id), index.to_string())?;
                        self.protect(&v)?;
                        if !matches!(v, Value::Null | Value::Undefined) {
                            output.push_str(&self.to_string(v)?);
                        }
                        if output.len() > 1_048_576 {
                            return Err(ScriptError::Runtime("string size limit"));
                        }
                    }
                    Ok(Value::String(output))
                })();
                self.join_stack.pop();
                result
            }
            "Array.prototype.map"
            | "Array.prototype.filter"
            | "Array.prototype.forEach"
            | "Array.prototype.some"
            | "Array.prototype.every" => {
                let callback = arg(0);
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "callback is not callable");
                }
                let produces = matches!(name, "Array.prototype.map" | "Array.prototype.filter");
                let output = if produces {
                    let len = if name.ends_with("map") {
                        u32::try_from(length).map_err(|_| {
                            ScriptError::Unsupported("map output length exceeds array range")
                        })?
                    } else {
                        0
                    };
                    let out = self.array_species_create(id, len)?;
                    self.protect(&Value::Object(out))?;
                    Some(out)
                } else {
                    None
                };
                let mut to = 0;
                let mut next_index = 0;
                while let Some(index) = self.next_present_index(id, next_index, length)? {
                    next_index = index + 1;
                    self.tick()?;
                    let key = index.to_string();
                    let value = self.get_member(Value::Object(id), &key)?;
                    self.protect(&value)?;
                    let selected = self.invoke_this(
                        callback.clone(),
                        arg(1),
                        vec![
                            value.clone(),
                            Value::number(index as f64),
                            Value::Object(id),
                        ],
                    )?;
                    match name {
                        "Array.prototype.map" => self.put_data(output.unwrap(), &key, selected)?,
                        "Array.prototype.filter" if selected.truthy() => {
                            self.put_data(output.unwrap(), to.to_string(), value)?;
                            to += 1;
                        }
                        "Array.prototype.some" if selected.truthy() => {
                            return Ok(Value::Bool(true))
                        }
                        "Array.prototype.every" if !selected.truthy() => {
                            return Ok(Value::Bool(false))
                        }
                        _ => {}
                    }
                }
                Ok(match output {
                    Some(id) => Value::Object(id),
                    None if name.ends_with("every") => Value::Bool(true),
                    None if name.ends_with("some") => Value::Bool(false),
                    _ => Value::Undefined,
                })
            }
            _ => Err(ScriptError::Unsupported("array method")),
        }
    }
}
