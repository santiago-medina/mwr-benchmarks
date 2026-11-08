//! Object enumeration and grouping share descriptors, keys, iterators and collection slots.
use super::{
    collections::Table,
    objects::Key,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn object_extended(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if name == "Object.hasOwn" {
            let id = self.object_coercible(arg(0))?;
            self.protect(&Value::Object(id))?;
            let key = self.to_key(arg(1))?;
            return Ok(Value::Bool(self.own(id, key)?.is_some()));
        }
        if name == "Object.fromEntries" {
            let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
            self.protect(&Value::Object(id))?;
            self.consume_iterable(arg(0), |e, entry| {
                if !matches!(entry, Value::Object(_)) {
                    return e.throw_error("TypeError", "Object.fromEntries entry is not an object");
                }
                let key = e.get_member(entry.clone(), "0")?;
                e.protect(&key)?;
                let value = e.get_member(entry, "1")?;
                e.protect(&value)?;
                let key = e.to_key(key)?;
                e.put_data(id, key, value)
            })?;
            return Ok(Value::Object(id));
        }
        if name == "Object.groupBy" || name == "Map.groupBy" {
            return self.group_by(name == "Map.groupBy", arg(0), arg(1));
        }
        let source = self.object_coercible(arg(0))?;
        self.protect(&Value::Object(source))?;
        let descriptors = name == "Object.getOwnPropertyDescriptors";
        let out = if descriptors {
            self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?
        } else {
            self.new_array(0)?
        };
        self.protect(&Value::Object(out))?;
        let mut index = 0u64;
        for key in self.own_keys(source)? {
            self.tick()?;
            if descriptors {
                if let Some(property) = self.own(source, &key)? {
                    let value = self.descriptor_object(property)?;
                    self.put_data(out, key, value)?;
                }
                continue;
            }
            let Key::String(key) = key else {
                continue;
            };
            if !self.own(source, &key)?.is_some_and(|p| p.enumerable) {
                continue;
            }
            self.frame(|e| {
                let value = e.get_member(Value::Object(source), &key)?;
                e.protect(&value)?;
                let value = if name == "Object.entries" {
                    let pair = e.new_array(2)?;
                    e.protect(&Value::Object(pair))?;
                    e.put_data(pair, "0", Value::String(key))?;
                    e.put_data(pair, "1", value)?;
                    Value::Object(pair)
                } else {
                    value
                };
                e.put_data(out, index.to_string(), value)
            })?;
            index += 1;
        }
        Ok(Value::Object(out))
    }
    fn group_by(&mut self, map: bool, items: Value, callback: Value) -> Result<Value> {
        if matches!(items, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "groupBy requires an input");
        }
        if !self.callable(&callback) {
            return self.throw_error("TypeError", "groupBy callback is not callable");
        }
        let out = self.new_object(if map {
            Some(self.intrinsics.as_ref().unwrap().collection["Map"])
        } else {
            None
        })?;
        self.protect(&Value::Object(out))?;
        if map {
            let mut data = self.data(out)?.clone();
            data.collection = Some(Table::new(false));
            self.replace_data(out, data)?;
        }
        let mut index = 0u64;
        self.consume_iterable(items, |e, value| {
            if index >= 9007199254740991 {
                return e.throw_error("TypeError", "too many grouping elements");
            }
            let mut key = e.invoke(
                callback.clone(),
                vec![value.clone(), Value::number(index as f64)],
            )?;
            e.protect(&key)?;
            index += 1;
            let array = if map {
                if matches!(key,Value::Number(n) if f64::from_bits(n)==0.0) {
                    key = Value::number(0.0);
                }
                let mut data = e.data(out)?.clone();
                let table = data.collection.as_mut().unwrap();
                if let Some(at) = table.find(&key) {
                    let Value::Object(array) = &table.entries[at].as_ref().unwrap().1 else {
                        unreachable!()
                    };
                    *array
                } else {
                    let array = e.new_array(0)?;
                    e.protect(&Value::Object(array))?;
                    table.write(key, Value::Object(array));
                    e.replace_data(out, data)?;
                    array
                }
            } else {
                let key = e.to_key(key)?;
                if e.own(out, &key)?.is_some() {
                    let Value::Object(array) = e.get_member(Value::Object(out), key)? else {
                        unreachable!()
                    };
                    array
                } else {
                    let array = e.new_array(0)?;
                    e.protect(&Value::Object(array))?;
                    e.put_data(out, key, Value::Object(array))?;
                    array
                }
            };
            let len = e.length_of(array)?;
            e.put_data(array, len.to_string(), value)
        })?;
        Ok(Value::Object(out))
    }
}
