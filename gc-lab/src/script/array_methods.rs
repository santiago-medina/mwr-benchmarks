//! Generic array operations use the existing descriptor, species and property paths.
use super::{
    objects::Key,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn integer(&mut self, value: Value) -> Result<f64> {
        let n = self.to_number(value)?;
        Ok(if n.is_nan() { 0.0 } else { n.trunc() })
    }
    pub(super) fn relative_index(&mut self, value: Value, len: u64) -> Result<u64> {
        let n = self.integer(value)?;
        Ok(if n < 0.0 {
            (len as f64 + n).max(0.0) as u64
        } else {
            n.min(len as f64) as u64
        })
    }
    fn remove_index(&mut self, id: Id, index: u64) -> Result<()> {
        if !self.delete(id, index.to_string())? {
            return self.throw_error("TypeError", "array property cannot be deleted");
        }
        Ok(())
    }
    fn copy_index(&mut self, id: Id, from: u64, to: u64) -> Result<()> {
        let key = from.to_string();
        if self.has(id, &key)? {
            let v = self.get_member(Value::Object(id), key)?;
            self.protect(&v)?;
            self.set_member(Value::Object(id), to.to_string(), v)
        } else {
            self.remove_index(id, to)
        }
    }
    pub(super) fn array_from(
        &mut self,
        receiver: Value,
        args: Vec<Value>,
        of: bool,
    ) -> Result<Value> {
        let input = args.first().cloned().unwrap_or(Value::Undefined);
        let map = args.get(1).cloned().unwrap_or(Value::Undefined);
        let this = args.get(2).cloned().unwrap_or(Value::Undefined);
        if !of && map != Value::Undefined && !self.callable(&map) {
            return self.throw_error("TypeError", "Array.from mapping function is not callable");
        }
        if !of && matches!(input, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "Array.from requires an input");
        }
        let method = if of {
            Value::Undefined
        } else {
            self.get_member(input.clone(), Key::Symbol(0))?
        };
        self.protect(&method)?;
        let iterable = !matches!(method, Value::Undefined | Value::Null);
        let source = if of || iterable {
            None
        } else {
            Some(self.object_coercible(input.clone())?)
        };
        if let Some(id) = source {
            self.protect(&Value::Object(id))?;
        }
        let len = if of {
            args.len() as u64
        } else if let Some(id) = source {
            self.length_of(id)?
        } else {
            0
        };
        let result = if self.constructible(&receiver) {
            self.construct(
                receiver,
                if iterable {
                    vec![]
                } else {
                    vec![Value::number(len as f64)]
                },
            )?
        } else {
            let n = u32::try_from(len)
                .map_err(|_| ScriptError::Runtime("Array.from output length limit"))?;
            Value::Object(self.new_array(n)?)
        };
        self.protect(&result)?;
        let Value::Object(out) = result.clone() else {
            unreachable!()
        };
        let mut index = 0u64;
        if iterable {
            self.consume_iterator(input, method, |e, value| {
                if index >= 9007199254740991 {
                    return e.throw_error("TypeError", "Array.from exceeds safe length");
                }
                let value = if map != Value::Undefined {
                    e.invoke_this(
                        map.clone(),
                        this.clone(),
                        vec![value, Value::number(index as f64)],
                    )?
                } else {
                    value
                };
                e.put_data(out, index.to_string(), value)?;
                index += 1;
                Ok(())
            })?;
        } else {
            while index < len {
                self.tick()?;
                let value = if of {
                    args[index as usize].clone()
                } else {
                    self.get_member(Value::Object(source.unwrap()), index.to_string())?
                };
                self.protect(&value)?;
                let value = if !of && map != Value::Undefined {
                    self.invoke_this(
                        map.clone(),
                        this.clone(),
                        vec![value, Value::number(index as f64)],
                    )?
                } else {
                    value
                };
                self.put_data(out, index.to_string(), value)?;
                index += 1;
            }
        }
        self.set_member(result.clone(), "length", Value::number(index as f64))?;
        Ok(result)
    }
    pub(super) fn array_extended(
        &mut self,
        name: &str,
        id: Id,
        len: u64,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let receiver = Value::Object(id);
        match name.rsplit('.').next().unwrap() {
            "slice" => {
                let start = self.relative_index(arg(0), len)?;
                let end = if arg(1) == Value::Undefined {
                    len
                } else {
                    self.relative_index(arg(1), len)?
                };
                let count = end.saturating_sub(start);
                let n = u32::try_from(count)
                    .map_err(|_| ScriptError::Runtime("slice output length limit"))?;
                let out = self.array_species_create(id, n)?;
                self.protect(&Value::Object(out))?;
                for offset in 0..count {
                    self.tick()?;
                    let key = (start + offset).to_string();
                    if self.has(id, &key)? {
                        let value = self.get_member(receiver.clone(), key)?;
                        self.put_data(out, offset.to_string(), value)?;
                    }
                }
                self.set_member(Value::Object(out), "length", Value::number(count as f64))?;
                Ok(Value::Object(out))
            }
            "splice" => {
                let start = self.relative_index(arg(0), len)?;
                let deleted = if args.is_empty() {
                    0
                } else if args.len() == 1 {
                    len - start
                } else {
                    self.integer(arg(1))?.max(0.0).min((len - start) as f64) as u64
                };
                let inserted = args.len().saturating_sub(2) as u64;
                let length = len - deleted + inserted;
                if length > 9007199254740991 {
                    return self.throw_error("TypeError", "splice length exceeds safe integer");
                }
                let n = u32::try_from(deleted)
                    .map_err(|_| ScriptError::Runtime("splice output length limit"))?;
                let out = self.array_species_create(id, n)?;
                self.protect(&Value::Object(out))?;
                for k in 0..deleted {
                    self.tick()?;
                    let key = (start + k).to_string();
                    if self.has(id, &key)? {
                        let v = self.get_member(receiver.clone(), key)?;
                        self.put_data(out, k.to_string(), v)?;
                    }
                }
                self.set_member(Value::Object(out), "length", Value::number(deleted as f64))?;
                if inserted < deleted {
                    for k in start..len - deleted {
                        self.tick()?;
                        self.copy_index(id, k + deleted, k + inserted)?;
                    }
                    for k in (length..len).rev() {
                        self.tick()?;
                        self.remove_index(id, k)?;
                    }
                } else if inserted > deleted {
                    for k in (start..len - deleted).rev() {
                        self.tick()?;
                        self.copy_index(id, k + deleted, k + inserted)?;
                    }
                }
                for (k, v) in args.into_iter().skip(2).enumerate() {
                    self.set_member(receiver.clone(), (start + k as u64).to_string(), v)?;
                }
                self.set_member(receiver, "length", Value::number(length as f64))?;
                Ok(Value::Object(out))
            }
            "shift" => {
                if len == 0 {
                    self.set_member(receiver, "length", Value::number(0.0))?;
                    return Ok(Value::Undefined);
                }
                let first = self.get_member(receiver.clone(), "0")?;
                self.protect(&first)?;
                for k in 1..len {
                    self.tick()?;
                    self.copy_index(id, k, k - 1)?;
                }
                self.remove_index(id, len - 1)?;
                self.set_member(receiver, "length", Value::number((len - 1) as f64))?;
                Ok(first)
            }
            "unshift" => {
                let count = args.len() as u64;
                let length = len + count;
                if length > 9007199254740991 {
                    return self.throw_error("TypeError", "unshift length exceeds safe integer");
                }
                if count > 0 {
                    for k in (0..len).rev() {
                        self.tick()?;
                        self.copy_index(id, k, k + count)?;
                    }
                    for (k, v) in args.into_iter().enumerate() {
                        self.set_member(receiver.clone(), k.to_string(), v)?;
                    }
                }
                self.set_member(receiver, "length", Value::number(length as f64))?;
                Ok(Value::number(length as f64))
            }
            "reverse" => {
                for lo in 0..len / 2 {
                    self.tick()?;
                    let hi = len - lo - 1;
                    self.frame(|e| {
                        let lower = if e.has(id, lo.to_string())? {
                            let v = e.get_member(receiver.clone(), lo.to_string())?;
                            e.protect(&v)?;
                            Some(v)
                        } else {
                            None
                        };
                        let upper = if e.has(id, hi.to_string())? {
                            let v = e.get_member(receiver.clone(), hi.to_string())?;
                            e.protect(&v)?;
                            Some(v)
                        } else {
                            None
                        };
                        match (lower, upper) {
                            (Some(a), Some(b)) => {
                                e.set_member(receiver.clone(), lo.to_string(), b)?;
                                e.set_member(receiver.clone(), hi.to_string(), a)?;
                            }
                            (None, Some(b)) => {
                                e.set_member(receiver.clone(), lo.to_string(), b)?;
                                e.remove_index(id, hi)?;
                            }
                            (Some(a), None) => {
                                e.remove_index(id, lo)?;
                                e.set_member(receiver.clone(), hi.to_string(), a)?;
                            }
                            _ => {}
                        }
                        Ok(())
                    })?;
                }
                Ok(receiver)
            }
            "fill" => {
                let start = self.relative_index(arg(1), len)?;
                let end = if arg(2) == Value::Undefined {
                    len
                } else {
                    self.relative_index(arg(2), len)?
                };
                for k in start..end {
                    self.tick()?;
                    self.set_member(receiver.clone(), k.to_string(), arg(0))?;
                }
                Ok(receiver)
            }
            "copyWithin" => {
                let to = self.relative_index(arg(0), len)?;
                let from = self.relative_index(arg(1), len)?;
                let end = if arg(2) == Value::Undefined {
                    len
                } else {
                    self.relative_index(arg(2), len)?
                };
                let count = end.saturating_sub(from).min(len - to);
                let backwards = from < to && to < from + count;
                for k in 0..count {
                    self.tick()?;
                    let offset = if backwards { count - k - 1 } else { k };
                    self.copy_index(id, from + offset, to + offset)?;
                }
                Ok(receiver)
            }
            "at" => {
                let n = self.integer(arg(0))?;
                let index = if n < 0.0 { len as f64 + n } else { n };
                if index < 0.0 || index >= len as f64 {
                    return Ok(Value::Undefined);
                }
                self.get_member(receiver, index.to_string())
            }
            "includes" => {
                if len == 0 {
                    return Ok(Value::Bool(false));
                }
                let start = self.relative_index(arg(1), len)?;
                for k in start..len {
                    self.tick()?;
                    if self
                        .get_member(receiver.clone(), k.to_string())?
                        .same_value_zero(&arg(0))
                    {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            "lastIndexOf" => {
                if len == 0 {
                    return Ok(Value::number(-1.0));
                }
                let n = if args.len() < 2 {
                    (len - 1) as f64
                } else {
                    self.integer(arg(1))?
                };
                if n == f64::NEG_INFINITY {
                    return Ok(Value::number(-1.0));
                }
                let n = if n < 0.0 {
                    len as f64 + n
                } else {
                    n.min((len - 1) as f64)
                };
                if n < 0.0 {
                    return Ok(Value::number(-1.0));
                }
                for k in (0..=n as u64).rev() {
                    self.tick()?;
                    if self.has(id, k.to_string())?
                        && self
                            .get_member(receiver.clone(), k.to_string())?
                            .strict_equal(&arg(0))
                    {
                        return Ok(Value::number(k as f64));
                    }
                }
                Ok(Value::number(-1.0))
            }
            "find" | "findIndex" | "findLast" | "findLastIndex" => {
                let callback = arg(0);
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "find predicate is not callable");
                }
                let reverse = name.contains("findLast");
                for n in 0..len {
                    self.tick()?;
                    let k = if reverse { len - n - 1 } else { n };
                    let found = self.frame(|e| {
                        let value = e.get_member(receiver.clone(), k.to_string())?;
                        e.protect(&value)?;
                        let yes = e.invoke_this(
                            callback.clone(),
                            arg(1),
                            vec![value.clone(), Value::number(k as f64), receiver.clone()],
                        )?;
                        Ok(if yes.truthy() { Some(value) } else { None })
                    })?;
                    if let Some(value) = found {
                        return Ok(if name.ends_with("Index") {
                            Value::number(k as f64)
                        } else {
                            value
                        });
                    }
                }
                Ok(if name.ends_with("Index") {
                    Value::number(-1.0)
                } else {
                    Value::Undefined
                })
            }
            "reduce" | "reduceRight" => {
                let callback = arg(0);
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "reduce callback is not callable");
                }
                let cell = self.new_object(None)?;
                self.protect(&Value::Object(cell))?;
                let mut initialized = args.len() > 1;
                if initialized {
                    self.put_data(cell, "value", arg(1))?;
                }
                for n in 0..len {
                    self.tick()?;
                    let k = if name.ends_with("Right") {
                        len - n - 1
                    } else {
                        n
                    };
                    if !self.has(id, k.to_string())? {
                        continue;
                    }
                    self.frame(|e| {
                        let value = e.get_member(receiver.clone(), k.to_string())?;
                        e.protect(&value)?;
                        let value = if initialized {
                            let acc = e.get_member(Value::Object(cell), "value")?;
                            e.invoke_this(
                                callback.clone(),
                                Value::Undefined,
                                vec![acc, value, Value::number(k as f64), receiver.clone()],
                            )?
                        } else {
                            initialized = true;
                            value
                        };
                        e.put_data(cell, "value", value)
                    })?;
                }
                if !initialized {
                    return self
                        .throw_error("TypeError", "reduce on empty input without initial value");
                }
                self.get_member(Value::Object(cell), "value")
            }
            "sort" => {
                let compare = arg(0);
                if compare != Value::Undefined && !self.callable(&compare) {
                    return self.throw_error("TypeError", "sort comparison is not callable");
                }
                let roots = self.new_array(0)?;
                self.protect(&Value::Object(roots))?;
                let mut values = vec![];
                for k in 0..len {
                    self.tick()?;
                    if self.has(id, k.to_string())? {
                        let value = self.get_member(receiver.clone(), k.to_string())?;
                        self.put_data(roots, values.len().to_string(), value.clone())?;
                        values.push(value);
                    }
                }
                let count = values.len();
                let mut work = vec![Value::Undefined; count];
                let mut width = 1;
                while width < count {
                    for start in (0..count).step_by(width * 2) {
                        let middle = (start + width).min(count);
                        let end = (middle + width).min(count);
                        let (mut left, mut right) = (start, middle);
                        for out in start..end {
                            self.tick()?;
                            let take_left = if left == middle {
                                false
                            } else if right == end {
                                true
                            } else {
                                self.sort_compare(&compare, &values[left], &values[right])? <= 0
                            };
                            if take_left {
                                work[out] = values[left].clone();
                                left += 1;
                            } else {
                                work[out] = values[right].clone();
                                right += 1;
                            }
                        }
                    }
                    std::mem::swap(&mut values, &mut work);
                    width = width.saturating_mul(2);
                }
                for (k, value) in values.into_iter().enumerate() {
                    self.set_member(receiver.clone(), k.to_string(), value)?;
                }
                for k in count as u64..len {
                    self.tick()?;
                    self.remove_index(id, k)?;
                }
                Ok(receiver)
            }
            _ => Err(ScriptError::Unsupported("array operation")),
        }
    }
    fn sort_compare(&mut self, callback: &Value, a: &Value, b: &Value) -> Result<i8> {
        if *a == Value::Undefined {
            return Ok(if *b == Value::Undefined { 0 } else { 1 });
        }
        if *b == Value::Undefined {
            return Ok(-1);
        }
        if *callback != Value::Undefined {
            let value = self.invoke(callback.clone(), vec![a.clone(), b.clone()])?;
            let n = self.to_number(value)?;
            return Ok(if n.is_nan() || n == 0.0 {
                0
            } else if n < 0.0 {
                -1
            } else {
                1
            });
        }
        let a = self.to_string(a.clone())?;
        let b = self.to_string(b.clone())?;
        Ok(match a.encode_utf16().cmp(b.encode_utf16()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    }
}
