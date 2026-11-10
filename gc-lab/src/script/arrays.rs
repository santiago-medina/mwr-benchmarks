//! Array exotics share descriptor validation and transactional GC edges with ordinary objects.
use super::js_string::{Builder, JsString};
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
        if let Some(index) = key.index() {
            // ArraySetLength is not involved (§10.4.2.1 step 2): one key plus `length`.
            let Some(Kind::Data {
                value: Value::Number(old),
                writable,
            }) = self.data(id)?.own("length").map(|p| p.kind.clone())
            else {
                unreachable!()
            };
            let old = f64::from_bits(old) as u32;
            if index >= old && !writable {
                return Ok(false);
            }
            if !self.define_in_place(id, key, desc)? {
                return Ok(false);
            }
            if index >= old {
                if let Some(p) = self.data_mut(id)?.properties.get_mut(&Key::from("length")) {
                    p.kind = Kind::Data {
                        value: Value::number(index as f64 + 1.0),
                        writable,
                    };
                }
            }
            return Ok(true);
        }
        if key != Key::from("length") {
            return self.define_in_place(id, key, desc);
        }
        // ArraySetLength (§10.4.2.4), stored in place: no Data clone per write.
        let Some(Kind::Data {
            value: Value::Number(old),
            writable,
        }) = self.data(id)?.own("length").map(|p| p.kind.clone())
        else {
            unreachable!()
        };
        let old = f64::from_bits(old) as u32;
        let new = if let Some(Value::Number(n)) = desc.value {
            f64::from_bits(n) as u32
        } else {
            old
        };
        // Steps 12-13: newLen >= oldLen is OrdinaryDefineOwnProperty.
        if new >= old {
            return self.define_in_place(id, key, desc);
        }
        // Step 14.
        if !writable {
            return Ok(false);
        }
        // Steps 15-16.
        let final_readonly = desc.writable == Some(false);
        if final_readonly {
            desc.writable = Some(true);
        }
        // Steps 17-18.
        if !self.define_in_place(id, key, desc)? {
            return Ok(false);
        }
        // Step 19: delete own array indices >= newLen, descending, stopping at the
        // first non-configurable one (19.d.iii). Deleting a configurable property of
        // an ordinary array runs no code and has no effect beyond its absence, so a
        // long range is one pass: the highest non-configurable index >= newLen stops
        // the deletion, every index above it goes.
        if old - new > 8 {
            let data = self.data(id)?;
            let stop = data
                .properties
                .iter()
                .filter_map(|(k, p)| k.index().filter(|i| *i >= new && !p.configurable))
                .max();
            let cut = stop.map_or(new, |s| s + 1);
            let mut edges = Vec::new();
            self.data_mut(id)?.properties.retain(|(k, p)| {
                let doomed = k.index().is_some_and(|i| i >= cut);
                if doomed {
                    edges.extend(p.edges());
                }
                !doomed
            });
            for child in edges {
                self.runtime.unlink(id, child)?;
            }
            let accepted = stop.is_none();
            if let Some(index) = stop {
                // Step 19.d.iii.
                self.define_in_place(
                    id,
                    Key::from("length"),
                    Descriptor {
                        value: Some(Value::number(index as f64 + 1.0)),
                        ..Descriptor::default()
                    },
                )?;
            }
            if final_readonly {
                self.define_in_place(
                    id,
                    Key::from("length"),
                    Descriptor {
                        writable: Some(false),
                        ..Descriptor::default()
                    },
                )?;
            }
            return Ok(accepted);
        }
        // Short ranges, one by one in descending order. Probe the
        // range directly when it is shorter than the property list (pop, length -= 1).
        let data = self.data(id)?;
        let mut indices: Vec<u32> = if ((old - new) as usize) <= data.properties.len() {
            (new..old)
                .rev()
                .filter(|i| data.properties.get(&Key::String(i.to_string())).is_some())
                .collect()
        } else {
            let mut v: Vec<u32> = data
                .properties
                .iter()
                .filter_map(|(k, _)| k.index().filter(|i| *i >= new))
                .collect();
            v.sort_by(|a, b| b.cmp(a));
            v
        };
        let mut accepted = true;
        for index in indices.drain(..) {
            let k = Key::String(index.to_string());
            if !self.data(id)?.own(&k).unwrap().configurable {
                // Step 19.d.iii.
                self.define_in_place(
                    id,
                    Key::from("length"),
                    Descriptor {
                        value: Some(Value::number(index as f64 + 1.0)),
                        ..Descriptor::default()
                    },
                )?;
                accepted = false;
                break;
            }
            self.remove_in_place(id, &k)?;
        }
        // Step 20 (and 19.d.iii.2).
        if final_readonly {
            self.define_in_place(
                id,
                Key::from("length"),
                Descriptor {
                    writable: Some(false),
                    ..Descriptor::default()
                },
            )?;
        }
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
    pub(super) fn iterator_result(&mut self, value: Value, done: bool) -> Result<Value> {
        self.protect(&value)?;
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        self.put_data(id, "value", value)?;
        self.put_data(id, "done", Value::Bool(done))?;
        Ok(Value::Object(id))
    }
    pub(super) fn array_species_create(&mut self, original: Id, len: u32) -> Result<Id> {
        let mut constructor = Value::Undefined;
        // §10.4.2.3 ArraySpeciesCreate step 3: IsArray(originalArray) (follows proxies).
        if self.is_array(&Value::Object(original))? {
            constructor = self.get_member(Value::Object(original), "constructor")?;
            self.protect(&constructor)?;
            // Step 5: another realm's %Array% constructor means "make a plain array here".
            if matches!(constructor, Value::Object(_)) && !self.realms.is_empty() {
                let realm = self.function_realm(&constructor)?;
                if realm != self.current_realm
                    && constructor == Value::Object(self.realm_intrinsics(realm).functions["Array"])
                {
                    constructor = Value::Undefined;
                }
            }
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
            "Array.from" => return self.array_from(receiver, args, false),
            "Array.of" => return self.array_from(receiver, args, true),
            "Array.fromAsync" => return self.array_from_async(receiver, args),
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
                return self.is_array(&arg(0)).map(Value::Bool);
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
                // §23.1.5.1 step 2.b: only a real TypedArray has [[ViewedArrayBuffer]];
                // a proxy target takes the LengthOfArrayLike path.
                if !self.is_proxy(target) && self.data(target)?.typed.is_some() {
                    self.typed_record(Value::Object(target))?;
                }
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
            "Array.prototype.indexOf" => {
                let n = self.to_number(arg(1))?;
                let n = if n.is_nan() { 0.0 } else { n.trunc() };
                if n >= length as f64 {
                    return Ok(Value::number(-1.0));
                }
                let mut at = if n < 0.0 {
                    (length as f64 + n).max(0.0) as u64
                } else {
                    n as u64
                };
                while let Some(index) = self.next_present_index(id, at, length)? {
                    self.tick()?;
                    at = index + 1;
                    let value = self.get_member(Value::Object(id), index.to_string())?;
                    if value.strict_equal(&arg(0)) {
                        return Ok(Value::number(index as f64));
                    }
                }
                Ok(Value::number(-1.0))
            }
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
            "Array.prototype.toLocaleString" => self.locale_join(id, length as u64),
            "Array.prototype.join" => {
                let separator: JsString = if arg(0) == Value::Undefined {
                    ",".into()
                } else {
                    self.to_js_string(arg(0))?
                };
                if self.join_stack.contains(&id) {
                    return Ok(Value::String(String::new().into()));
                }
                self.join_stack.push(id);
                let result = (|| {
                    let mut output = Builder::default();
                    for index in 0..length {
                        self.tick()?;
                        if index > 0 {
                            output.push(&separator);
                        }
                        let v = self.get_member(Value::Object(id), index.to_string())?;
                        self.protect(&v)?;
                        if !matches!(v, Value::Null | Value::Undefined) {
                            output.push(&self.to_js_string(v)?);
                        }
                        if output.size() > 1_048_576 {
                            return Err(ScriptError::Runtime("string size limit"));
                        }
                    }
                    Ok(Value::String(output.finish()))
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
            _ => self.array_extended(name, id, length, args),
        }
    }
}
impl Engine<'_> {
    /// §23.1.3.32 Array.prototype.toLocaleString steps 4-7 (shared with
    /// %TypedArray%.prototype.toLocaleString, §23.2.3.31): each element that is
    /// not undefined or null contributes ToString(Invoke(element, "toLocaleString")).
    /// The separator is the implementation-defined list separator ",".
    pub(super) fn locale_join(&mut self, id: Id, length: u64) -> Result<Value> {
        if self.join_stack.contains(&id) {
            return Ok(Value::String(String::new().into()));
        }
        self.join_stack.push(id);
        let result = (|| {
            let mut output = String::new();
            for index in 0..length {
                self.tick()?;
                if index > 0 {
                    output.push(',');
                }
                let v = self.get_member(Value::Object(id), index.to_string())?;
                self.protect(&v)?;
                if !matches!(v, Value::Null | Value::Undefined) {
                    let f = self.get_member(v.clone(), "toLocaleString")?;
                    if !self.callable(&f) {
                        return self.throw_error("TypeError", "toLocaleString is not a function");
                    }
                    let s = self.invoke_this(f, v, vec![])?;
                    output.push_str(&self.to_string(s)?);
                }
                if output.len() > 1_048_576 {
                    return Err(ScriptError::Runtime("string size limit"));
                }
            }
            Ok(Value::String(output.into()))
        })();
        self.join_stack.pop();
        result
    }
}
