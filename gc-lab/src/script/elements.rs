//! Element fast paths over the dense elements of `PropertyMap` (OBJECT_MODEL.md slice 4).
//!
//! Each entry point is an exact shortcut of one spec operation for an array index on
//! objects whose element access is ordinary. It reads or writes the element slot by `u32`
//! without building the decimal key string. When any precondition fails, it reports
//! "not handled" before mutating anything, and the caller takes the generic string-key
//! path, which stays the single implementation of the exotic and accessor cases.
use super::objects::{with_length_key, Data, Kind, Property};
use super::vm::{Engine, Object, ScriptError, Value};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

/// Prototype levels walked before handing over to the generic path (which has its own
/// traversal limit and error).
const CHAIN: usize = 64;

impl Engine<'_> {
    /// `Data` of an object whose [[GetOwnProperty]], [[DefineOwnProperty]], [[Get]] and
    /// [[Set]] are the ordinary ones (§10.1) for every array index: `Engine::plain` minus the
    /// key test (an index is never `length`). String objects qualify: their code-unit
    /// properties (§10.4.3.5) are materialized as read-only data properties at creation.
    fn plain_elements(&self, id: Id) -> Option<&Data> {
        match self.objects.get(&id)? {
            Object::Ordinary(d)
            | Object::Builtin { data: d, .. }
            | Object::Bound { data: d, .. }
            | Object::Closure { data: d, .. }
                if d.typed.is_none() && d.namespace.is_none() && d.parameter_map.is_none() =>
            {
                Some(d)
            }
            _ => None,
        }
    }
    /// OrdinaryGet (§10.1.8.1) of array index `index` with Receiver = O: an own data
    /// element is the value (step 3), a hole goes to the parent (step 2.b), the end of the
    /// chain is undefined (step 2.a). `None`: an accessor, an exotic level or a long chain.
    fn get_element_fast(&self, id: Id, index: u32) -> Option<Value> {
        let mut at = Some(id);
        for _ in 0..CHAIN {
            let Some(level) = at else {
                return Some(Value::Undefined);
            };
            let d = self.plain_elements(level)?;
            match d.properties.get_index(index) {
                None => at = d.prototype,
                Some(Property {
                    kind: Kind::Data { value, .. },
                    ..
                }) => return Some(value.clone()),
                Some(_) => return None,
            }
        }
        None
    }
    /// Get(O, ToString(index)) for an object O (§7.3.2).
    pub(super) fn get_element(&mut self, id: Id, index: u64) -> Result<Value> {
        if let Ok(i) = u32::try_from(index) {
            if i < u32::MAX {
                self.runtime.value(id)?;
                if let Some(v) = self.get_element_fast(id, i) {
                    return Ok(v);
                }
            }
        }
        self.get_member(Value::Object(id), index.to_string())
    }
    /// The array-index value of a property-key operand when it is a Number that is
    /// already canonical (ToString gives the index digits, §7.1.21): `GetElem`/`SetElem`
    /// with a number key then need no key string. -0 qualifies (ToString(-0) is "0").
    pub(super) fn element_index(key: &Value) -> Option<u32> {
        let Value::Number(bits) = key else {
            return None;
        };
        let n = f64::from_bits(*bits);
        (n >= 0.0 && n < u32::MAX as f64 && n.fract() == 0.0).then_some(n as u32)
    }
    /// `GetElem` with a canonical index key: `Some` when the fast path answered.
    pub(super) fn get_elem_fast(&mut self, receiver: &Value, index: u32) -> Result<Option<Value>> {
        let Value::Object(id) = receiver else {
            return Ok(None);
        };
        self.runtime.value(*id)?;
        Ok(self.get_element_fast(*id, index))
    }
    /// The array `length` and its writability when `id` is a plain array, read in place.
    fn array_length(d: &Data) -> Option<(u32, bool)> {
        match with_length_key(|k| d.properties.get(k).map(|p| p.kind.clone())) {
            Some(Kind::Data {
                value: Value::Number(n),
                writable,
            }) => Some((f64::from_bits(n) as u32, writable)),
            _ => None,
        }
    }
    /// LengthOfArrayLike (§7.3.18) when `id` is an Array exotic object: its `length` is
    /// always an own data property holding a uint32 Number, so Get and ToLength are a read.
    pub(super) fn array_length_fast(&self, id: Id) -> Option<u64> {
        match self.objects.get(&id)? {
            Object::Ordinary(d) if d.array => Self::array_length(d).map(|(n, _)| n as u64),
            _ => None,
        }
    }
    /// Stores `property` at element `index` (existing or new), linking its edges first
    /// and unlinking the replaced ones; for an array, bumps `length` to `index + 1` when
    /// `grow` (§10.4.2.1 step 3.k). The caller has validated the store.
    fn store_element(&mut self, id: Id, index: u32, value: &Value, grow: bool) -> Result<()> {
        if let Some(child) = value.id() {
            self.runtime.link(id, child)?;
        }
        let data = self.data_mut(id)?;
        let property = Property::data(value.clone(), true, true, true);
        let old = match data.properties.get_index_mut(index) {
            Some(slot) => std::mem::replace(slot, property).edges(),
            None => {
                data.properties.push((index.to_string().into(), property));
                Vec::new()
            }
        };
        if grow {
            if let Some(Property {
                kind: Kind::Data { value: slot, .. },
                ..
            }) = with_length_key(|k| data.properties.get_mut(k))
            {
                *slot = Value::number(index as f64 + 1.0);
            }
        }
        for child in old {
            self.runtime.unlink(id, child)?;
        }
        Ok(())
    }
    /// CreateDataProperty(O, index, value) (§7.3.5) when O's elements are ordinary and the
    /// store cannot fail: the element is absent on an extensible object, or present and
    /// configurable (ValidateAndApplyPropertyDescriptor, §10.1.6.3 steps 2 and 6, with a
    /// complete data descriptor); for an array, `length` must be writable when it grows
    /// (§10.4.2.1 step 3.f). `false`: nothing was done, use [[DefineOwnProperty]].
    fn put_element_fast(&mut self, id: Id, index: u32, value: &Value) -> Result<bool> {
        self.runtime.value(id)?;
        let Some(d) = self.plain_elements(id) else {
            return Ok(false);
        };
        let ok = match d.properties.get_index(index) {
            None => d.extensible,
            Some(p) => p.configurable,
        };
        let grow = if d.array {
            match Self::array_length(d) {
                Some((length, writable)) if index >= length => {
                    if !writable {
                        return Ok(false);
                    }
                    true
                }
                Some(_) => false,
                None => return Ok(false),
            }
        } else {
            false
        };
        if !ok {
            return Ok(false);
        }
        self.store_element(id, index, value, grow)?;
        Ok(true)
    }
    /// CreateDataPropertyOrThrow(O, ToString(index), value) (§7.3.7).
    pub(super) fn put_element(&mut self, id: Id, index: u64, value: Value) -> Result<()> {
        if let Ok(i) = u32::try_from(index) {
            if i < u32::MAX && self.put_element_fast(id, i, &value)? {
                return Ok(());
            }
        }
        self.put_data(id, index.to_string(), value)
    }
    /// OrdinarySet (§10.1.9.2) of array index `index` with Receiver = O, for the cases
    /// `Engine::plain_set` covers for string keys: every level is ordinary for elements,
    /// and the index is either absent from the chain or a writable data property (own:
    /// replace the value, §10.1.9.2 step 2.d; inherited or absent: CreateDataProperty on
    /// an extensible receiver, step 2.e, with the array `length` bump of §10.4.2.1).
    /// `false`: nothing was done, use [[Set]].
    pub(super) fn set_element_fast(&mut self, id: Id, index: u32, value: &Value) -> Result<bool> {
        self.runtime.value(id)?;
        let Some(own) = self.plain_elements(id) else {
            return Ok(false);
        };
        match own.properties.get_index(index) {
            Some(Property {
                kind: Kind::Data { writable: true, .. },
                ..
            }) => {
                // Own writable data property: only [[Value]] changes, the attributes stay.
                if let Some(child) = value.id() {
                    self.runtime.link(id, child)?;
                }
                let mut old = None;
                if let Some(Property {
                    kind: Kind::Data { value: slot, .. },
                    ..
                }) = self.data_mut(id)?.properties.get_index_mut(index)
                {
                    old = std::mem::replace(slot, value.clone()).id();
                }
                if let Some(child) = old {
                    self.runtime.unlink(id, child)?;
                }
                return Ok(true);
            }
            Some(_) => return Ok(false),
            None => {}
        }
        let mut at = own.prototype;
        for _ in 0..CHAIN {
            let Some(p) = at else { break };
            let Some(d) = self.plain_elements(p) else {
                return Ok(false);
            };
            match d.properties.get_index(index) {
                None => at = d.prototype,
                Some(Property {
                    kind: Kind::Data { writable: true, .. },
                    ..
                }) => {
                    at = None;
                    break;
                }
                Some(_) => return Ok(false),
            }
        }
        if at.is_some() || !own.extensible {
            return Ok(false);
        }
        let grow = if own.array {
            match Self::array_length(own) {
                Some((length, writable)) if index >= length => {
                    if !writable {
                        return Ok(false);
                    }
                    true
                }
                Some(_) => false,
                None => return Ok(false),
            }
        } else {
            false
        };
        self.store_element(id, index, value, grow)?;
        Ok(true)
    }
    /// IteratorStepValue (§7.4.8) on the record (iterator, next): `Some(value)`, or `None`
    /// once done. When [[NextMethod]] is %ArrayIteratorPrototype%.next and the iterator has
    /// [[IteratedArrayLike]], the call's result would be a fresh CreateIterResultObject
    /// (§7.4.14) whose `done` and `value` are own data properties read straight back
    /// (IteratorComplete and IteratorValue, §7.4.6-7): the object never escapes, so the
    /// steps of §23.1.5.1 run directly and the value is returned without the object.
    /// Only in a single-realm engine: with realms, the call would switch to the function's
    /// realm for its TypeErrors.
    pub(super) fn iterator_step_value(
        &mut self,
        iterator: &Value,
        next: &Value,
    ) -> Result<Option<Value>> {
        if let (Value::Object(it), Value::Object(f)) = (iterator, next) {
            if self.realms.is_empty()
                && matches!(self.objects.get(f), Some(Object::Builtin { name, .. })
                    if name == "%ArrayIteratorPrototype%.next")
                && matches!(self.objects.get(it), Some(Object::Ordinary(d)) if d.iterator.is_some())
            {
                self.tick()?;
                return self.array_iterator_step(*it);
            }
        }
        let step = self.invoke_this(next.clone(), iterator.clone(), vec![])?;
        self.protect(&step)?;
        if !matches!(step, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator result is not an object");
        }
        if self.get_member(step.clone(), "done")?.truthy() {
            return Ok(None);
        }
        let value = self.get_member(step, "value")?;
        self.protect(&value)?;
        Ok(Some(value))
    }
}
