//! Typed view copying separates raw-bit moves from element conversion and species construction.
use super::{
    objects::Key,
    typed_arrays::Typed,
    vm::{Engine, ScriptError, Value},
};
use std::rc::Rc;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    fn typed_raw(&mut self, value: Value) -> Result<(crate::dual::Id, Typed)> {
        if let Value::Object(id) = value {
            if let Some(typed) = self.data(id).ok().and_then(|d| d.typed.clone()) {
                return Ok((id, typed));
            }
        }
        self.throw_error("TypeError", "receiver is not a typed array")
    }
    fn typed_bits(
        &mut self,
        source: &Typed,
        target: &Typed,
        from: usize,
        to: usize,
        count: usize,
        snapshot: bool,
    ) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        let size = source.kind.size();
        let source_store = self
            .data(source.view.buffer)?
            .buffer
            .as_ref()
            .unwrap()
            .clone();
        let target_store = self
            .data(target.view.buffer)?
            .buffer
            .as_ref()
            .unwrap()
            .clone();
        let from = source.view.offset + from * size;
        let to = target.view.offset + to * size;
        let bytes = count * size;
        if snapshot {
            let values = source_store.borrow().bytes[from..from + bytes].to_vec();
            target_store.borrow_mut().bytes[to..to + bytes].copy_from_slice(&values);
        } else if Rc::ptr_eq(&source_store, &target_store) {
            let mut buffer = source_store.borrow_mut();
            for index in 0..bytes {
                let value = buffer.bytes[from + index];
                buffer.bytes[to + index] = value;
            }
        } else {
            target_store.borrow_mut().bytes[to..to + bytes]
                .copy_from_slice(&source_store.borrow().bytes[from..from + bytes]);
        }
        Ok(())
    }
    pub(super) fn typed_copy_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        let (id, typed) = self.typed_raw(receiver.clone())?;
        if method == "subarray" {
            let length = self.typed_length(&typed)?.unwrap_or(0) as u64;
            let start = self.relative_index(arg(0), length)?;
            let offset = typed.view.offset + start as usize * typed.kind.size();
            let mut arguments = vec![
                Value::Object(typed.view.buffer),
                Value::number(offset as f64),
            ];
            if typed.view.length.is_some() || arg(1) != Value::Undefined {
                let end = if arg(1) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(1), length)?
                };
                arguments.push(Value::number(end.saturating_sub(start) as f64));
            }
            return self
                .typed_species(id, typed.kind, arguments, None)
                .map(|(value, _)| value);
        }
        if method == "set" {
            let offset = self.integer(arg(1))?;
            if offset < 0.0 {
                return self.throw_error("RangeError", "negative typed-array offset");
            }
            let (_, _, target_length) = self.typed_record(receiver)?;
            if let Value::Object(source) = arg(0) {
                if !self.is_proxy(source) && self.data(source)?.typed.is_some() {
                    let (_, source, length) = self.typed_record(Value::Object(source))?;
                    if !offset.is_finite()
                        || offset > target_length as f64
                        || length as f64 + offset > target_length as f64
                    {
                        return self
                            .throw_error("RangeError", "typed-array source exceeds destination");
                    }
                    if typed.kind.big() != source.kind.big() {
                        return self.throw_error("TypeError", "typed-array content types differ");
                    }
                    if source.kind == typed.kind {
                        self.typed_bits(&source, &typed, 0, offset as usize, length, true)?;
                    } else {
                        let mut values = vec![];
                        for index in 0..length {
                            self.tick()?;
                            values.push(self.typed_get(&source, index as f64)?);
                        }
                        for (index, value) in values.into_iter().enumerate() {
                            self.tick()?;
                            self.typed_write(&typed, offset + index as f64, value)?;
                        }
                    }
                    return Ok(Value::Undefined);
                }
            }
            let source = self.object_coercible(arg(0))?;
            self.protect(&Value::Object(source))?;
            let length = self.length_of(source)?;
            if !offset.is_finite()
                || offset > target_length as f64
                || length as f64 + offset > target_length as f64
            {
                return self.throw_error("RangeError", "typed-array source exceeds destination");
            }
            for index in 0..length {
                self.tick()?;
                self.frame(|e| {
                    let value = e.get_member(Value::Object(source), index.to_string())?;
                    e.protect(&value)?;
                    e.typed_write(&typed, offset + index as f64, value)
                })?;
            }
            return Ok(Value::Undefined);
        }
        let (_, _, length) = self.typed_record(receiver.clone())?;
        let len = length as u64;
        match method {
            "slice" => {
                let start = self.relative_index(arg(0), len)? as usize;
                let end = if arg(1) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(1), len)? as usize
                };
                let count = end.saturating_sub(start);
                let (output, target) = self.typed_species(
                    id,
                    typed.kind,
                    vec![Value::number(count as f64)],
                    Some(count),
                )?;
                if count != 0 {
                    let (_, _, current) = self.typed_record(receiver)?;
                    let end = end.min(current);
                    let count = end.saturating_sub(start);
                    if typed.kind == target.kind {
                        self.typed_bits(&typed, &target, start, 0, count, false)?;
                    } else {
                        for index in 0..count {
                            self.tick()?;
                            let value = self.typed_get(&typed, (start + index) as f64)?;
                            self.typed_write(&target, index as f64, value)?;
                        }
                    }
                }
                Ok(output)
            }
            "reverse" => {
                let store = self
                    .data(typed.view.buffer)?
                    .buffer
                    .as_ref()
                    .unwrap()
                    .clone();
                let size = typed.kind.size();
                for index in 0..length / 2 {
                    self.tick()?;
                    let left = typed.view.offset + index * size;
                    let right = typed.view.offset + (length - index - 1) * size;
                    let mut buffer = store.borrow_mut();
                    for byte in 0..size {
                        buffer.bytes.swap(left + byte, right + byte);
                    }
                }
                Ok(receiver)
            }
            "fill" => {
                let value = self.element_coerce(typed.kind, arg(0))?;
                let start = self.relative_index(arg(1), len)? as usize;
                let end = if arg(2) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(2), len)? as usize
                };
                let (_, _, current) = self.typed_record(receiver.clone())?;
                let end = end.min(current);
                let bytes = typed.kind.bytes(&value, true);
                let store = self
                    .data(typed.view.buffer)?
                    .buffer
                    .as_ref()
                    .unwrap()
                    .clone();
                for index in start..end {
                    self.tick()?;
                    let begin = typed.view.offset + index * bytes.len();
                    store.borrow_mut().bytes[begin..begin + bytes.len()].copy_from_slice(&bytes);
                }
                Ok(receiver)
            }
            "copyWithin" => {
                let to = self.relative_index(arg(0), len)? as usize;
                let from = self.relative_index(arg(1), len)? as usize;
                let end = if arg(2) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(2), len)? as usize
                };
                let count = end.saturating_sub(from).min(length - to);
                if count != 0 {
                    let (_, _, current) = self.typed_record(receiver.clone())?;
                    let count = count
                        .min(current.saturating_sub(from))
                        .min(current.saturating_sub(to));
                    if count != 0 {
                        let size = typed.kind.size();
                        let start = typed.view.offset + from * size;
                        let to = typed.view.offset + to * size;
                        let store = self
                            .data(typed.view.buffer)?
                            .buffer
                            .as_ref()
                            .unwrap()
                            .clone();
                        store
                            .borrow_mut()
                            .bytes
                            .copy_within(start..start + count * size, to);
                    }
                }
                Ok(receiver)
            }
            "with" | "toReversed" => {
                let replacement = if method == "with" {
                    let index = self.integer(arg(0))?;
                    let index = if index < 0.0 {
                        length as f64 + index
                    } else {
                        index
                    };
                    let value = self.element_coerce(typed.kind, arg(1))?;
                    if !self.typed_valid(&typed, index)? {
                        return self.throw_error("RangeError", "replacement index is out of range");
                    }
                    Some((index as usize, value))
                } else {
                    None
                };
                let output = self.typed_default(typed.kind, len)?;
                let (_, target, _) = self.typed_record(output.clone())?;
                for index in 0..length {
                    self.tick()?;
                    let value = if let Some((position, value)) = &replacement {
                        if *position == index {
                            value.clone()
                        } else {
                            self.typed_get(&typed, index as f64)?
                        }
                    } else {
                        self.typed_get(&typed, (length - index - 1) as f64)?
                    };
                    self.typed_write(&target, index as f64, value)?;
                }
                Ok(output)
            }
            _ => Err(ScriptError::Unsupported("typed-array copying method")),
        }
    }
    pub(super) fn typed_static(
        &mut self,
        name: &str,
        constructor: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if !self.constructible(&constructor) {
            return self.throw_error("TypeError", "TypedArray.from/of requires a constructor");
        }
        let of = name.ends_with(".of");
        let mapper = if of { Value::Undefined } else { arg(1) };
        if mapper != Value::Undefined && !self.callable(&mapper) {
            return self.throw_error("TypeError", "TypedArray.from mapper is not callable");
        }
        let this = arg(2);
        let mut input = None;
        let length = if of {
            args.len() as u64
        } else {
            let source = arg(0);
            let method = self.get_member(source.clone(), Key::Symbol(0))?;
            self.protect(&method)?;
            if matches!(method, Value::Null | Value::Undefined) {
                let source = self.object_coercible(source)?;
                self.protect(&Value::Object(source))?;
                input = Some(source);
                self.length_of(source)?
            } else {
                let list = self.new_array(0)?;
                self.protect(&Value::Object(list))?;
                self.consume_iterator(source, method, |e, value| {
                    e.append_array(list, Some(value), false)
                })?;
                input = Some(list);
                self.length_of(list)?
            }
        };
        let output = self.construct(constructor, vec![Value::number(length as f64)])?;
        self.protect(&output)?;
        let (_, typed, size) = self.typed_record(output.clone())?;
        if (size as u64) < length {
            return self.throw_error(
                "TypeError",
                "typed-array constructor returned a short result",
            );
        }
        for index in 0..length {
            self.tick()?;
            self.frame(|e| {
                let value = if of {
                    args[index as usize].clone()
                } else {
                    e.get_member(Value::Object(input.unwrap()), index.to_string())?
                };
                e.protect(&value)?;
                let value = if mapper == Value::Undefined {
                    value
                } else {
                    e.invoke_this(
                        mapper.clone(),
                        this.clone(),
                        vec![value, Value::number(index as f64)],
                    )?
                };
                e.typed_write(&typed, index as f64, value)
            })?;
        }
        Ok(output)
    }
}
