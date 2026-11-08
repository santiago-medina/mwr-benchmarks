//! Byte stores contain no JS references. Views retain their buffer through the traced graph.
//! Spec reference: https://tc39.es/ecma262/multipage/structured-data.html
use super::{
    buffer_elements::{Element, ELEMENTS},
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) const MAX_BYTES: usize = 16777216;
const TOTAL_BYTES: usize = 67108864;
pub(super) type Store = Rc<RefCell<Buffer>>;
pub(super) struct Buffer {
    pub bytes: Vec<u8>,
    pub maximum: Option<usize>,
    pub detached: bool,
    budget: Rc<Cell<usize>>,
}
impl Drop for Buffer {
    fn drop(&mut self) {
        self.budget
            .set(self.budget.get().saturating_sub(self.bytes.len()));
    }
}
#[derive(Clone)]
pub(super) struct View {
    pub buffer: Id,
    pub offset: usize,
    pub length: Option<usize>,
}
impl View {
    pub fn length(&self, buffer: &Buffer) -> Option<usize> {
        if buffer.detached || self.offset > buffer.bytes.len() {
            return None;
        }
        let length = self.length.unwrap_or(buffer.bytes.len() - self.offset);
        if length > buffer.bytes.len() - self.offset {
            None
        } else {
            Some(length)
        }
    }
}
impl Engine<'_> {
    pub(super) fn to_index(&mut self, value: Value) -> Result<u64> {
        let n = self.integer(value)?;
        if !(0.0..=9007199254740991.0).contains(&n) {
            return self.throw_error("RangeError", "invalid index");
        }
        Ok(n as u64)
    }
    pub(super) fn buffer_slot(&mut self, value: Value) -> Result<Store> {
        if let Value::Object(id) = value {
            if let Some(buffer) = self.data(id).ok().and_then(|d| d.buffer.clone()) {
                return Ok(buffer);
            }
        }
        self.throw_error("TypeError", "receiver is not an ArrayBuffer")
    }
    fn view_slot(&mut self, value: Value) -> Result<View> {
        if let Value::Object(id) = value {
            if let Some(view) = self.data(id).ok().and_then(|d| d.view.clone()) {
                return Ok(view);
            }
        }
        self.throw_error("TypeError", "receiver is not a DataView")
    }
    fn allocate_bytes(&mut self, length: usize, replacing: usize) -> Result<Vec<u8>> {
        if length > MAX_BYTES
            || self
                .buffer_bytes
                .get()
                .saturating_sub(replacing)
                .saturating_add(length)
                > TOTAL_BYTES
        {
            return self.throw_error("RangeError", "host buffer allocation limit");
        }
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(length).is_err() {
            return self.throw_error("RangeError", "buffer allocation failed");
        }
        bytes.resize(length, 0);
        Ok(bytes)
    }
    pub(super) fn new_buffer(
        &mut self,
        length: usize,
        maximum: Option<usize>,
        prototype: Option<Id>,
    ) -> Result<Value> {
        if maximum.is_some_and(|max| max > MAX_BYTES || max < length) {
            return self.throw_error("RangeError", "invalid buffer maximum");
        }
        let prototype =
            prototype.unwrap_or(self.intrinsics.as_ref().unwrap().collection["ArrayBuffer"]);
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        let bytes = self.allocate_bytes(length, 0)?;
        self.buffer_bytes.set(self.buffer_bytes.get() + length);
        let buffer = Rc::new(RefCell::new(Buffer {
            bytes,
            maximum,
            detached: false,
            budget: self.buffer_bytes.clone(),
        }));
        let mut data = self.data(id)?.clone();
        data.buffer = Some(buffer);
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    fn detach_buffer(buffer: &mut Buffer) {
        buffer
            .budget
            .set(buffer.budget.get().saturating_sub(buffer.bytes.len()));
        buffer.bytes = vec![];
        buffer.detached = true;
    }
    pub(super) fn construct_buffer(
        &mut self,
        name: &str,
        target: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if name == "ArrayBuffer" {
            let length = self.to_index(arg(0))?;
            let maximum = if let Value::Object(options) = arg(1) {
                let value = self.get_member(Value::Object(options), "maxByteLength")?;
                if value == Value::Undefined {
                    None
                } else {
                    Some(self.to_index(value)? as usize)
                }
            } else {
                None
            };
            if length > MAX_BYTES as u64 {
                return self.throw_error("RangeError", "host buffer allocation limit");
            }
            if maximum.is_some_and(|max| max < length as usize || max > MAX_BYTES) {
                return self.throw_error("RangeError", "invalid buffer maximum");
            }
            let prototype = self.get_member(target, "prototype")?;
            return self.new_buffer(
                length as usize,
                maximum,
                if let Value::Object(id) = prototype {
                    Some(id)
                } else {
                    None
                },
            );
        }
        let source = arg(0);
        self.protect(&source)?;
        let buffer = self.buffer_slot(source.clone())?;
        let offset = self.to_index(arg(1))? as usize;
        let (initial, fixed, detached) = {
            let b = buffer.borrow();
            (b.bytes.len(), b.maximum.is_none(), b.detached)
        };
        if detached {
            return self.throw_error("TypeError", "detached buffer");
        }
        if offset > initial {
            return self.throw_error("RangeError", "view offset exceeds buffer");
        }
        let explicit = arg(2) != Value::Undefined;
        let length = if explicit {
            let length = self.to_index(arg(2))? as usize;
            if length > initial - offset {
                return self.throw_error("RangeError", "view length exceeds buffer");
            }
            Some(length)
        } else if fixed {
            Some(initial - offset)
        } else {
            None
        };
        let prototype = self.get_member(target, "prototype")?;
        let proto = if let Value::Object(id) = prototype {
            id
        } else {
            self.intrinsics.as_ref().unwrap().collection["DataView"]
        };
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        let invalid = {
            let b = buffer.borrow();
            if b.detached {
                1
            } else if offset > b.bytes.len() || explicit && length.unwrap() > b.bytes.len() - offset
            {
                2
            } else {
                0
            }
        };
        if invalid == 1 {
            return self.throw_error("TypeError", "detached buffer");
        }
        if invalid == 2 {
            return self.throw_error("RangeError", "view is outside resized buffer");
        }
        let Value::Object(source) = source else {
            unreachable!()
        };
        let mut data = self.data(id)?.clone();
        data.view = Some(View {
            buffer: source,
            offset,
            length,
        });
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    pub(super) fn element_coerce(&mut self, kind: Element, value: Value) -> Result<Value> {
        if kind.big() {
            let n = self.to_bigint(value, false)?;
            self.big_value(n)
        } else {
            self.to_number(value).map(Value::number)
        }
    }
    pub(super) fn bootstrap_buffers(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let object = i.object;
        let function = i.function;
        for name in ["ArrayBuffer", "DataView"] {
            let prototype = self.new_object(Some(object))?;
            self.runtime.link(env, prototype)?;
            objects.insert(format!("{name}.prototype"), prototype);
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert(name.into(), prototype);
            self.define(
                prototype,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String(name.into())),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
            let getters: &[&str] = if name == "ArrayBuffer" {
                &["byteLength", "detached", "maxByteLength", "resizable"]
            } else {
                &["buffer", "byteLength", "byteOffset"]
            };
            for getter in getters {
                let full = format!("{name}.prototype.{getter}");
                let f = self.intrinsic_function(env, &full, Some(function))?;
                self.define(
                    f,
                    "name",
                    Descriptor {
                        value: Some(Value::String(format!("get {getter}"))),
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
                    prototype,
                    *getter,
                    Descriptor {
                        get: Some(Value::Object(f)),
                        configurable: Some(true),
                        ..Default::default()
                    },
                )?;
            }
            let methods: Vec<(String, usize)> = if name == "ArrayBuffer" {
                vec![
                    ("slice".into(), 2),
                    ("resize".into(), 1),
                    ("transfer".into(), 0),
                    ("transferToFixedLength".into(), 0),
                ]
            } else {
                ELEMENTS
                    .iter()
                    .filter(|(_, kind)| *kind != Element::U8Clamped)
                    .flat_map(|(kind, _)| [(format!("get{kind}"), 1), (format!("set{kind}"), 2)])
                    .collect()
            };
            for (method, length) in methods {
                let full = format!("{name}.prototype.{method}");
                let f = self.intrinsic_function(env, &full, Some(function))?;
                self.define(
                    f,
                    "length",
                    Descriptor {
                        value: Some(Value::number(length as f64)),
                        ..Default::default()
                    },
                )?;
                self.hidden(prototype, method, Value::Object(f))?;
            }
        }
        let view = self.intrinsic_function(env, "ArrayBuffer.isView", Some(function))?;
        self.hidden(objects["ArrayBuffer"], "isView", Value::Object(view))?;
        let species = self.intrinsic_function(env, "ArrayBuffer.species", Some(function))?;
        self.define(
            species,
            "name",
            Descriptor {
                value: Some(Value::String("get [Symbol.species]".into())),
                ..Default::default()
            },
        )?;
        self.define(
            species,
            "length",
            Descriptor {
                value: Some(Value::number(0.0)),
                ..Default::default()
            },
        )?;
        self.define(
            objects["ArrayBuffer"],
            Key::Symbol(2),
            Descriptor {
                get: Some(Value::Object(species)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub(super) fn buffer_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if matches!(name, "ArrayBuffer" | "DataView") {
            return self.throw_error("TypeError", "buffer/view constructor requires new");
        }
        if name == "ArrayBuffer.species" {
            return Ok(receiver);
        }
        if name == "ArrayBuffer.isView" {
            return Ok(Value::Bool(
                matches!(arg(0),Value::Object(id) if self.data(id).is_ok_and(|d|d.view.is_some()||d.typed.is_some())),
            ));
        }
        let method = name.rsplit('.').next().unwrap();
        if name.starts_with("DataView.") {
            let view = self.view_slot(receiver)?;
            if method == "buffer" {
                return Ok(Value::Object(view.buffer));
            }
            let buffer = self.buffer_slot(Value::Object(view.buffer))?;
            if matches!(method, "byteLength" | "byteOffset") {
                let length = view.length(&buffer.borrow());
                return Ok(Value::number(if method == "byteLength" {
                    length.unwrap_or(0)
                } else if length.is_some() {
                    view.offset
                } else {
                    0
                } as f64));
            }
            let set = method.starts_with("set");
            let kind = ELEMENTS
                .iter()
                .find(|(name, _)| *name == &method[3..])
                .map(|(_, kind)| *kind)
                .ok_or(ScriptError::Unsupported("DataView element"))?;
            let index = self.to_index(arg(0))?;
            let value = if set {
                Some(self.element_coerce(kind, arg(1))?)
            } else {
                None
            };
            let little = arg(if set { 2 } else { 1 }).truthy();
            let length = view.length(&buffer.borrow());
            let Some(length) = length else {
                return self.throw_error("TypeError", "view is detached or out of bounds");
            };
            if index > length as u64 || kind.size() as u64 > length as u64 - index {
                return self.throw_error("RangeError", "view access exceeds bounds");
            }
            let start = view.offset + index as usize;
            if let Some(value) = value {
                let bytes = kind.bytes(&value, little);
                buffer.borrow_mut().bytes[start..start + kind.size()].copy_from_slice(&bytes);
                return Ok(Value::Undefined);
            }
            return Ok(kind.read(&buffer.borrow().bytes[start..start + kind.size()], little));
        }
        let source = receiver.clone();
        let buffer = self.buffer_slot(receiver)?;
        match method {
            "byteLength" => {
                let length = buffer.borrow().bytes.len();
                Ok(Value::number(length as f64))
            }
            "detached" => {
                let detached = buffer.borrow().detached;
                Ok(Value::Bool(detached))
            }
            "resizable" => {
                let resizable = buffer.borrow().maximum.is_some();
                Ok(Value::Bool(resizable))
            }
            "maxByteLength" => {
                let b = buffer.borrow();
                Ok(Value::number(if b.detached {
                    0
                } else {
                    b.maximum.unwrap_or(b.bytes.len())
                } as f64))
            }
            "resize" => {
                if buffer.borrow().maximum.is_none() {
                    return self.throw_error("TypeError", "buffer is not resizable");
                }
                let length = self.to_index(arg(0))?;
                let (detached, max, old) = {
                    let b = buffer.borrow();
                    (b.detached, b.maximum.unwrap(), b.bytes.len())
                };
                if detached {
                    return self.throw_error("TypeError", "detached buffer");
                }
                if length > max as u64 {
                    return self.throw_error("RangeError", "buffer resize exceeds maximum");
                }
                let mut bytes = self.allocate_bytes(length as usize, old)?;
                let mut b = buffer.borrow_mut();
                let count = bytes.len().min(b.bytes.len());
                bytes[..count].copy_from_slice(&b.bytes[..count]);
                self.buffer_bytes
                    .set(self.buffer_bytes.get() - old + bytes.len());
                b.bytes = bytes;
                Ok(Value::Undefined)
            }
            "transfer" | "transferToFixedLength" => {
                let old = buffer.borrow().bytes.len();
                let length = if arg(0) == Value::Undefined {
                    old as u64
                } else {
                    self.to_index(arg(0))?
                };
                if buffer.borrow().detached {
                    return self.throw_error("TypeError", "detached buffer");
                }
                if length > MAX_BYTES as u64 {
                    return self.throw_error("RangeError", "host buffer allocation limit");
                }
                let maximum = if method == "transfer" {
                    buffer.borrow().maximum
                } else {
                    None
                };
                let output = self.new_buffer(length as usize, maximum, None)?;
                let target = self.buffer_slot(output.clone())?;
                let mut source = buffer.borrow_mut();
                let count = source.bytes.len().min(length as usize);
                target.borrow_mut().bytes[..count].copy_from_slice(&source.bytes[..count]);
                Self::detach_buffer(&mut source);
                Ok(output)
            }
            "slice" => {
                if buffer.borrow().detached {
                    return self.throw_error("TypeError", "detached buffer");
                }
                let length = buffer.borrow().bytes.len() as u64;
                let first = self.relative_index(arg(0), length)?;
                let end = if arg(1) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(1), length)?
                };
                let count = end.saturating_sub(first);
                let default =
                    Value::Object(self.intrinsics.as_ref().unwrap().functions["ArrayBuffer"]);
                let constructor = self.get_member(source.clone(), "constructor")?;
                self.protect(&constructor)?;
                let constructor = if constructor == Value::Undefined {
                    default
                } else {
                    if !matches!(constructor, Value::Object(_)) {
                        return self
                            .throw_error("TypeError", "buffer constructor is not an object");
                    }
                    let species = self.get_member(constructor, Key::Symbol(2))?;
                    if matches!(species, Value::Null | Value::Undefined) {
                        default
                    } else {
                        species
                    }
                };
                self.protect(&constructor)?;
                let output = self.construct(constructor, vec![Value::number(count as f64)])?;
                self.protect(&output)?;
                let target = self.buffer_slot(output.clone())?;
                if output == source
                    || Rc::ptr_eq(&target, &buffer)
                    || target.borrow().detached
                    || target.borrow().bytes.len() < count as usize
                {
                    return self.throw_error("TypeError", "invalid buffer species result");
                }
                if buffer.borrow().detached {
                    return self.throw_error("TypeError", "detached source buffer");
                }
                let b = buffer.borrow();
                let count = (count as usize).min(b.bytes.len().saturating_sub(first as usize));
                if count != 0 {
                    target.borrow_mut().bytes[..count]
                        .copy_from_slice(&b.bytes[first as usize..first as usize + count]);
                }
                Ok(output)
            }
            _ => Err(ScriptError::Unsupported("buffer operation")),
        }
    }
}
