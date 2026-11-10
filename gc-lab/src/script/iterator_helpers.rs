//! Iterator helpers: the %Iterator% constructor, Iterator.from, Iterator.concat,
//! the %Iterator.prototype% methods, %IteratorHelperPrototype% and
//! %WrapForValidIteratorPrototype%.
//!
//! Spec: ECMA-262 §27.1.2 (%IteratorHelperPrototype%), §27.1.3 (%Iterator%,
//! Iterator.from, Iterator.concat, %WrapForValidIteratorPrototype%) and §27.1.4
//! (%Iterator.prototype%), plus Explicit Resource Management's
//! %Iterator.prototype%[@@dispose].
//!
//! The spec builds each helper with CreateIteratorFromClosure over an abstract
//! closure that yields. This interpreter has no native coroutines, so each
//! closure is written as a resumable state machine (`Helper`): `next` runs the
//! closure up to its next Yield, and `return` on a suspended-yield helper
//! replays the closure's handling of an abrupt return completion at that Yield
//! (IfAbruptCloseIterator, or the inner/outer close pair of flatMap).
use super::{
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;

const DISPOSE: u64 = 13;

/// An Iterator Record (§7.4.1): [[Iterator]] and [[NextMethod]].
#[derive(Clone)]
pub(super) struct Record {
    pub iterator: Value,
    pub next: Value,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum State {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Clone)]
pub(super) enum Kind {
    Map(Value),
    Filter(Value),
    Take(f64),
    Drop(f64),
    FlatMap {
        mapper: Value,
        inner: Option<Record>,
    },
    Concat {
        /// (iterable, open method) pairs of Iterator.concat.
        items: Vec<(Value, Value)>,
        index: usize,
        inner: Option<Record>,
    },
    /// IteratorZip (joint iteration): `iters[i]` is None once removed from
    /// openIters; `keys` is Some for Iterator.zipKeyed.
    Zip {
        iters: Vec<Option<Record>>,
        mode: ZipMode,
        padding: Vec<Value>,
        keys: Option<Vec<Key>>,
    },
    /// Iterator.prototype.chunks; `last` is set after the final partial chunk.
    Chunks {
        size: usize,
        buffer: Vec<Value>,
        last: bool,
    },
    /// Iterator.prototype.windows; `last` is set after the final undersized window.
    Windows {
        size: usize,
        partial: bool,
        buffer: Vec<Value>,
        last: bool,
    },
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ZipMode {
    Shortest,
    Longest,
    Strict,
}

/// An iterator helper object's slots: [[UnderlyingIterators]] (at most one,
/// none for Iterator.concat), [[GeneratorState]] and the closure's captures.
#[derive(Clone)]
pub(super) struct Helper {
    pub underlying: Option<Record>,
    pub state: State,
    pub kind: Kind,
    pub counter: f64,
}

/// The [[Iterated]] slot of a %WrapForValidIteratorPrototype% object, or a helper.
#[derive(Clone)]
pub(super) enum IterSlot {
    Helper(Box<Helper>),
    Wrap(Record),
}

impl Record {
    fn edges(&self) -> impl Iterator<Item = Id> {
        self.iterator.id().into_iter().chain(self.next.id())
    }
}

impl IterSlot {
    pub(super) fn edges(&self) -> Vec<Id> {
        match self {
            IterSlot::Wrap(r) => r.edges().collect(),
            IterSlot::Helper(h) => {
                let mut out: Vec<Id> = h.underlying.iter().flat_map(Record::edges).collect();
                match &h.kind {
                    Kind::Map(f) | Kind::Filter(f) => out.extend(f.id()),
                    Kind::Take(_) | Kind::Drop(_) => {}
                    Kind::FlatMap { mapper, inner } => {
                        out.extend(mapper.id());
                        out.extend(inner.iter().flat_map(Record::edges));
                    }
                    Kind::Concat { items, inner, .. } => {
                        for (a, b) in items {
                            out.extend(a.id());
                            out.extend(b.id());
                        }
                        out.extend(inner.iter().flat_map(Record::edges));
                    }
                    Kind::Zip { iters, padding, .. } => {
                        out.extend(iters.iter().flatten().flat_map(Record::edges));
                        out.extend(padding.iter().filter_map(Value::id));
                    }
                    Kind::Chunks { buffer, .. } | Kind::Windows { buffer, .. } => {
                        out.extend(buffer.iter().filter_map(Value::id));
                    }
                }
                out
            }
        }
    }
}

/// Built-in function names handled here, with their "name" and "length".
const PROTOTYPE_METHODS: &[(&str, usize)] = &[
    ("map", 1),
    ("filter", 1),
    ("take", 1),
    ("drop", 1),
    ("flatMap", 1),
    ("reduce", 1),
    ("toArray", 0),
    ("forEach", 1),
    ("some", 1),
    ("every", 1),
    ("find", 1),
    ("chunks", 1),
    ("windows", 1),
    ("includes", 1),
    ("join", 1),
];

pub(super) fn handles(name: &str) -> bool {
    name == "Iterator"
        || name.starts_with("Iterator.")
        || name.starts_with("%IteratorHelperPrototype%.")
        || name.starts_with("%WrapForValidIteratorPrototype%.")
}

impl Engine<'_> {
    fn iter_named(&mut self, f: Id, name: &str, length: usize) -> Result<()> {
        self.define(
            f,
            "length",
            Descriptor {
                value: Some(Value::number(length as f64)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        self.define(
            f,
            "name",
            Descriptor {
                value: Some(Value::String(name.into())),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    fn iter_function(&mut self, env: Id, full: &str, name: &str, length: usize) -> Result<Id> {
        let function = self.intrinsics.as_ref().unwrap().function;
        let f = self.intrinsic_function(env, full, Some(function))?;
        self.iter_named(f, name, length)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .functions
            .insert(full.into(), f);
        Ok(f)
    }
    /// Creates %Iterator%, its prototype accessors and methods, and the two
    /// helper prototypes. `iterator` is %Iterator.prototype%.
    pub(super) fn bootstrap_iterator_helpers(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
        iterator: Id,
    ) -> Result<()> {
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("Iterator".into(), iterator);
        // §27.1.3.1 %Iterator%; length 0.
        let ctor = self.iter_function(env, "Iterator", "Iterator", 0)?;
        objects.insert("Iterator".into(), ctor);
        // §27.1.3.2.3 Iterator.prototype: { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: false }.
        self.define(
            ctor,
            "prototype",
            Descriptor {
                value: Some(Value::Object(iterator)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..Default::default()
            },
        )?;
        for (method, length) in [("from", 1), ("concat", 0), ("zip", 1), ("zipKeyed", 1)] {
            let f = self.iter_function(env, &format!("Iterator.{method}"), method, length)?;
            self.hidden(ctor, method, Value::Object(f))?;
        }
        for (method, length) in PROTOTYPE_METHODS {
            let f = self.iter_function(
                env,
                &format!("Iterator.prototype.{method}"),
                method,
                *length,
            )?;
            self.hidden(iterator, *method, Value::Object(f))?;
        }
        // §27.1.4.1 constructor and §27.1.4.14 @@toStringTag: accessors whose
        // setters run SetterThatIgnoresPrototypeProperties.
        for (property, key, label) in [
            ("constructor", Key::from("constructor"), "constructor"),
            ("toStringTag", Key::Symbol(1), "[Symbol.toStringTag]"),
        ] {
            let get = self.iter_function(
                env,
                &format!("Iterator.prototype.get.{property}"),
                &format!("get {label}"),
                0,
            )?;
            let set = self.iter_function(
                env,
                &format!("Iterator.prototype.set.{property}"),
                &format!("set {label}"),
                1,
            )?;
            self.define(
                iterator,
                key,
                Descriptor {
                    get: Some(Value::Object(get)),
                    set: Some(Value::Object(set)),
                    enumerable: Some(false),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
        }
        // Explicit Resource Management: %Iterator.prototype% [ @@dispose ] ( ).
        let dispose =
            self.iter_function(env, "Iterator.prototype.dispose", "[Symbol.dispose]", 0)?;
        self.hidden(iterator, Key::Symbol(DISPOSE), Value::Object(dispose))?;
        // §27.1.2.1 %IteratorHelperPrototype%.
        let helper = self.new_object(Some(iterator))?;
        self.runtime.link(env, helper)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("IteratorHelper".into(), helper);
        for method in ["next", "return"] {
            let f = self.iter_function(
                env,
                &format!("%IteratorHelperPrototype%.{method}"),
                method,
                0,
            )?;
            self.hidden(helper, method, Value::Object(f))?;
        }
        self.define(
            helper,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Iterator Helper".into())),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        // §27.1.3.2.1.1 %WrapForValidIteratorPrototype%.
        let wrap = self.new_object(Some(iterator))?;
        self.runtime.link(env, wrap)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("WrapForValidIterator".into(), wrap);
        for method in ["next", "return"] {
            let f = self.iter_function(
                env,
                &format!("%WrapForValidIteratorPrototype%.{method}"),
                method,
                0,
            )?;
            self.hidden(wrap, method, Value::Object(f))?;
        }
        Ok(())
    }

    /// §27.1.3.1 Iterator ( ) as [[Construct]].
    pub(super) fn construct_iterator(&mut self, f: Value, new_target: Value) -> Result<Value> {
        // Step 1: NewTarget undefined or the active function object → TypeError.
        if new_target == f || new_target == Value::Undefined {
            return self.throw_error("TypeError", "Iterator cannot be constructed directly");
        }
        // Step 2: OrdinaryCreateFromConstructor(NewTarget, "%Iterator.prototype%").
        let proto =
            self.prototype_from_constructor(new_target.clone(), |i| i.collection["Iterator"])?;
        self.new_object(Some(proto)).map(Value::Object)
    }

    pub(super) fn iterator_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        for a in &args {
            self.protect(a)?;
        }
        self.protect(&receiver)?;
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            // §27.1.3.1 step 1: a [[Call]] has NewTarget undefined.
            "Iterator" => self.throw_error("TypeError", "Constructor Iterator requires 'new'"),
            "Iterator.from" => self.iterator_from(arg(0)),
            "Iterator.concat" => self.iterator_concat(args),
            "Iterator.zip" => self.iterator_zip(arg(0), arg(1), false),
            "Iterator.zipKeyed" => self.iterator_zip(arg(0), arg(1), true),
            "Iterator.prototype.get.constructor" => Ok(Value::Object(
                self.intrinsics.as_ref().unwrap().functions["Iterator"],
            )),
            "Iterator.prototype.get.toStringTag" => Ok(Value::String("Iterator".into())),
            "Iterator.prototype.set.constructor" => {
                self.setter_ignoring_prototype(receiver, Key::from("constructor"), arg(0))
            }
            "Iterator.prototype.set.toStringTag" => {
                self.setter_ignoring_prototype(receiver, Key::Symbol(1), arg(0))
            }
            "Iterator.prototype.dispose" => {
                // Steps 1-3: GetMethod(O, "return"); if present, Call it.
                let method = self.get_member(receiver.clone(), "return")?;
                self.protect(&method)?;
                if !matches!(method, Value::Undefined | Value::Null) {
                    if !self.callable(&method) {
                        return self.throw_error("TypeError", "return is not a function");
                    }
                    self.invoke_this(method, receiver, vec![])?;
                }
                Ok(Value::Undefined)
            }
            "%IteratorHelperPrototype%.next" => self.helper_next(receiver),
            "%IteratorHelperPrototype%.return" => self.helper_return(receiver),
            "%WrapForValidIteratorPrototype%.next" => {
                let record = self.wrapped(&receiver)?;
                self.invoke_this(record.next, record.iterator, vec![])
            }
            "%WrapForValidIteratorPrototype%.return" => {
                let record = self.wrapped(&receiver)?;
                let method = self.get_member(record.iterator.clone(), "return")?;
                self.protect(&method)?;
                if matches!(method, Value::Undefined | Value::Null) {
                    return self.iterator_result(Value::Undefined, true);
                }
                if !self.callable(&method) {
                    return self.throw_error("TypeError", "return is not a function");
                }
                self.invoke_this(method, record.iterator, vec![])
            }
            _ => {
                let Some(method) = name.strip_prefix("Iterator.prototype.") else {
                    return Err(ScriptError::Unsupported("Iterator method"));
                };
                self.iterator_method(method, receiver, &args)
            }
        }
    }

    /// SetterThatIgnoresPrototypeProperties (§27.1.4.1.2 / §27.1.4.14.2 helper).
    fn setter_ignoring_prototype(&mut self, this: Value, p: Key, v: Value) -> Result<Value> {
        let Value::Object(id) = this else {
            return self.throw_error("TypeError", "setter called on non-object");
        };
        let home = self.intrinsics.as_ref().unwrap().collection["Iterator"];
        if id == home {
            return self.throw_error("TypeError", "cannot set property on Iterator.prototype");
        }
        if self.own(id, p.clone())?.is_none() {
            let ok = self.define(
                id,
                p,
                Descriptor {
                    value: Some(v),
                    writable: Some(true),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
            if !ok {
                return self.throw_error("TypeError", "cannot define property");
            }
        } else {
            self.set_member(Value::Object(id), p, v)?;
        }
        Ok(Value::Undefined)
    }

    fn wrapped(&mut self, receiver: &Value) -> Result<Record> {
        if let Value::Object(id) = receiver {
            if let Some(IterSlot::Wrap(r)) = self.data(*id).ok().and_then(|d| d.iter_slot.clone()) {
                return Ok(r);
            }
        }
        self.throw_error("TypeError", "not a wrapped iterator")
    }

    /// GetIteratorDirect (§7.4.2).
    fn iterator_direct(&mut self, obj: Value) -> Result<Record> {
        let next = self.get_member(obj.clone(), "next")?;
        self.protect(&next)?;
        Ok(Record {
            iterator: obj,
            next,
        })
    }

    /// GetIteratorFlattenable (§7.4.4); `strings` selects iterate-string-primitives.
    fn iterator_flattenable(&mut self, obj: Value, strings: bool) -> Result<Record> {
        if !matches!(obj, Value::Object(_)) && !(strings && matches!(obj, Value::String(_))) {
            return self.throw_error("TypeError", "value is not an object");
        }
        let method = self.get_member(obj.clone(), Key::Symbol(0))?;
        self.protect(&method)?;
        let iterator = if matches!(method, Value::Undefined | Value::Null) {
            obj
        } else {
            if !self.callable(&method) {
                return self.throw_error("TypeError", "Symbol.iterator is not a function");
            }
            self.invoke_this(method, obj, vec![])?
        };
        self.protect(&iterator)?;
        if !matches!(iterator, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator is not an object");
        }
        self.iterator_direct(iterator)
    }

    /// IteratorStepValue (§7.4.10): `None` when done.
    fn step_value(&mut self, record: &Record) -> Result<Option<Value>> {
        if !self.callable(&record.next) {
            return self.throw_error("TypeError", "next is not a function");
        }
        let result = self.invoke_this(record.next.clone(), record.iterator.clone(), vec![])?;
        self.protect(&result)?;
        if !matches!(result, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator result is not an object");
        }
        if self.get_member(result.clone(), "done")?.truthy() {
            return Ok(None);
        }
        let value = self.get_member(result, "value")?;
        self.protect(&value)?;
        Ok(Some(value))
    }

    /// IteratorClose with a throw completion: closes, then rethrows `error`.
    fn close_with<T>(&mut self, iterator: Value, error: ScriptError) -> Result<T> {
        match self.close_iterator(iterator, Err(error)) {
            Err(e) => Err(e),
            Ok(_) => unreachable!("close_iterator returns a pending throw"),
        }
    }

    /// IfAbruptCloseIterator (§7.4.13).
    fn or_close<T>(&mut self, r: Result<T>, iterator: &Value) -> Result<T> {
        match r {
            Err(e @ ScriptError::Thrown(_)) => self.close_with(iterator.clone(), e),
            other => other,
        }
    }

    /// §27.1.3.2.2 Iterator.from ( O ).
    fn iterator_from(&mut self, o: Value) -> Result<Value> {
        let record = self.iterator_flattenable(o, true)?;
        let ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Iterator"]);
        if self.ordinary_has_instance(record.iterator.clone(), ctor)? {
            return Ok(record.iterator);
        }
        let proto = self.intrinsics.as_ref().unwrap().collection["WrapForValidIterator"];
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.iter_slot = Some(IterSlot::Wrap(record));
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }

    /// Iterator.concat ( ...items ) (ECMA-262 2026 §27.1.3.2.1).
    fn iterator_concat(&mut self, items: Vec<Value>) -> Result<Value> {
        let mut iterables = vec![];
        for item in items {
            if !matches!(item, Value::Object(_)) {
                return self.throw_error("TypeError", "Iterator.concat item is not an object");
            }
            let method = self.get_member(item.clone(), Key::Symbol(0))?;
            self.protect(&method)?;
            if matches!(method, Value::Undefined | Value::Null) {
                return self.throw_error("TypeError", "Iterator.concat item is not iterable");
            }
            if !self.callable(&method) {
                return self.throw_error("TypeError", "Symbol.iterator is not a function");
            }
            iterables.push((item, method));
        }
        self.new_helper(
            None,
            Kind::Concat {
                items: iterables,
                index: 0,
                inner: None,
            },
        )
    }

    fn new_helper(&mut self, underlying: Option<Record>, kind: Kind) -> Result<Value> {
        let proto = self.intrinsics.as_ref().unwrap().collection["IteratorHelper"];
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.iter_slot = Some(IterSlot::Helper(Box::new(Helper {
            underlying,
            state: State::SuspendedStart,
            kind,
            counter: 0.0,
        })));
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }

    fn helper_of(&mut self, receiver: &Value) -> Result<(Id, Helper)> {
        if let Value::Object(id) = receiver {
            if let Some(IterSlot::Helper(h)) = self.data(*id).ok().and_then(|d| d.iter_slot.clone())
            {
                return Ok((*id, *h));
            }
        }
        self.throw_error("TypeError", "not an Iterator Helper")
    }

    fn store_helper(&mut self, id: Id, helper: Helper) -> Result<()> {
        let mut data = self.data(id)?.clone();
        data.iter_slot = Some(IterSlot::Helper(Box::new(helper)));
        self.replace_data(id, data)
    }

    fn set_state(&mut self, id: Id, state: State) -> Result<()> {
        let mut data = self.data(id)?.clone();
        if let Some(IterSlot::Helper(h)) = data.iter_slot.as_mut() {
            h.state = state;
        }
        self.replace_data(id, data)
    }

    /// §27.1.2.1.1 %IteratorHelperPrototype%.next ( ): GeneratorResume.
    fn helper_next(&mut self, receiver: Value) -> Result<Value> {
        let (id, mut helper) = self.helper_of(&receiver)?;
        match helper.state {
            State::Executing => {
                return self.throw_error("TypeError", "Iterator Helper is already running")
            }
            State::Completed => return self.iterator_result(Value::Undefined, true),
            _ => {}
        }
        helper.state = State::Executing;
        self.store_helper(id, helper.clone())?;
        let r = self.helper_step(id, &mut helper);
        match r {
            Ok(Some(value)) => {
                helper.state = State::SuspendedYield;
                self.store_helper(id, helper)?;
                self.iterator_result(value, false)
            }
            Ok(None) => {
                helper.state = State::Completed;
                self.store_helper(id, helper)?;
                self.iterator_result(Value::Undefined, true)
            }
            Err(e) => {
                helper.state = State::Completed;
                self.store_helper(id, helper)?;
                Err(e)
            }
        }
    }

    /// Runs the helper's closure until its next Yield (`Some`) or its return (`None`).
    /// Intermediate captures are stored back so the object stays a GC root for them.
    fn helper_step(&mut self, id: Id, h: &mut Helper) -> Result<Option<Value>> {
        loop {
            self.tick()?;
            match &mut h.kind {
                Kind::Zip { .. } => return self.zip_step(h),
                Kind::Chunks { size, last, .. } => {
                    if *last {
                        return Ok(None);
                    }
                    let size = *size;
                    let underlying = h.underlying.clone().unwrap();
                    loop {
                        self.tick()?;
                        let Kind::Chunks { buffer, last, .. } = &mut h.kind else {
                            unreachable!()
                        };
                        match self.step_value(&underlying)? {
                            None => {
                                if buffer.is_empty() {
                                    return Ok(None);
                                }
                                *last = true;
                                let chunk = std::mem::take(buffer);
                                return self.iterator_array(chunk).map(Some);
                            }
                            Some(v) => {
                                buffer.push(v);
                                if buffer.len() == size {
                                    let chunk = std::mem::take(buffer);
                                    return self.iterator_array(chunk).map(Some);
                                }
                            }
                        }
                    }
                }
                Kind::Windows {
                    size,
                    partial,
                    last,
                    ..
                } => {
                    if *last {
                        return Ok(None);
                    }
                    let (size, partial) = (*size, *partial);
                    let underlying = h.underlying.clone().unwrap();
                    loop {
                        self.tick()?;
                        let Kind::Windows { buffer, last, .. } = &mut h.kind else {
                            unreachable!()
                        };
                        match self.step_value(&underlying)? {
                            None => {
                                if partial && !buffer.is_empty() && buffer.len() < size {
                                    *last = true;
                                    let window = buffer.clone();
                                    return self.iterator_array(window).map(Some);
                                }
                                return Ok(None);
                            }
                            Some(v) => {
                                if buffer.len() == size {
                                    buffer.remove(0);
                                }
                                buffer.push(v);
                                if buffer.len() == size {
                                    let window = buffer.clone();
                                    return self.iterator_array(window).map(Some);
                                }
                            }
                        }
                    }
                }
                Kind::Concat {
                    items,
                    index,
                    inner,
                } => {
                    if inner.is_none() {
                        let Some((iterable, method)) = items.get(*index).cloned() else {
                            return Ok(None);
                        };
                        *index += 1;
                        let iter = self.invoke_this(method, iterable, vec![])?;
                        self.protect(&iter)?;
                        if !matches!(iter, Value::Object(_)) {
                            return self.throw_error("TypeError", "iterator is not an object");
                        }
                        let record = self.iterator_direct(iter)?;
                        *inner = Some(record);
                        self.store_helper(id, h.clone())?;
                        continue;
                    }
                    let record = inner.clone().unwrap();
                    match self.step_value(&record) {
                        Ok(Some(v)) => return Ok(Some(v)),
                        Ok(None) => {
                            if let Kind::Concat { inner, .. } = &mut h.kind {
                                *inner = None;
                            }
                            self.store_helper(id, h.clone())?;
                        }
                        Err(e) => return Err(e),
                    }
                }
                Kind::FlatMap { mapper, inner } => {
                    let underlying = h.underlying.clone().unwrap();
                    if let Some(record) = inner.clone() {
                        let r = self.step_value(&record);
                        match self.or_close(r, &underlying.iterator)? {
                            Some(v) => return Ok(Some(v)),
                            None => {
                                *inner = None;
                                h.counter += 1.0;
                                self.store_helper(id, h.clone())?;
                                continue;
                            }
                        }
                    }
                    let mapper = mapper.clone();
                    let Some(value) = self.step_value(&underlying)? else {
                        return Ok(None);
                    };
                    let r = self.invoke(mapper, vec![value, Value::number(h.counter)]);
                    let mapped = self.or_close(r, &underlying.iterator)?;
                    self.protect(&mapped)?;
                    let r = self.iterator_flattenable(mapped, false);
                    let record = self.or_close(r, &underlying.iterator)?;
                    if let Kind::FlatMap { inner, .. } = &mut h.kind {
                        *inner = Some(record);
                    }
                    self.store_helper(id, h.clone())?;
                }
                Kind::Map(mapper) => {
                    let underlying = h.underlying.clone().unwrap();
                    let mapper = mapper.clone();
                    let Some(value) = self.step_value(&underlying)? else {
                        return Ok(None);
                    };
                    let r = self.invoke(mapper, vec![value, Value::number(h.counter)]);
                    let mapped = self.or_close(r, &underlying.iterator)?;
                    h.counter += 1.0;
                    return Ok(Some(mapped));
                }
                Kind::Filter(predicate) => {
                    let underlying = h.underlying.clone().unwrap();
                    let predicate = predicate.clone();
                    let Some(value) = self.step_value(&underlying)? else {
                        return Ok(None);
                    };
                    let r = self.invoke(predicate, vec![value.clone(), Value::number(h.counter)]);
                    let selected = self.or_close(r, &underlying.iterator)?;
                    h.counter += 1.0;
                    if selected.truthy() {
                        return Ok(Some(value));
                    }
                }
                Kind::Take(remaining) => {
                    let underlying = h.underlying.clone().unwrap();
                    if *remaining == 0.0 {
                        // Step 8.a.i: Return ? IteratorClose(iterated, ReturnCompletion(undefined)).
                        self.close_iterator(underlying.iterator, Ok(None))?;
                        return Ok(None);
                    }
                    if *remaining != f64::INFINITY {
                        *remaining -= 1.0;
                    }
                    return self.step_value(&underlying);
                }
                Kind::Drop(remaining) => {
                    let underlying = h.underlying.clone().unwrap();
                    while *remaining > 0.0 {
                        self.tick()?;
                        if *remaining != f64::INFINITY {
                            *remaining -= 1.0;
                        }
                        if self.step_done(&underlying)? {
                            return Ok(None);
                        }
                    }
                    return self.step_value(&underlying);
                }
            }
        }
    }

    /// §27.1.2.1.2 %IteratorHelperPrototype%.return ( ).
    fn helper_return(&mut self, receiver: Value) -> Result<Value> {
        let (id, helper) = self.helper_of(&receiver)?;
        match helper.state {
            State::Executing => self.throw_error("TypeError", "Iterator Helper is already running"),
            State::Completed => self.iterator_result(Value::Undefined, true),
            State::SuspendedStart => {
                // Step 4: completed, then IteratorCloseAll(O.[[UnderlyingIterators]], return).
                self.set_state(id, State::Completed)?;
                let underlying = match helper.kind {
                    Kind::Zip { iters, .. } => iters.into_iter().flatten().collect(),
                    _ => helper.underlying.into_iter().collect(),
                };
                self.close_all(underlying, Ok(None))?;
                self.iterator_result(Value::Undefined, true)
            }
            State::SuspendedYield => {
                // GeneratorResumeAbrupt with a return completion at the closure's Yield.
                self.set_state(id, State::Executing)?;
                let r = self.helper_abrupt_return(&helper);
                self.set_state(id, State::Completed)?;
                r?;
                self.iterator_result(Value::Undefined, true)
            }
        }
    }

    fn helper_abrupt_return(&mut self, h: &Helper) -> Result<()> {
        match &h.kind {
            Kind::Concat { inner, .. } => {
                // Return ? IteratorClose(iteratorRecord, completion).
                if let Some(r) = inner {
                    self.close_iterator(r.iterator.clone(), Ok(None))?;
                }
                Ok(())
            }
            Kind::FlatMap { inner, .. } => {
                let outer = h.underlying.clone().unwrap().iterator;
                if let Some(r) = inner {
                    // backupCompletion = IteratorClose(inner, completion); IfAbruptCloseIterator.
                    let backup = self.close_iterator(r.iterator.clone(), Ok(None));
                    self.or_close(backup, &outer)?;
                }
                self.close_iterator(outer, Ok(None))?;
                Ok(())
            }
            Kind::Zip { iters, .. } => {
                // IteratorCloseAll(openIters, completion).
                let open = iters.iter().flatten().cloned().collect();
                self.close_all(open, Ok(None))?;
                Ok(())
            }
            // The final partial chunk / window is yielded with `? Yield`: no close.
            Kind::Chunks { last: true, .. } | Kind::Windows { last: true, .. } => Ok(()),
            _ => {
                let outer = h.underlying.clone().unwrap().iterator;
                self.close_iterator(outer, Ok(None))?;
                Ok(())
            }
        }
    }

    /// §27.1.4.2-13: the %Iterator.prototype% methods.
    fn iterator_method(&mut self, method: &str, o: Value, args: &[Value]) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        // Step 1-2: O must be an Object.
        if !matches!(o, Value::Object(_)) {
            return self.throw_error(
                "TypeError",
                &format!("Iterator.prototype.{method} called on non-object"),
            );
        }
        match method {
            "chunks" | "windows" => return self.chunks_or_windows(method, o, args),
            "includes" => return self.iterator_includes(o, arg(0), arg(1)),
            "join" => return self.iterator_join(o, arg(0)),
            _ => {}
        }
        if method == "toArray" {
            let record = self.iterator_direct(o)?;
            let mut items = vec![];
            while let Some(v) = self.step_value(&record)? {
                self.tick()?;
                items.push(v);
            }
            return self.iterator_array(items);
        }
        // Steps 3-4: validate the argument, closing O (iterated.[[NextMethod]] undefined) on failure.
        let (kind, f) = match method {
            "take" | "drop" => {
                let limit = self.to_number(arg(0));
                let limit = self.or_close(limit, &o)?;
                // Steps 6-7: NaN, or a finite limit above 2**53 - 1, is a RangeError.
                if limit.is_nan() || (limit.is_finite() && limit > 9007199254740991.0) {
                    let e = self
                        .throw_error::<()>(
                            "RangeError",
                            "limit must be an integer no greater than 2**53 - 1",
                        )
                        .unwrap_err();
                    return self.close_with(o, e);
                }
                let limit = if limit.is_infinite() {
                    limit
                } else {
                    limit.trunc() + 0.0
                };
                if limit < 0.0 {
                    let e = self
                        .throw_error::<()>("RangeError", "limit must be non-negative")
                        .unwrap_err();
                    return self.close_with(o, e);
                }
                (
                    Some(if method == "take" {
                        Kind::Take(limit)
                    } else {
                        Kind::Drop(limit)
                    }),
                    Value::Undefined,
                )
            }
            _ => {
                let f = arg(0);
                if !self.callable(&f) {
                    let e = self
                        .throw_error::<()>(
                            "TypeError",
                            &format!("{} is not a function", "callback"),
                        )
                        .unwrap_err();
                    return self.close_with(o, e);
                }
                let kind = match method {
                    "map" => Some(Kind::Map(f.clone())),
                    "filter" => Some(Kind::Filter(f.clone())),
                    "flatMap" => Some(Kind::FlatMap {
                        mapper: f.clone(),
                        inner: None,
                    }),
                    _ => None,
                };
                (kind, f)
            }
        };
        // Step 5: iterated = ? GetIteratorDirect(O).
        let record = self.iterator_direct(o)?;
        if let Some(kind) = kind {
            return self.new_helper(Some(record), kind);
        }
        let iterator = record.iterator.clone();
        let mut counter = 0.0;
        match method {
            "reduce" => {
                let mut accumulator = if args.len() < 2 {
                    let Some(first) = self.step_value(&record)? else {
                        return self.throw_error(
                            "TypeError",
                            "Reduce of empty iterator with no initial value",
                        );
                    };
                    counter = 1.0;
                    first
                } else {
                    arg(1)
                };
                loop {
                    self.tick()?;
                    self.protect(&accumulator)?;
                    let Some(value) = self.step_value(&record)? else {
                        return Ok(accumulator);
                    };
                    let r =
                        self.invoke(f.clone(), vec![accumulator, value, Value::number(counter)]);
                    accumulator = self.or_close(r, &iterator)?;
                    counter += 1.0;
                }
            }
            "forEach" | "some" | "every" | "find" => loop {
                self.tick()?;
                let Some(value) = self.step_value(&record)? else {
                    return Ok(match method {
                        "some" => Value::Bool(false),
                        "every" => Value::Bool(true),
                        _ => Value::Undefined,
                    });
                };
                let r = self.invoke(f.clone(), vec![value.clone(), Value::number(counter)]);
                let result = self.or_close(r, &iterator)?;
                let hit = match method {
                    "some" if result.truthy() => Some(Value::Bool(true)),
                    "every" if !result.truthy() => Some(Value::Bool(false)),
                    "find" if result.truthy() => Some(value),
                    _ => None,
                };
                if let Some(out) = hit {
                    // Return ? IteratorClose(iterated, NormalCompletion(out)).
                    self.close_iterator(iterator, Ok(Some(out.clone())))?;
                    return Ok(out);
                }
                counter += 1.0;
            },
            _ => Err(ScriptError::Unsupported("Iterator method")),
        }
    }

    fn iterator_array(&mut self, items: Vec<Value>) -> Result<Value> {
        let array = self.new_array(0)?;
        self.protect(&Value::Object(array))?;
        for (i, v) in items.into_iter().enumerate() {
            self.define(
                array,
                i.to_string(),
                Descriptor {
                    value: Some(v),
                    writable: Some(true),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
        }
        Ok(Value::Object(array))
    }

    fn type_error(&mut self, name: &str, message: &str) -> ScriptError {
        self.throw_error::<()>(name, message).unwrap_err()
    }

    /// IteratorStep (§7.4.9) reduced to its `done` answer.
    fn step_done(&mut self, record: &Record) -> Result<bool> {
        if !self.callable(&record.next) {
            return self.throw_error("TypeError", "next is not a function");
        }
        let result = self.invoke_this(record.next.clone(), record.iterator.clone(), vec![])?;
        self.protect(&result)?;
        if !matches!(result, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator result is not an object");
        }
        Ok(self.get_member(result, "done")?.truthy())
    }

    /// GetIterator(obj, sync) (§7.4.3).
    fn get_iterator(&mut self, obj: Value) -> Result<Record> {
        let method = self.get_member(obj.clone(), Key::Symbol(0))?;
        self.protect(&method)?;
        if !self.callable(&method) {
            return self.throw_error("TypeError", "object is not iterable");
        }
        let (iterator, next) = self.open_iterator(obj, method)?;
        Ok(Record { iterator, next })
    }

    /// IteratorCloseAll (joint iteration): closes in reverse list order,
    /// threading the completion.
    fn close_all(
        &mut self,
        records: Vec<Record>,
        mut pending: Result<Option<Value>>,
    ) -> Result<Option<Value>> {
        for record in records.into_iter().rev() {
            pending = self.close_iterator(record.iterator, pending);
        }
        pending
    }

    /// IfAbruptCloseIterators: on a throw completion, IteratorCloseAll and rethrow.
    fn or_close_all<T>(&mut self, r: Result<T>, records: &[Record]) -> Result<T> {
        match r {
            Err(e @ ScriptError::Thrown(_)) => match self.close_all(records.to_vec(), Err(e)) {
                Err(e) => Err(e),
                Ok(_) => unreachable!("close_all returns a pending throw"),
            },
            other => other,
        }
    }

    /// Iterator.zip ( iterables [ , options ] ) and Iterator.zipKeyed
    /// ( iterables [ , options ] ) (ECMA-262 2026, Joint Iteration).
    fn iterator_zip(&mut self, iterables: Value, options: Value, keyed: bool) -> Result<Value> {
        // Step 1: iterables must be an Object.
        if !matches!(iterables, Value::Object(_)) {
            return self.throw_error("TypeError", "iterables is not an object");
        }
        // Step 2: GetOptionsObject(options).
        let options = match options {
            Value::Undefined => Value::Object(self.new_object(None)?),
            Value::Object(_) => options,
            _ => return self.throw_error("TypeError", "options is not an object"),
        };
        self.protect(&options)?;
        // Steps 3-5: mode.
        let mode = self.get_member(options.clone(), "mode")?;
        let mode = match &mode {
            Value::Undefined => ZipMode::Shortest,
            Value::String(s) if s == "shortest" => ZipMode::Shortest,
            Value::String(s) if s == "longest" => ZipMode::Longest,
            Value::String(s) if s == "strict" => ZipMode::Strict,
            _ => {
                return self.throw_error(
                    "TypeError",
                    "mode must be \"shortest\", \"longest\" or \"strict\"",
                )
            }
        };
        // Steps 6-7: paddingOption.
        let mut padding_option = Value::Undefined;
        if mode == ZipMode::Longest {
            padding_option = self.get_member(options, "padding")?;
            self.protect(&padding_option)?;
            if !matches!(padding_option, Value::Undefined | Value::Object(_)) {
                return self.throw_error("TypeError", "padding is not an object");
            }
        }
        let mut iters: Vec<Record> = vec![];
        let mut keys: Vec<Key> = vec![];
        if keyed {
            // zipKeyed steps 9-10: enumerable own keys whose value is not undefined.
            let Value::Object(id) = iterables else {
                unreachable!()
            };
            let all = self.own_keys(id)?;
            for key in all {
                self.tick()?;
                let desc = self.own(id, key.clone());
                let desc = self.or_close_all(desc, &iters)?;
                if !desc.is_some_and(|d| d.enumerable) {
                    continue;
                }
                let value = self.get_member(iterables.clone(), key.clone());
                let value = self.or_close_all(value, &iters)?;
                self.protect(&value)?;
                if value == Value::Undefined {
                    continue;
                }
                let iter = self.iterator_flattenable(value, false);
                let iter = self.or_close_all(iter, &iters)?;
                keys.push(key);
                iters.push(iter);
            }
        } else {
            // zip steps 9-11: iterate iterables, GetIteratorFlattenable each value.
            let input = self.get_iterator(iterables)?;
            loop {
                self.tick()?;
                let next = self.step_value(&input);
                let Some(next) = self.or_close_all(next, &iters)? else {
                    break;
                };
                let iter = self.iterator_flattenable(next, false);
                let mut with_input = vec![input.clone()];
                with_input.extend(iters.iter().cloned());
                let iter = self.or_close_all(iter, &with_input)?;
                iters.push(iter);
            }
        }
        // Padding (zip step 14 / zipKeyed step 12).
        let n = iters.len();
        let mut padding = vec![];
        if mode == ZipMode::Longest {
            if padding_option == Value::Undefined {
                padding = vec![Value::Undefined; n];
            } else if keyed {
                for key in &keys {
                    let value = self.get_member(padding_option.clone(), key.clone());
                    let value = self.or_close_all(value, &iters)?;
                    self.protect(&value)?;
                    padding.push(value);
                }
            } else {
                let padding_iter = self.get_iterator(padding_option);
                let padding_iter = self.or_close_all(padding_iter, &iters)?;
                let mut using = true;
                for _ in 0..n {
                    self.tick()?;
                    if using {
                        let next = self.step_value(&padding_iter);
                        match self.or_close_all(next, &iters)? {
                            Some(v) => padding.push(v),
                            None => using = false,
                        }
                    }
                    if !using {
                        padding.push(Value::Undefined);
                    }
                }
                if using {
                    let r = self.close_iterator(padding_iter.iterator, Ok(None));
                    self.or_close_all(r, &iters)?;
                }
            }
        }
        self.new_helper(
            None,
            Kind::Zip {
                iters: iters.into_iter().map(Some).collect(),
                mode,
                padding,
                keys: keyed.then_some(keys),
            },
        )
    }

    /// The IteratorZip closure, up to its next Yield.
    fn zip_step(&mut self, h: &mut Helper) -> Result<Option<Value>> {
        let Kind::Zip {
            iters,
            mode,
            padding,
            keys,
        } = &mut h.kind
        else {
            unreachable!()
        };
        let n = iters.len();
        if n == 0 {
            return Ok(None);
        }
        let mode = *mode;
        let open = |iters: &Vec<Option<Record>>| -> Vec<Record> {
            iters.iter().flatten().cloned().collect()
        };
        let mut results = Vec::with_capacity(n);
        for i in 0..n {
            let result = match iters[i].clone() {
                None => padding[i].clone(),
                Some(record) => match self.step_value(&record) {
                    Err(e) => {
                        iters[i] = None;
                        let all = open(iters);
                        return self.close_all(all, Err(e)).map(|_| None);
                    }
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        iters[i] = None;
                        match mode {
                            ZipMode::Shortest => {
                                let all = open(iters);
                                self.close_all(all, Ok(None))?;
                                return Ok(None);
                            }
                            ZipMode::Strict => {
                                if i != 0 {
                                    let e = self.type_error(
                                        "TypeError",
                                        "iterators passed to Iterator.zip have different lengths",
                                    );
                                    let all = open(iters);
                                    return self.close_all(all, Err(e)).map(|_| None);
                                }
                                for k in 1..n {
                                    let Some(other) = iters[k].clone() else {
                                        continue;
                                    };
                                    match self.step_done(&other) {
                                        Err(e) => {
                                            iters[k] = None;
                                            let all = open(iters);
                                            return self.close_all(all, Err(e)).map(|_| None);
                                        }
                                        Ok(true) => iters[k] = None,
                                        Ok(false) => {
                                            let e = self.type_error(
                                                "TypeError",
                                                "iterators passed to Iterator.zip have different lengths",
                                            );
                                            let all = open(iters);
                                            return self.close_all(all, Err(e)).map(|_| None);
                                        }
                                    }
                                }
                                return Ok(None);
                            }
                            ZipMode::Longest => {
                                if iters.iter().all(Option::is_none) {
                                    return Ok(None);
                                }
                                padding[i].clone()
                            }
                        }
                    }
                },
            };
            self.protect(&result)?;
            results.push(result);
        }
        // finishResults.
        match keys.clone() {
            None => self.iterator_array(results).map(Some),
            Some(keys) => {
                let obj = self.new_object(None)?;
                self.protect(&Value::Object(obj))?;
                for (key, value) in keys.into_iter().zip(results) {
                    self.define(
                        obj,
                        key,
                        Descriptor {
                            value: Some(value),
                            writable: Some(true),
                            enumerable: Some(true),
                            configurable: Some(true),
                            ..Default::default()
                        },
                    )?;
                }
                Ok(Some(Value::Object(obj)))
            }
        }
    }

    /// Validates a chunks / windows size: an integral Number in [1, 2**32 - 1].
    fn size_argument(&mut self, size: &Value, o: &Value) -> Result<usize> {
        let n = match size {
            Value::Number(b) => f64::from_bits(*b),
            _ => f64::NAN,
        };
        if !(n.is_finite() && n.trunc() == n) {
            let e = self.type_error("TypeError", "size must be an integral Number");
            return self.close_with(o.clone(), e);
        }
        if !(1.0..=4294967295.0).contains(&n) {
            let e = self.type_error("RangeError", "size must be between 1 and 2**32 - 1");
            return self.close_with(o.clone(), e);
        }
        Ok(n as usize)
    }

    /// Iterator.prototype.chunks ( chunkSize ) and Iterator.prototype.windows
    /// ( windowSize [ , undersized ] ) (ECMA-262 2026, Iterator Chunking).
    fn chunks_or_windows(&mut self, method: &str, o: Value, args: &[Value]) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let size = self.size_argument(&arg(0), &o)?;
        let kind = if method == "chunks" {
            Kind::Chunks {
                size,
                buffer: vec![],
                last: false,
            }
        } else {
            let partial = match arg(1) {
                Value::Undefined => false,
                Value::String(s) if s == "only-full" => false,
                Value::String(s) if s == "allow-partial" => true,
                _ => {
                    let e = self.type_error(
                        "TypeError",
                        "undersized must be \"only-full\" or \"allow-partial\"",
                    );
                    return self.close_with(o, e);
                }
            };
            Kind::Windows {
                size,
                partial,
                buffer: vec![],
                last: false,
            }
        };
        let record = self.iterator_direct(o)?;
        self.new_helper(Some(record), kind)
    }

    /// Iterator.prototype.includes ( searchElement [ , skippedElements ] ).
    fn iterator_includes(&mut self, o: Value, search: Value, skipped: Value) -> Result<Value> {
        let to_skip = match skipped {
            Value::Undefined => 0.0,
            Value::Number(b)
                if {
                    let n = f64::from_bits(b);
                    n.is_infinite() || n.trunc() == n
                } =>
            {
                f64::from_bits(b)
            }
            _ => {
                let e = self.type_error("TypeError", "skippedElements must be an integral Number");
                return self.close_with(o, e);
            }
        };
        if to_skip < 0.0 {
            let e = self.type_error("RangeError", "skippedElements must be non-negative");
            return self.close_with(o, e);
        }
        if to_skip.is_finite() && to_skip > 9007199254740991.0 {
            let e = self.type_error("RangeError", "skippedElements must be at most 2**53 - 1");
            return self.close_with(o, e);
        }
        let record = self.iterator_direct(o)?;
        let mut count = 0.0;
        loop {
            self.tick()?;
            let Some(value) = self.step_value(&record)? else {
                return Ok(Value::Bool(false));
            };
            if count < to_skip {
                count += 1.0;
            } else if value.same_value_zero(&search) {
                self.close_iterator(record.iterator, Ok(Some(Value::Bool(true))))?;
                return Ok(Value::Bool(true));
            }
        }
    }

    /// Iterator.prototype.join ( separator ) (Iterator Join proposal).
    fn iterator_join(&mut self, o: Value, separator: Value) -> Result<Value> {
        let sep = if separator == Value::Undefined {
            ",".to_string()
        } else {
            let r = self.to_string(separator);
            self.or_close(r, &o)?
        };
        let record = self.iterator_direct(o)?;
        let iterator = record.iterator.clone();
        let mut out = String::new();
        let mut first = true;
        loop {
            self.tick()?;
            let Some(value) = self.step_value(&record)? else {
                return Ok(Value::String(out.into()));
            };
            if !first {
                out.push_str(&sep);
            }
            first = false;
            if !matches!(value, Value::Undefined | Value::Null) {
                let r = self.to_string(value);
                out.push_str(&self.or_close(r, &iterator)?);
            }
        }
    }
}
