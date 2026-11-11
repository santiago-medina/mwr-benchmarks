//! Integer-indexed views reuse byte codecs and the ordinary property API at their boundary.
use super::{
    buffer_elements::Element,
    buffers::{Store, View, MAX_BYTES},
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) const NAMES: &[(&str, Element)] = &[
    ("Int8Array", Element::I8),
    ("Uint8Array", Element::U8),
    ("Uint8ClampedArray", Element::U8Clamped),
    ("Int16Array", Element::I16),
    ("Uint16Array", Element::U16),
    ("Int32Array", Element::I32),
    ("Uint32Array", Element::U32),
    ("Float16Array", Element::F16),
    ("Float32Array", Element::F32),
    ("Float64Array", Element::F64),
    ("BigInt64Array", Element::I64),
    ("BigUint64Array", Element::U64),
];
pub(super) fn kind(name: &str) -> Option<Element> {
    NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, kind)| *kind)
}
pub(super) fn numeric_index(key: &Key) -> Option<f64> {
    let Key::String(key) = key else {
        return None;
    };
    if key == "-0" {
        return Some(-0.0);
    }
    let number = match key.as_str() {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ => super::numeric::decimal_number(key),
    };
    if super::execute::number_string(number) == *key {
        Some(number)
    } else {
        None
    }
}
#[derive(Clone)]
pub(super) struct Typed {
    pub view: View,
    pub kind: Element,
}
impl Engine<'_> {
    pub(super) fn typed_length(&self, typed: &Typed) -> Result<Option<usize>> {
        let buffer = self
            .data(typed.view.buffer)?
            .buffer
            .as_ref()
            .unwrap()
            .borrow();
        Ok(typed.view.length(&buffer).map(|n| n / typed.kind.size()))
    }
    pub(super) fn typed_record(&mut self, value: Value) -> Result<(Id, Typed, usize)> {
        if let Value::Object(id) = value {
            if let Some(typed) = self.data(id).ok().and_then(|d| d.typed.clone()) {
                if let Some(length) = self.typed_length(&typed)? {
                    return Ok((id, typed, length));
                }
                return self.throw_error("TypeError", "typed view is detached or out of bounds");
            }
        }
        self.throw_error("TypeError", "receiver is not a typed array")
    }
    /// IsImmutableBuffer(O.[[ViewedArrayBuffer]]) (immutable-arraybuffer §25.1.3.4).
    pub(super) fn typed_immutable(&self, typed: &Typed) -> Result<bool> {
        self.buffer_immutable(typed.view.buffer)
    }
    /// ValidateTypedArray(O, order, accessMode) (immutable-arraybuffer §23.2.4.4): the
    /// write-mode immutable check comes after RequireInternalSlot and before the
    /// out-of-bounds check.
    pub(super) fn typed_record_mode(
        &mut self,
        value: Value,
        write: bool,
    ) -> Result<(Id, Typed, usize)> {
        if write {
            if let Value::Object(id) = value {
                if let Some(typed) = self.data(id).ok().and_then(|d| d.typed.clone()) {
                    if self.typed_immutable(&typed)? {
                        return self
                            .throw_error("TypeError", "cannot write to an immutable ArrayBuffer");
                    }
                }
            }
        }
        self.typed_record(value)
    }
    pub(super) fn typed_get(&self, typed: &Typed, index: f64) -> Result<Value> {
        let valid = self.typed_valid(typed, index)?;
        if !valid {
            return Ok(Value::Undefined);
        }
        let start = typed.view.offset + index as usize * typed.kind.size();
        let buffer = self
            .data(typed.view.buffer)?
            .buffer
            .as_ref()
            .unwrap()
            .borrow();
        let size = typed.kind.size();
        Ok(buffer
            .bytes
            .with(|s| typed.kind.read(&s[start..start + size], true)))
    }
    pub(super) fn typed_valid(&self, typed: &Typed, index: f64) -> Result<bool> {
        Ok(index.is_finite()
            && index.trunc() == index
            && index >= 0.0
            && !(index == 0.0 && index.is_sign_negative())
            && self
                .typed_length(typed)?
                .is_some_and(|len| index < len as f64))
    }
    pub(super) fn typed_write(&mut self, typed: &Typed, index: f64, value: Value) -> Result<()> {
        let value = self.element_coerce(typed.kind, value)?;
        if self.typed_valid(typed, index)? {
            let bytes = typed.kind.bytes(&value, true);
            let start = typed.view.offset + index as usize * typed.kind.size();
            let buffer = self
                .data(typed.view.buffer)?
                .buffer
                .as_ref()
                .unwrap()
                .clone();
            buffer.borrow_mut().bytes.set(start, &bytes);
        }
        Ok(())
    }
    pub(super) fn typed_define(
        &mut self,
        typed: &Typed,
        index: f64,
        descriptor: Descriptor,
    ) -> Result<bool> {
        // [[DefineOwnProperty]] (immutable-arraybuffer §10.4.5.3): an immutable element
        // is { value, writable: false, enumerable: true, configurable: false }, and
        // ValidateAndApplyPropertyDescriptor only accepts a compatible redefinition.
        if self.typed_valid(typed, index)? && self.typed_immutable(typed)? {
            let current = self.typed_get(typed, index)?;
            return Ok(descriptor.configurable != Some(true)
                && descriptor.enumerable != Some(false)
                && descriptor.get.is_none()
                && descriptor.set.is_none()
                && descriptor.writable != Some(true)
                && descriptor
                    .value
                    .as_ref()
                    .is_none_or(|v| v.same_value(&current)));
        }
        if !self.typed_valid(typed, index)?
            || descriptor.get.is_some()
            || descriptor.set.is_some()
            || descriptor.writable == Some(false)
            || descriptor.enumerable == Some(false)
            || descriptor.configurable == Some(false)
        {
            return Ok(false);
        }
        if let Some(value) = descriptor.value {
            self.typed_write(typed, index, value)?;
        }
        Ok(true)
    }
    pub(super) fn typed_fixed(&self, typed: &Typed) -> Result<bool> {
        Ok(typed.view.length.is_some()
            && self
                .data(typed.view.buffer)?
                .buffer
                .as_ref()
                .unwrap()
                .borrow()
                .maximum
                .is_none())
    }
    fn typed_attach(&mut self, prototype: Id, view: View, kind: Element) -> Result<Value> {
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.typed = Some(Typed { view, kind });
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    pub(super) fn typed_owned(
        &mut self,
        prototype: Id,
        kind: Element,
        length: u64,
    ) -> Result<Value> {
        if length > MAX_BYTES as u64 / kind.size() as u64 {
            return self.throw_error("RangeError", "host typed-array allocation limit");
        }
        let bytes = length as usize * kind.size();
        let buffer = self.new_buffer(bytes, None, None)?;
        let Value::Object(buffer) = buffer else {
            unreachable!()
        };
        self.typed_attach(
            prototype,
            View {
                buffer,
                offset: 0,
                length: Some(bytes),
            },
            kind,
        )
    }
    fn typed_from_buffer(
        &mut self,
        prototype: Id,
        kind: Element,
        source: Id,
        buffer: Store,
        args: &[Value],
    ) -> Result<Value> {
        let offset = self.to_index(args.get(1).cloned().unwrap_or(Value::Undefined))? as usize;
        if offset % kind.size() != 0 {
            return self.throw_error("RangeError", "unaligned typed-array offset");
        }
        let (initial, fixed, detached) = {
            let b = buffer.borrow();
            (b.bytes.len(), b.maximum.is_none(), b.detached)
        };
        if detached {
            return self.throw_error("TypeError", "detached buffer");
        }
        if offset > initial {
            return self.throw_error("RangeError", "typed-array offset exceeds buffer");
        }
        let length = if args.get(2).is_none_or(|v| *v == Value::Undefined) {
            if fixed {
                if (initial - offset) % kind.size() != 0 {
                    return self.throw_error("RangeError", "unaligned buffer length");
                }
                Some(initial - offset)
            } else {
                None
            }
        } else {
            let length = self.to_index(args[2].clone())?;
            if length > (initial - offset) as u64 / kind.size() as u64 {
                return self.throw_error("RangeError", "typed-array length exceeds buffer");
            }
            Some(length as usize * kind.size())
        };
        let invalid = {
            let b = buffer.borrow();
            if b.detached {
                1
            } else if offset > b.bytes.len() || length.is_some_and(|n| n > b.bytes.len() - offset) {
                2
            } else {
                0
            }
        };
        if invalid == 1 {
            return self.throw_error("TypeError", "detached buffer");
        }
        if invalid == 2 {
            return self.throw_error("RangeError", "typed view is outside resized buffer");
        }
        self.typed_attach(
            prototype,
            View {
                buffer: source,
                offset,
                length,
            },
            kind,
        )
    }
    pub(super) fn construct_typed(
        &mut self,
        name: &str,
        target: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let kind = kind(name).unwrap();
        let prototype = self.prototype_from_constructor(target, |i| i.collection[name])?;
        self.protect(&Value::Object(prototype))?;
        let source = args.first().cloned().unwrap_or(Value::Undefined);
        if let Value::Object(source_id) = source.clone() {
            let data = self.data(source_id)?.clone();
            if let Some(buffer) = data.buffer {
                return self.typed_from_buffer(prototype, kind, source_id, buffer, &args);
            }
            if let Some(typed) = data.typed {
                let Some(length) = self.typed_length(&typed)? else {
                    return self.throw_error("TypeError", "invalid source typed array");
                };
                if typed.kind.big() != kind.big() {
                    return self.throw_error("TypeError", "typed-array content types differ");
                }
                let output = self.typed_owned(prototype, kind, length as u64)?;
                let (_, destination, _) = self.typed_record(output.clone())?;
                if kind == typed.kind {
                    let bytes = self
                        .data(typed.view.buffer)?
                        .buffer
                        .as_ref()
                        .unwrap()
                        .borrow()
                        .bytes
                        .get(typed.view.offset, typed.view.offset + length * kind.size());
                    self.data(destination.view.buffer)?
                        .buffer
                        .as_ref()
                        .unwrap()
                        .borrow_mut()
                        .bytes
                        .set(0, &bytes);
                } else {
                    for index in 0..length {
                        self.tick()?;
                        let value = self.typed_get(&typed, index as f64)?;
                        self.typed_write(&destination, index as f64, value)?;
                    }
                }
                return Ok(output);
            }
            let method = self.get_member(source.clone(), Key::Symbol(0))?;
            self.protect(&method)?;
            let (input, length) = if matches!(method, Value::Null | Value::Undefined) {
                (source_id, self.length_of(source_id)?)
            } else {
                let list = self.new_array(0)?;
                self.protect(&Value::Object(list))?;
                self.consume_iterator(source, method, |e, v| e.append_array(list, Some(v), false))?;
                (list, self.length_of(list)?)
            };
            let output = self.typed_owned(prototype, kind, length)?;
            let (_, typed, _) = self.typed_record(output.clone())?;
            for index in 0..length {
                self.tick()?;
                self.frame(|e| {
                    let value = e.get_member(Value::Object(input), index.to_string())?;
                    e.protect(&value)?;
                    e.typed_write(&typed, index as f64, value)
                })?;
            }
            return Ok(output);
        }
        let length = self.to_index(source)?;
        self.typed_owned(prototype, kind, length)
    }
    pub(super) fn bootstrap_typed(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let object = i.object;
        let function = i.function;
        let base = self.intrinsic_function(env, "%TypedArray%", Some(function))?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .functions
            .insert("%TypedArray%".into(), base);
        self.define(
            base,
            "name",
            Descriptor {
                value: Some(Value::String("TypedArray".into())),
                ..Default::default()
            },
        )?;
        self.define(
            base,
            "length",
            Descriptor {
                value: Some(Value::number(0.0)),
                ..Default::default()
            },
        )?;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("TypedArray".into(), proto);
        self.define(
            base,
            "prototype",
            Descriptor {
                value: Some(Value::Object(proto)),
                ..Default::default()
            },
        )?;
        self.hidden(proto, "constructor", Value::Object(base))?;
        for getter in ["buffer", "byteOffset", "byteLength", "length", "tag"] {
            let full = format!("%TypedArray%.prototype.{getter}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.define(
                f,
                "name",
                Descriptor {
                    value: Some(Value::String(if getter == "tag" {
                        "get [Symbol.toStringTag]".into()
                    } else {
                        (format!("get {getter}")).into()
                    })),
                    ..Default::default()
                },
            )?;
            self.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(0.0)),
                    ..Default::default()
                },
            )?;
            self.define(
                proto,
                if getter == "tag" {
                    Key::Symbol(1)
                } else {
                    Key::from(getter)
                },
                Descriptor {
                    get: Some(Value::Object(f)),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
        }
        for method in ["keys", "values", "entries"] {
            let full = format!("%TypedArray%.prototype.{method}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(0.0)),
                    ..Default::default()
                },
            )?;
            self.hidden(proto, method, Value::Object(f))?;
            if method == "values" {
                self.hidden(proto, Key::Symbol(0), Value::Object(f))?;
            }
        }
        let species = self.intrinsic_function(env, "%TypedArray%.species", Some(function))?;
        self.define(
            species,
            "length",
            Descriptor {
                value: Some(Value::number(0.0)),
                ..Default::default()
            },
        )?;
        self.define(
            species,
            "name",
            Descriptor {
                value: Some(Value::String("get [Symbol.species]".into())),
                ..Default::default()
            },
        )?;
        self.define(
            base,
            Key::Symbol(2),
            Descriptor {
                get: Some(Value::Object(species)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        self.bootstrap_typed_methods(env, base, proto, function)?;
        for (name, kind) in NAMES {
            let prototype = self.new_object(Some(proto))?;
            self.runtime.link(env, prototype)?;
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert((*name).into(), prototype);
            objects.insert(format!("{name}.prototype"), prototype);
            let ctor = objects[*name];
            let mut data = self.data(ctor)?.clone();
            data.prototype = Some(base);
            self.replace_data(ctor, data)?;
            self.define(
                ctor,
                "length",
                Descriptor {
                    value: Some(Value::number(3.0)),
                    ..Default::default()
                },
            )?;
            if *name == "Uint8Array" {
                // Uint8Array base64/hex methods (ECMA-262 §23.3.2-3; see base64.rs).
                for (target, method) in [
                    (ctor, "fromBase64"),
                    (ctor, "fromHex"),
                    (prototype, "toBase64"),
                    (prototype, "toHex"),
                    (prototype, "setFromBase64"),
                    (prototype, "setFromHex"),
                ] {
                    let full = if target == ctor {
                        format!("Uint8Array.{method}")
                    } else {
                        format!("Uint8Array.prototype.{method}")
                    };
                    let f = self.intrinsic_function(env, &full, Some(function))?;
                    self.hidden(target, method, Value::Object(f))?;
                }
            }
            for object in [ctor, prototype] {
                self.define(
                    object,
                    "BYTES_PER_ELEMENT",
                    Descriptor {
                        value: Some(Value::number(kind.size() as f64)),
                        ..Default::default()
                    },
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn typed_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if matches!(name, "%TypedArray%.from" | "%TypedArray%.of") {
            return self.typed_static(name, receiver, args);
        }
        if kind(name).is_some() || name == "%TypedArray%" {
            return self.throw_error(
                "TypeError",
                "typed-array constructor requires a concrete new target",
            );
        }
        if name == "%TypedArray%.species" {
            return Ok(receiver);
        }
        let method = name.rsplit('.').next().unwrap();
        if method == "tag" {
            return Ok(if let Value::Object(id) = receiver {
                self.data(id)
                    .ok()
                    .and_then(|d| d.typed.as_ref())
                    .and_then(|t| NAMES.iter().find(|(_, kind)| *kind == t.kind))
                    .map(|(name, _)| Value::String((*name).into()))
                    .unwrap_or(Value::Undefined)
            } else {
                Value::Undefined
            });
        }
        let Value::Object(id) = receiver.clone() else {
            return self.throw_error("TypeError", "invalid typed-array receiver");
        };
        let Some(typed) = self.data(id).ok().and_then(|d| d.typed.clone()) else {
            return self.throw_error("TypeError", "invalid typed-array receiver");
        };
        if method == "buffer" {
            return Ok(Value::Object(typed.view.buffer));
        }
        let length = self.typed_length(&typed)?;
        match method {
            "length" => Ok(Value::number(length.unwrap_or(0) as f64)),
            "byteLength" => Ok(Value::number(
                (length.unwrap_or(0) * typed.kind.size()) as f64,
            )),
            "byteOffset" => Ok(Value::number(if length.is_some() {
                typed.view.offset
            } else {
                0
            } as f64)),
            "keys" | "values" | "entries" => {
                self.typed_record(receiver.clone())?;
                self.array_iterator(
                    receiver,
                    if method == "keys" {
                        0
                    } else if method == "values" {
                        1
                    } else {
                        2
                    },
                )
            }
            _ => self.typed_method(method, receiver, args),
        }
    }
}
