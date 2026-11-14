//! Set algebra follows GetSetRecord and uses live or copied traversal as each operation requires.
//! Algorithm reference: https://tc39.es/ecma262/multipage/keyed-collections.html#sec-set-objects
use super::{
    collections::Table,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
struct SetRecord {
    object: Value,
    size: f64,
    has: Value,
    keys: Value,
}
impl Engine<'_> {
    fn set_record(&mut self, other: Value) -> Result<SetRecord> {
        if !matches!(other, Value::Object(_)) {
            return self.throw_error("TypeError", "set operand is not an object");
        }
        let size = self.get_member(other.clone(), "size")?;
        self.protect(&size)?;
        let size = self.to_number(size)?;
        if size.is_nan() {
            return self.throw_error("TypeError", "set operand size is NaN");
        }
        let size = size.trunc();
        if size < 0.0 {
            return self.throw_error("RangeError", "set operand size is negative");
        }
        let has = self.get_member(other.clone(), "has")?;
        self.protect(&has)?;
        if !self.callable(&has) {
            return self.throw_error("TypeError", "set operand has method is not callable");
        }
        let keys = self.get_member(other.clone(), "keys")?;
        self.protect(&keys)?;
        if !self.callable(&keys) {
            return self.throw_error("TypeError", "set operand keys method is not callable");
        }
        Ok(SetRecord {
            object: other,
            size,
            has,
            keys,
        })
    }
    fn set_has(&self, id: Id, key: &Value) -> Result<bool> {
        Ok(self
            .data(id)?
            .collection
            .as_ref()
            .unwrap()
            .find(key)
            .is_some())
    }
    fn set_insert(&mut self, id: Id, key: Value) -> Result<()> {
        // SetData stores +0 even when a set-like iterator yields -0.
        // Table::write canonicalizes the lookup key but not the stored value.
        let key = if matches!(&key, Value::Number(bits) if f64::from_bits(*bits) == 0.0) {
            Value::number(0.0)
        } else {
            key
        };
        let mut data = self.data(id)?.clone();
        data.collection.as_mut().unwrap().write(key.clone(), key);
        self.replace_data(id, data)
    }
    fn set_remove(&mut self, id: Id, key: &Value) -> Result<()> {
        let mut data = self.data(id)?.clone();
        let table = data.collection.as_mut().unwrap();
        if let Some(at) = table.find(key) {
            table.remove_at(at);
        }
        self.replace_data(id, data)
    }
    fn set_copy(&mut self, source: Option<Id>) -> Result<Id> {
        let table = if let Some(id) = source {
            self.data(id)?.collection.clone().unwrap()
        } else {
            Table::new(true)
        };
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().collection["Set"]))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.collection = Some(table);
        self.replace_data(id, data)?;
        Ok(id)
    }
    fn walk_set(
        &mut self,
        id: Id,
        mut visit: impl FnMut(&mut Self, Value) -> Result<bool>,
    ) -> Result<bool> {
        let mut index = 0;
        loop {
            self.tick()?;
            let entry = self
                .data(id)?
                .collection
                .as_ref()
                .unwrap()
                .entries
                .get(index)
                .cloned();
            let Some(entry) = entry else { return Ok(true) };
            index += 1;
            if let Some((key, _)) = entry {
                if !self.frame(|e| {
                    e.protect(&key)?;
                    visit(e, key)
                })? {
                    return Ok(false);
                }
            }
        }
    }
    pub(super) fn set_algebra(&mut self, op: &str, id: Id, other: Value) -> Result<Value> {
        let record = self.set_record(other)?;
        let own_size = self.data(id)?.collection.as_ref().unwrap().len() as f64;
        match op {
            "isSubsetOf" => {
                if own_size > record.size {
                    return Ok(Value::Bool(false));
                }
                let yes = self.walk_set(id, |e, key| {
                    e.invoke_this(record.has.clone(), record.object.clone(), vec![key])
                        .map(|v| v.truthy())
                })?;
                Ok(Value::Bool(yes))
            }
            "isSupersetOf" | "isDisjointFrom" => {
                if op == "isSupersetOf" && own_size < record.size {
                    return Ok(Value::Bool(false));
                }
                if op == "isDisjointFrom" && own_size <= record.size {
                    let yes = self.walk_set(id, |e, key| {
                        e.invoke_this(record.has.clone(), record.object.clone(), vec![key])
                            .map(|v| !v.truthy())
                    })?;
                    return Ok(Value::Bool(yes));
                }
                let (iterator, next) = self.open_iterator(record.object, record.keys)?;
                let yes = self.visit_iterator(iterator, next, |e, key| {
                    let found = e.set_has(id, &key)?;
                    Ok(if op == "isSupersetOf" { found } else { !found })
                })?;
                Ok(Value::Bool(yes))
            }
            "union" | "symmetricDifference" => {
                let (iterator, next) = self.open_iterator(record.object, record.keys)?;
                let out = self.set_copy(Some(id))?;
                self.visit_iterator(iterator, next, |e, key| {
                    let exists = e.set_has(out, &key)?;
                    if op == "union" {
                        if !exists {
                            e.set_insert(out, key)?;
                        }
                    } else if e.set_has(id, &key)? {
                        if exists {
                            e.set_remove(out, &key)?;
                        }
                    } else if !exists {
                        e.set_insert(out, key)?;
                    }
                    Ok(true)
                })?;
                Ok(Value::Object(out))
            }
            "intersection" | "difference" => {
                let difference = op == "difference";
                let out = self.set_copy(if difference { Some(id) } else { None })?;
                if own_size <= record.size {
                    self.walk_set(if difference { out } else { id }, |e, key| {
                        let present = e
                            .invoke_this(
                                record.has.clone(),
                                record.object.clone(),
                                vec![key.clone()],
                            )?
                            .truthy();
                        if present {
                            if difference {
                                e.set_remove(out, &key)?;
                            } else if !e.set_has(out, &key)? {
                                e.set_insert(out, key)?;
                            }
                        }
                        Ok(true)
                    })?;
                } else {
                    let (iterator, next) = self.open_iterator(record.object, record.keys)?;
                    self.visit_iterator(iterator, next, |e, key| {
                        if difference {
                            e.set_remove(out, &key)?;
                        } else if e.set_has(id, &key)? && !e.set_has(out, &key)? {
                            e.set_insert(out, key)?;
                        }
                        Ok(true)
                    })?;
                }
                Ok(Value::Object(out))
            }
            _ => Err(ScriptError::Unsupported("set operation")),
        }
    }
}
