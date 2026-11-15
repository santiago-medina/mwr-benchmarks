//! The embedding API (SERVO_BINDINGS.md E1, E2, E4, E5): what a binding layer such as
//! the mozjs-compatible facade needs from the engine.
//!
//! - E1 host objects: an ordinary object with a class word, a payload word and reserved
//!   slots. The slots are graph edges; anything else the embedder holds is reported by
//!   `Host::trace` (a JSClass trace hook) and released by `Host::finalize`.
//! - E2 host functions: built-in function objects whose [[Call]] / [[Construct]]
//!   (ECMA-262 §10.3.1, §10.3.2) dispatch to `Host::call` with a function word.
//! - E4 embedder roots: `Host::trace_roots` (mozjs `JS_AddExtraGCRootsTracer`) and
//!   explicit `root` / `unroot` (the facade's `rooted!` slots).
//! - E5 values and objects: `JsValue`, `ObjectId` and the property / call operations.
//!
//! Ids returned here are not rooted: a GC can run at any allocation, so the embedder
//! roots what it keeps before calling back into the engine.
use super::objects::{Descriptor, Key};
use super::vm::{Engine, ScriptError, Value};
use crate::dual::{ExternalRoot, HostTag, HostTracer, Id};
use std::rc::Rc;

type Result<T> = std::result::Result<T, ScriptError>;

/// `Object::Builtin` names of host functions; the constructor name makes them
/// constructible (`Engine::constructible`).
pub(super) const HOST_FUNCTION: &str = "%HostFunction%";
pub(super) const HOST_CONSTRUCTOR: &str = "%HostConstructor%";

/// An object in this engine's heap. Identity is stable while the object lives (the
/// collector does not move objects).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId(pub(super) Id);

/// A JavaScript value as seen by the embedder.
#[derive(Clone, Debug, PartialEq)]
pub struct JsValue(pub(super) Value);

/// A property key: a string or a symbol (by its engine-wide number).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    String(String),
    Symbol(u64),
}

impl From<&str> for PropertyKey {
    fn from(v: &str) -> Self {
        Self::String(v.into())
    }
}
impl From<PropertyKey> for Key {
    fn from(v: PropertyKey) -> Self {
        match v {
            PropertyKey::String(s) => Key::String(s),
            PropertyKey::Symbol(s) => Key::Symbol(s),
        }
    }
}

impl JsValue {
    pub fn undefined() -> Self {
        Self(Value::Undefined)
    }
    pub fn null() -> Self {
        Self(Value::Null)
    }
    pub fn boolean(b: bool) -> Self {
        Self(Value::Bool(b))
    }
    pub fn number(n: f64) -> Self {
        Self(Value::number(n))
    }
    pub fn string(s: &str) -> Self {
        Self(Value::String(s.into()))
    }
    pub fn object(id: ObjectId) -> Self {
        Self(Value::Object(id.0))
    }
    pub fn symbol(id: u64) -> Self {
        Self(Value::Symbol(id))
    }
    pub fn is_undefined(&self) -> bool {
        matches!(self.0, Value::Undefined)
    }
    pub fn is_null(&self) -> bool {
        matches!(self.0, Value::Null)
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self.0 {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }
    pub fn as_number(&self) -> Option<f64> {
        match self.0 {
            Value::Number(bits) => Some(f64::from_bits(bits)),
            _ => None,
        }
    }
    /// The string's text; lone surrogates become U+FFFD.
    pub fn as_string(&self) -> Option<String> {
        match &self.0 {
            Value::String(s) => Some(s.lossy().into_owned()),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<ObjectId> {
        match self.0 {
            Value::Object(id) => Some(ObjectId(id)),
            _ => None,
        }
    }
    pub fn as_symbol(&self) -> Option<u64> {
        match self.0 {
            Value::Symbol(id) => Some(id),
            _ => None,
        }
    }
    pub fn is_bigint(&self) -> bool {
        matches!(self.0, Value::BigInt(_))
    }
}

/// The embedder. The engine holds it behind an `Rc` and clones that before each call,
/// so `call` may re-enter the engine (including other host functions).
pub trait Host {
    /// [[Call]] (`new_target` None) or [[Construct]] of a host function.
    fn call(
        &self,
        engine: &mut Engine<'_>,
        function: u64,
        this: JsValue,
        args: &[JsValue],
        new_target: Option<ObjectId>,
    ) -> Result<JsValue>;
    /// Extra roots, reported at the start of every collection.
    fn trace_roots(&self, _out: &mut Vec<ObjectId>) {}
    /// Edges of a traced host object held outside its reserved slots. Called during
    /// marking: it must not call into the engine.
    fn trace(&self, _object: ObjectId, _class: u64, _payload: u64, _out: &mut Vec<ObjectId>) {}
    /// A host object was swept. Called after the collection, outside any script.
    fn finalize(&self, _class: u64, _payload: u64) {}
}

/// Adapts `Host` to the runtime's id-level tracer.
struct Tracer(Rc<dyn Host>);
impl HostTracer for Tracer {
    fn roots(&self, out: &mut Vec<Id>) {
        let mut ids = vec![];
        self.0.trace_roots(&mut ids);
        out.extend(ids.into_iter().map(|o| o.0));
    }
    fn trace(&self, id: Id, class: u64, payload: u64, out: &mut Vec<Id>) {
        let mut ids = vec![];
        self.0.trace(ObjectId(id), class, payload, &mut ids);
        out.extend(ids.into_iter().map(|o| o.0));
    }
}

impl Engine<'_> {
    /// Install the embedder: host function dispatch, tracing and finalization.
    pub fn set_host(&mut self, host: Rc<dyn Host>) {
        self.runtime
            .set_host_tracer(Some(Rc::new(Tracer(host.clone()))));
        self.host = Some(host);
    }

    pub(super) fn host_call(
        &mut self,
        function: u64,
        this: Value,
        args: Vec<Value>,
        new_target: Option<Id>,
    ) -> Result<Value> {
        let host = self
            .host
            .clone()
            .ok_or(ScriptError::Runtime("no embedder host"))?;
        self.frame(|e| {
            e.protect(&this)?;
            for arg in &args {
                e.protect(arg)?;
            }
            let args: Vec<_> = args.into_iter().map(JsValue).collect();
            host.call(e, function, JsValue(this), &args, new_target.map(ObjectId))
                .map(|v| v.0)
        })
    }

    pub(super) fn run_host_finalizers(&mut self) {
        let dead = self.runtime.take_finalized();
        if let Some(host) = self.host.clone() {
            for tag in dead {
                host.finalize(tag.class, tag.payload);
            }
        }
    }

    // ----- realms and scripts -----

    /// Create a fresh global environment and realm with all intrinsics, replacing any
    /// previous one, and return the global object. Scripts then share it
    /// (`evaluate_script`), unlike `execute`, which builds a new global per program.
    pub fn init_realm(&mut self) -> Result<ObjectId> {
        self.release_globals();
        self.steps_for_external_event();
        let global = self.new_environment(None, &[], true)?;
        self.global = Some(self.runtime.root(global)?);
        self.frame(|e| e.bootstrap(global))?;
        self.global_object()
            .ok_or(ScriptError::Runtime("no global object"))
    }
    pub fn global_object(&self) -> Option<ObjectId> {
        self.intrinsics.as_ref().map(|i| ObjectId(i.global))
    }
    pub fn object_prototype(&self) -> Option<ObjectId> {
        self.intrinsics.as_ref().map(|i| ObjectId(i.object))
    }
    pub fn function_prototype(&self) -> Option<ObjectId> {
        self.intrinsics.as_ref().map(|i| ObjectId(i.function))
    }
    /// ScriptEvaluation (§16.1.6) of a classic script against the current realm's global
    /// environment; returns the completion value. A parse error is a thrown SyntaxError.
    pub fn evaluate_script(&mut self, source: &str) -> Result<JsValue> {
        self.steps_for_external_event();
        self.eval_script(Value::String(source.into())).map(JsValue)
    }

    // ----- rooting -----

    pub fn root(&mut self, object: ObjectId) -> Result<ExternalRoot> {
        Ok(self.runtime.root(object.0)?)
    }
    pub fn unroot(&mut self, root: ExternalRoot) -> Result<()> {
        Ok(self.runtime.unroot(root)?)
    }
    pub fn is_alive(&self, object: ObjectId) -> bool {
        self.runtime.value(object.0).is_ok()
    }

    // ----- objects -----

    pub fn create_object(&mut self, prototype: Option<ObjectId>) -> Result<ObjectId> {
        self.frame(|e| e.new_object(prototype.map(|p| p.0)).map(ObjectId))
    }
    /// A new host object (E1) with `slots` reserved slots, all undefined.
    pub fn create_host_object(
        &mut self,
        prototype: Option<ObjectId>,
        class: u64,
        payload: u64,
        slots: usize,
        traced: bool,
    ) -> Result<ObjectId> {
        self.frame(|e| {
            let id = e.new_object(prototype.map(|p| p.0))?;
            e.protect(&Value::Object(id))?;
            e.attach_host(ObjectId(id), class, payload, slots, traced)?;
            Ok(ObjectId(id))
        })
    }
    /// Make an existing ordinary object a host object (used for the global object).
    pub fn attach_host(
        &mut self,
        object: ObjectId,
        class: u64,
        payload: u64,
        slots: usize,
        traced: bool,
    ) -> Result<()> {
        let mut data = self.data(object.0)?.clone();
        data.host_slots = Some(vec![Value::Undefined; slots]);
        self.replace_data(object.0, data)?;
        self.runtime.set_host_tag(
            object.0,
            HostTag {
                class,
                payload,
                traced,
            },
        )?;
        Ok(())
    }
    /// The class and payload words of a host object.
    pub fn host_class(&self, object: ObjectId) -> Option<(u64, u64)> {
        self.runtime
            .host_tag(object.0)
            .map(|t| (t.class, t.payload))
    }
    pub fn set_host_payload(&mut self, object: ObjectId, payload: u64) -> Result<()> {
        let mut tag = self
            .runtime
            .host_tag(object.0)
            .ok_or(ScriptError::Runtime("not a host object"))?;
        tag.payload = payload;
        Ok(self.runtime.set_host_tag(object.0, tag)?)
    }
    pub fn reserved_slot(&self, object: ObjectId, index: usize) -> Result<JsValue> {
        self.data(object.0)?
            .host_slots
            .as_ref()
            .and_then(|s| s.get(index))
            .cloned()
            .map(JsValue)
            .ok_or(ScriptError::Runtime("reserved slot out of range"))
    }
    pub fn set_reserved_slot(
        &mut self,
        object: ObjectId,
        index: usize,
        value: JsValue,
    ) -> Result<()> {
        let mut data = self.data(object.0)?.clone();
        let slot = data
            .host_slots
            .as_mut()
            .and_then(|s| s.get_mut(index))
            .ok_or(ScriptError::Runtime("reserved slot out of range"))?;
        *slot = value.0;
        self.replace_data(object.0, data)
    }

    // ----- functions -----

    /// A host function object (E2) with %Function.prototype%, `length` and `name`.
    pub fn create_function(
        &mut self,
        name: &str,
        length: usize,
        function: u64,
        constructor: bool,
    ) -> Result<ObjectId> {
        let prototype = self
            .intrinsics
            .as_ref()
            .ok_or(ScriptError::Runtime("no realm"))?
            .function;
        self.frame(|e| {
            let id = e.new_object(Some(prototype))?;
            e.protect(&Value::Object(id))?;
            let mut data = e.data(id)?.clone();
            data.internal = Some(super::promises::Internal::Host { function });
            e.objects.insert(
                id,
                super::vm::Object::Builtin {
                    name: if constructor {
                        HOST_CONSTRUCTOR
                    } else {
                        HOST_FUNCTION
                    }
                    .into(),
                    data: e.data(id)?.clone(),
                },
            );
            e.replace_data(id, data)?;
            for (key, value) in [
                ("length", Value::number(length as f64)),
                ("name", Value::String(name.into())),
            ] {
                e.define(
                    id,
                    key,
                    Descriptor {
                        value: Some(value),
                        configurable: Some(true),
                        ..Descriptor::default()
                    },
                )?;
            }
            Ok(ObjectId(id))
        })
    }
    pub fn is_callable(&self, value: &JsValue) -> bool {
        self.callable(&value.0)
    }
    pub fn call_function(
        &mut self,
        function: &JsValue,
        this: &JsValue,
        args: &[JsValue],
    ) -> Result<JsValue> {
        let args = args.iter().map(|a| a.0.clone()).collect();
        self.frame(|e| {
            e.protect(&function.0)?;
            e.protect(&this.0)?;
            e.invoke_this(function.0.clone(), this.0.clone(), args)
                .map(JsValue)
        })
    }
    pub fn construct_object(&mut self, function: &JsValue, args: &[JsValue]) -> Result<JsValue> {
        let args = args.iter().map(|a| a.0.clone()).collect();
        self.frame(|e| {
            e.protect(&function.0)?;
            e.construct(function.0.clone(), args).map(JsValue)
        })
    }

    // ----- properties -----

    pub fn get_property(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
    ) -> Result<JsValue> {
        let key: Key = key.into().into();
        self.frame(|e| e.get_member(Value::Object(object.0), key).map(JsValue))
    }
    pub fn set_property(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
        value: JsValue,
    ) -> Result<()> {
        let key: Key = key.into().into();
        self.frame(|e| {
            e.protect(&value.0)?;
            e.set_member(Value::Object(object.0), key, value.0)
        })
    }
    /// [[DefineOwnProperty]] of a data property; false when the object refused it.
    pub fn define_property(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
        value: JsValue,
        writable: bool,
        enumerable: bool,
        configurable: bool,
    ) -> Result<bool> {
        let key: Key = key.into().into();
        self.frame(|e| {
            e.define(
                object.0,
                key,
                Descriptor {
                    value: Some(value.0),
                    writable: Some(writable),
                    enumerable: Some(enumerable),
                    configurable: Some(configurable),
                    ..Descriptor::default()
                },
            )
        })
    }
    /// [[DefineOwnProperty]] of an accessor property (WebIDL attributes).
    pub fn define_accessor(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
        getter: Option<ObjectId>,
        setter: Option<ObjectId>,
        enumerable: bool,
        configurable: bool,
    ) -> Result<bool> {
        let key: Key = key.into().into();
        let function =
            |f: Option<ObjectId>| Some(f.map_or(Value::Undefined, |f| Value::Object(f.0)));
        self.frame(|e| {
            e.define(
                object.0,
                key,
                Descriptor {
                    get: function(getter),
                    set: function(setter),
                    enumerable: Some(enumerable),
                    configurable: Some(configurable),
                    ..Descriptor::default()
                },
            )
        })
    }
    pub fn delete_property(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
    ) -> Result<bool> {
        let key: Key = key.into().into();
        self.frame(|e| e.delete(object.0, key))
    }
    pub fn own_property_keys(&mut self, object: ObjectId) -> Result<Vec<PropertyKey>> {
        let keys = self.frame(|e| e.own_keys(object.0))?;
        Ok(keys
            .into_iter()
            .map(|k| match k {
                Key::String(s) => PropertyKey::String(s),
                Key::Symbol(s) => PropertyKey::Symbol(s),
            })
            .collect())
    }
    pub fn prototype_of(&self, object: ObjectId) -> Result<Option<ObjectId>> {
        Ok(self.data(object.0)?.prototype.map(ObjectId))
    }

    // ----- conversions and errors -----

    /// ToString (§7.1.17).
    pub fn to_string_value(&mut self, value: &JsValue) -> Result<String> {
        let value = value.0.clone();
        self.frame(|e| e.to_string(value))
    }
    /// The thrown value of an abrupt completion, if it is one.
    pub fn exception_value(&self, error: &ScriptError) -> Option<JsValue> {
        match error {
            ScriptError::Thrown(exception) => Some(JsValue(exception.value.clone())),
            _ => None,
        }
    }
    /// An abrupt completion throwing `value`, for a host function to return.
    pub fn throw_value(&self, value: JsValue) -> ScriptError {
        self.thrown(value.0)
    }
    /// A new `name` error object (TypeError, RangeError, ...) thrown as a completion.
    pub fn throw_new_error(&mut self, name: &str, message: &str) -> ScriptError {
        match self.error_value(name, message) {
            Ok(value) => self.thrown(value),
            Err(error) => error,
        }
    }
}
