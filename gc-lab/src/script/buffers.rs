//! Byte stores contain no JS references. Views retain their buffer through the traced graph.
//! Spec reference: https://tc39.es/ecma262/multipage/structured-data.html
use super::shared_block::{Bytes, SharedBlock};
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
    sync::Arc,
};
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) const MAX_BYTES: usize = 16777216;
const TOTAL_BYTES: usize = 67108864;
pub(super) type Store = Rc<RefCell<Buffer>>;
pub(super) struct Buffer {
    pub bytes: Bytes,
    pub maximum: Option<usize>,
    pub detached: bool,
    /// IsSharedArrayBuffer (§25.2.2.2): set for SharedArrayBuffer stores, which never
    /// detach. Their bytes are a `Bytes::Shared` block other agents may hold (AGENTS.md).
    pub shared: bool,
    /// [[ArrayBufferIsImmutable]] (proposal-immutable-arraybuffer §25.1.7): the bytes
    /// never change after AllocateImmutableArrayBuffer and the buffer never detaches.
    pub immutable: bool,
    budget: Rc<Cell<usize>>,
    /// Bytes charged to this engine's host budget. For a shared block another agent may
    /// have grown it; that growth is charged to that agent.
    pub accounted: usize,
}
impl Buffer {
    /// Charge a new length to the engine budget (resize, grow, detach).
    pub fn account(&mut self, length: usize) {
        self.budget
            .set(self.budget.get().saturating_sub(self.accounted) + length);
        self.accounted = length;
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        self.budget
            .set(self.budget.get().saturating_sub(self.accounted));
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
        let total = buffer.bytes.len();
        if buffer.detached || self.offset > total {
            return None;
        }
        let length = self.length.unwrap_or(total - self.offset);
        if length > total - self.offset {
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
    /// RequireInternalSlot(O, [[ArrayBufferData]]) plus the IsSharedArrayBuffer check that
    /// ArrayBuffer.prototype (false) and SharedArrayBuffer.prototype (true) methods make.
    fn buffer_receiver(&mut self, value: Value, shared: bool) -> Result<Store> {
        if let Value::Object(id) = value {
            if let Some(buffer) = self.data(id).ok().and_then(|d| d.buffer.clone()) {
                if buffer.borrow().shared == shared {
                    return Ok(buffer);
                }
            }
        }
        if shared {
            self.throw_error("TypeError", "receiver is not a SharedArrayBuffer")
        } else {
            self.throw_error("TypeError", "receiver is not an ArrayBuffer")
        }
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
        self.new_buffer_kind(length, maximum, prototype, false)
    }
    /// AllocateArrayBuffer (§25.1.3.1) / AllocateSharedArrayBuffer (§25.2.2.1).
    pub(super) fn new_buffer_kind(
        &mut self,
        length: usize,
        maximum: Option<usize>,
        prototype: Option<Id>,
        shared: bool,
    ) -> Result<Value> {
        if maximum.is_some_and(|max| max > MAX_BYTES || max < length) {
            return self.throw_error("RangeError", "invalid buffer maximum");
        }
        let prototype = prototype.unwrap_or(
            self.intrinsics.as_ref().unwrap().collection[if shared {
                "SharedArrayBuffer"
            } else {
                "ArrayBuffer"
            }],
        );
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        let bytes = self.allocate_bytes(length, 0)?;
        self.buffer_bytes.set(self.buffer_bytes.get() + length);
        let bytes = if shared {
            // A growable shared block reserves maxByteLength now, so `grow` never reallocates
            // under another agent's feet (what V8, SpiderMonkey and wasmtime do). The host
            // budget is still charged the byte length only; see AGENTS.md.
            let mut bytes = bytes;
            if let Some(max) = maximum {
                if bytes.try_reserve_exact(max - length).is_err() {
                    self.buffer_bytes.set(self.buffer_bytes.get() - length);
                    return self.throw_error(
                        "RangeError",
                        "cannot reserve SharedArrayBuffer maxByteLength",
                    );
                }
            }
            Bytes::Shared(SharedBlock::new(bytes))
        } else {
            Bytes::Local(bytes)
        };
        let buffer = Rc::new(RefCell::new(Buffer {
            bytes,
            maximum,
            detached: false,
            shared,
            immutable: false,
            budget: self.buffer_bytes.clone(),
            accounted: length,
        }));
        let mut data = self.data(id)?.clone();
        data.buffer = Some(buffer);
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    /// A SharedArrayBuffer object in this agent over a block another agent created (the
    /// test262 agent broadcast; later, structured clone to a worker). Its bytes were charged to
    /// the creating agent, so this one charges nothing until it grows the block itself.
    pub(super) fn shared_buffer_from_block(
        &mut self,
        block: Arc<SharedBlock>,
        maximum: Option<usize>,
    ) -> Result<Value> {
        let prototype = self.intrinsics.as_ref().unwrap().collection["SharedArrayBuffer"];
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        let buffer = Rc::new(RefCell::new(Buffer {
            bytes: Bytes::Shared(block),
            maximum,
            detached: false,
            shared: true,
            immutable: false,
            budget: self.buffer_bytes.clone(),
            accounted: 0,
        }));
        let mut data = self.data(id)?.clone();
        data.buffer = Some(buffer);
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    /// IsImmutableBuffer (immutable-arraybuffer §25.1.3.4) for the buffer object `buffer`.
    pub(super) fn buffer_immutable(&self, buffer: Id) -> Result<bool> {
        Ok(self
            .data(buffer)?
            .buffer
            .as_ref()
            .is_some_and(|b| b.borrow().immutable))
    }
    pub(super) fn detach_buffer(buffer: &mut Buffer) {
        buffer.account(0);
        buffer.bytes = Bytes::Local(vec![]);
        buffer.detached = true;
    }
    pub(super) fn construct_buffer(
        &mut self,
        name: &str,
        target: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if name == "ArrayBuffer" || name == "SharedArrayBuffer" {
            // §25.1.4.1 ArrayBuffer ( length [ , options ] ) / §25.2.3.1 SharedArrayBuffer.
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
            // AllocateArrayBuffer step 2: byteLength > maxByteLength is checked first; the
            // host allocation limits only apply after OrdinaryCreateFromConstructor (step 3).
            if maximum.is_some_and(|max| (max as u64) < length) {
                return self.throw_error("RangeError", "invalid buffer maximum");
            }
            let shared = name == "SharedArrayBuffer";
            let prototype = self.prototype_from_constructor(target, |i| {
                i.collection[if shared {
                    "SharedArrayBuffer"
                } else {
                    "ArrayBuffer"
                }]
            })?;
            if length > MAX_BYTES as u64 {
                return self.throw_error("RangeError", "host buffer allocation limit");
            }
            if maximum.is_some_and(|max| max > MAX_BYTES) {
                return self.throw_error("RangeError", "invalid buffer maximum");
            }
            return self.new_buffer_kind(length as usize, maximum, Some(prototype), shared);
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
        let proto = self.prototype_from_constructor(target, |i| i.collection["DataView"])?;
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
        for name in ["ArrayBuffer", "SharedArrayBuffer", "DataView"] {
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
                &[
                    "byteLength",
                    "detached",
                    "immutable",
                    "maxByteLength",
                    "resizable",
                ]
            } else if name == "SharedArrayBuffer" {
                &["byteLength", "growable", "maxByteLength"]
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
                        value: Some(Value::String((format!("get {getter}")).into())),
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
                    ("transferToImmutable".into(), 0),
                    ("sliceToImmutable".into(), 2),
                ]
            } else if name == "SharedArrayBuffer" {
                vec![("grow".into(), 1), ("slice".into(), 2)]
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
        for owner in ["ArrayBuffer", "SharedArrayBuffer"] {
            self.buffer_species(env, function, objects[owner], &format!("{owner}.species"))?;
        }
        Ok(())
    }
    fn buffer_species(&mut self, env: Id, function: Id, owner: Id, name: &str) -> Result<()> {
        let species = self.intrinsic_function(env, name, Some(function))?;
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
            owner,
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
        if matches!(name, "ArrayBuffer" | "SharedArrayBuffer" | "DataView") {
            return self.throw_error("TypeError", "buffer/view constructor requires new");
        }
        if name == "ArrayBuffer.species" || name == "SharedArrayBuffer.species" {
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
            // SetViewValue step 3 (immutable-arraybuffer §25.3.1.6): before ToIndex.
            if set && buffer.borrow().immutable {
                return self.throw_error("TypeError", "cannot write to an immutable ArrayBuffer");
            }
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
                buffer.borrow_mut().bytes.set(start, &bytes);
                return Ok(Value::Undefined);
            }
            return Ok(buffer
                .borrow()
                .bytes
                .with(|s| kind.read(&s[start..start + kind.size()], little)));
        }
        let source = receiver.clone();
        if name.starts_with("SharedArrayBuffer.") {
            return self.shared_buffer_builtin(method, receiver, args);
        }
        let buffer = self.buffer_receiver(receiver, false)?;
        match method {
            "byteLength" => {
                let length = buffer.borrow().bytes.len();
                Ok(Value::number(length as f64))
            }
            "detached" => {
                let detached = buffer.borrow().detached;
                Ok(Value::Bool(detached))
            }
            // get ArrayBuffer.prototype.immutable (immutable-arraybuffer §25.1.6.1).
            "immutable" => {
                let immutable = buffer.borrow().immutable;
                Ok(Value::Bool(immutable))
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
                let count = bytes.len().min(old);
                b.bytes
                    .with(|s| bytes[..count].copy_from_slice(&s[..count]));
                b.account(bytes.len());
                b.bytes = Bytes::Local(bytes);
                Ok(Value::Undefined)
            }
            // ArrayBufferCopyAndDetach (immutable-arraybuffer §25.1.3.3).
            "transfer" | "transferToFixedLength" | "transferToImmutable" => {
                let old = buffer.borrow().bytes.len();
                let length = if arg(0) == Value::Undefined {
                    old as u64
                } else {
                    self.to_index(arg(0))?
                };
                if buffer.borrow().detached {
                    return self.throw_error("TypeError", "detached buffer");
                }
                if buffer.borrow().immutable {
                    return self.throw_error("TypeError", "cannot detach an immutable ArrayBuffer");
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
                let values = source.bytes.get(0, count);
                target.borrow_mut().bytes.set(0, &values);
                target.borrow_mut().immutable = method == "transferToImmutable";
                Self::detach_buffer(&mut source);
                Ok(output)
            }
            // ArrayBuffer.prototype.sliceToImmutable (immutable-arraybuffer §25.1.6.8).
            "sliceToImmutable" => {
                if buffer.borrow().detached {
                    return self.throw_error("TypeError", "detached buffer");
                }
                let length = buffer.borrow().bytes.len() as u64;
                // ResolveBounds (§7.1).
                let first = self.relative_index(arg(0), length)?;
                let end = if arg(1) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(1), length)?
                };
                let count = end.saturating_sub(first) as usize;
                if buffer.borrow().detached {
                    return self.throw_error("TypeError", "detached buffer");
                }
                if (buffer.borrow().bytes.len() as u64) < end {
                    return self.throw_error("RangeError", "buffer shrank below the slice end");
                }
                // AllocateImmutableArrayBuffer (§25.1.3.2).
                let output = self.new_buffer(count, None, None)?;
                let target = self.buffer_slot(output.clone())?;
                let first = first as usize;
                let mut t = target.borrow_mut();
                let values = buffer.borrow().bytes.get(first, first + count);
                t.bytes.set(0, &values);
                t.immutable = true;
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
                let target = self.buffer_receiver(output.clone(), false)?;
                if target.borrow().detached || target.borrow().immutable {
                    return self.throw_error("TypeError", "invalid buffer species result");
                }
                if output == source
                    || Rc::ptr_eq(&target, &buffer)
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
                    let values = b.bytes.get(first as usize, first as usize + count);
                    target.borrow_mut().bytes.set(0, &values);
                }
                Ok(output)
            }
            _ => Err(ScriptError::Unsupported("buffer operation")),
        }
    }
    /// SharedArrayBuffer.prototype getters and methods (§25.2.5).
    fn shared_buffer_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        let source = receiver.clone();
        let buffer = self.buffer_receiver(receiver, true)?;
        match method {
            // §25.2.5.2 get byteLength: ArrayBufferByteLength(O, seq-cst).
            "byteLength" => Ok(Value::number(buffer.borrow().bytes.len() as f64)),
            // §25.2.5.4 get growable: not IsFixedLengthArrayBuffer(O).
            "growable" => Ok(Value::Bool(buffer.borrow().maximum.is_some())),
            // §25.2.5.5 get maxByteLength.
            "maxByteLength" => {
                let b = buffer.borrow();
                Ok(Value::number(b.maximum.unwrap_or(b.bytes.len()) as f64))
            }
            // §25.2.5.3 grow ( newLength ).
            "grow" => {
                // Step 3: not growable → TypeError (before ToIndex).
                let Some(max) = buffer.borrow().maximum else {
                    return self.throw_error("TypeError", "SharedArrayBuffer is not growable");
                };
                let length = self.to_index(arg(0))?;
                if length > max as u64 {
                    return self.throw_error("RangeError", "grow length exceeds maxByteLength");
                }
                let old = buffer.borrow().bytes.len();
                if (length as usize) < old {
                    return self.throw_error("RangeError", "SharedArrayBuffer cannot shrink");
                }
                // Budget check only; the block grows in place under its lock so that a
                // concurrent grow by another agent is never lost (§25.2.5.3 steps 9-12).
                let accounted = buffer.borrow().accounted;
                if length as usize > MAX_BYTES
                    || self
                        .buffer_bytes
                        .get()
                        .saturating_sub(accounted)
                        .saturating_add(length as usize)
                        > TOTAL_BYTES
                {
                    return self.throw_error("RangeError", "host buffer allocation limit");
                }
                let mut b = buffer.borrow_mut();
                let shrink = b.bytes.with_mut(|v| {
                    if (length as usize) < v.len() {
                        true
                    } else {
                        v.resize(length as usize, 0);
                        false
                    }
                });
                if shrink {
                    drop(b);
                    return self.throw_error("RangeError", "SharedArrayBuffer cannot shrink");
                }
                b.account(length as usize);
                Ok(Value::Undefined)
            }
            // §25.2.5.6 slice ( start, end ).
            "slice" => {
                let length = buffer.borrow().bytes.len() as u64;
                let first = self.relative_index(arg(0), length)?;
                let end = if arg(1) == Value::Undefined {
                    length
                } else {
                    self.relative_index(arg(1), length)?
                };
                let count = end.saturating_sub(first);
                // Step 9: SpeciesConstructor(O, %SharedArrayBuffer%).
                let default =
                    Value::Object(self.intrinsics.as_ref().unwrap().functions["SharedArrayBuffer"]);
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
                // Steps 11-14: a distinct SharedArrayBuffer at least newLen bytes long.
                let target = self.buffer_receiver(output.clone(), true)?;
                if Rc::ptr_eq(&target, &buffer) {
                    return self.throw_error("TypeError", "species result is the same buffer");
                }
                if (target.borrow().bytes.len() as u64) < count {
                    return self.throw_error("TypeError", "species result is too small");
                }
                let b = buffer.borrow();
                let count = (count as usize).min(b.bytes.len().saturating_sub(first as usize));
                if count != 0 {
                    let values = b.bytes.get(first as usize, first as usize + count);
                    target.borrow_mut().bytes.set(0, &values);
                }
                Ok(output)
            }
            _ => Err(ScriptError::Unsupported("SharedArrayBuffer operation")),
        }
    }
}
