//! For-in traverses prototype levels without exposing its cursor to user iterator hooks.
use super::{
    objects::Key,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashSet;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct Cursor {
    pub current: Option<Id>,
    pub keys: Option<Vec<String>>,
    pub index: usize,
    pub seen: HashSet<String>,
}
impl Engine<'_> {
    pub(super) fn bootstrap_enumeration(&mut self, env: Id, iterator: Id) -> Result<()> {
        let function = self.intrinsics.as_ref().unwrap().function;
        let proto = self.new_object(Some(iterator))?;
        self.runtime.link(env, proto)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("ForInIterator".into(), proto);
        let name = "%ForInIteratorPrototype%.next";
        let next = self.intrinsic_function(env, name, Some(function))?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .functions
            .insert(name.into(), next);
        self.hidden(proto, "next", Value::Object(next))?;
        Ok(())
    }
    pub(super) fn enumeration(&mut self, source: Value) -> Result<Value> {
        let current = if matches!(source, Value::Null | Value::Undefined) {
            None
        } else {
            let id = self.object_coercible(source)?;
            self.protect(&Value::Object(id))?;
            Some(id)
        };
        let proto = self.intrinsics.as_ref().unwrap().collection["ForInIterator"];
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.enumeration = Some(Cursor {
            current,
            keys: None,
            index: 0,
            seen: HashSet::new(),
        });
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    pub(super) fn enumeration_next(&mut self, receiver: Value) -> Result<Value> {
        let Value::Object(id) = receiver else {
            return self.throw_error("TypeError", "invalid enumeration receiver");
        };
        let Some(mut cursor) = self.data(id).ok().and_then(|d| d.enumeration.clone()) else {
            return self.throw_error("TypeError", "invalid enumeration receiver");
        };
        let mut value = None;
        while let Some(current) = cursor.current {
            self.tick()?;
            if cursor.keys.is_none() {
                cursor.keys = Some(
                    self.own_keys(current)?
                        .into_iter()
                        .filter_map(|k| {
                            if let Key::String(s) = k {
                                Some(s)
                            } else {
                                None
                            }
                        })
                        .collect(),
                );
                cursor.index = 0;
            }
            if let Some(key) = cursor.keys.as_ref().unwrap().get(cursor.index).cloned() {
                cursor.index += 1;
                if cursor.seen.contains(&key) {
                    continue;
                }
                if let Some(property) = self.own(current, &key)? {
                    cursor.seen.insert(key.clone());
                    if property.enumerable {
                        value = Some(key);
                        break;
                    }
                }
            } else {
                // §14.7.5.10 EnumerateObjectProperties walks [[GetPrototypeOf]] (proxy traps run).
                cursor.current = self.get_prototype_of(current)?;
                cursor.keys = None;
            }
        }
        if cursor.current.is_none() {
            cursor.seen.clear();
            cursor.keys = None;
        }
        let mut data = self.data(id)?.clone();
        data.enumeration = Some(cursor);
        self.replace_data(id, data)?;
        let done = value.is_none();
        self.iterator_result(
            value
                .map(|s| Value::String(s.into()))
                .unwrap_or(Value::Undefined),
            done,
        )
    }
}
