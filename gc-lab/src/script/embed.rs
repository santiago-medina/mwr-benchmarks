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
use super::objects::{Descriptor, Key, Kind};
use super::vm::{Engine, ScriptError, Value};
use crate::dual::{ExternalRoot, HostTag, HostTracer, Id};
use std::rc::Rc;

type Result<T> = std::result::Result<T, ScriptError>;

/// `Object::Builtin` names of host functions; the constructor name makes them
/// constructible (`Engine::constructible`).
pub(super) const HOST_FUNCTION: &str = "%HostFunction%";
pub(super) const HOST_CONSTRUCTOR: &str = "%HostConstructor%";

/// A realm created through the embedding API (REALMS.md "Servo"): one per Servo
/// global (window, iframe, worker, worklet). Realm 0 is the first one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RealmId(usize);

/// An object in this engine's heap. Identity is stable while the object lives (the
/// collector does not move objects).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId(pub(super) Id);

/// An own property as seen by the embedder (ECMA-262 §6.2.6 Property Descriptor):
/// `value`/`writable` for data properties, `getter`/`setter` for accessors.
/// A property descriptor whose fields may be absent (ECMA-262 §6.2.6), as
/// [[DefineOwnProperty]] takes it. `get`/`set` of `Some(None)` mean undefined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialDescriptor {
    pub value: Option<JsValue>,
    pub writable: Option<bool>,
    pub get: Option<Option<ObjectId>>,
    pub set: Option<Option<ObjectId>>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDescriptor {
    pub value: Option<JsValue>,
    pub writable: bool,
    pub getter: Option<ObjectId>,
    pub setter: Option<ObjectId>,
    pub enumerable: bool,
    pub configurable: bool,
}

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
    /// InitializeHostDefinedRealm (§9.6) for an embedder global: a new realm with fresh
    /// intrinsics and global object. The running realm is unchanged, except that the
    /// first call also makes realm 0 the running realm (there was none). The realm
    /// count is capped at 64 (`new_realm`), a test-harness guard to lift for Servo.
    pub fn create_global_realm(&mut self) -> Result<RealmId> {
        if self.intrinsics.is_none() && self.realms.is_empty() {
            self.init_realm()?;
            return Ok(RealmId(0));
        }
        self.frame(|e| e.new_realm()).map(RealmId)
    }
    /// The running realm.
    pub fn current_realm_id(&self) -> RealmId {
        RealmId(self.current_realm)
    }
    /// Make `realm` the running realm (mozjs `JS::EnterRealm`); returns the previous one,
    /// to pass back when leaving (`JS::LeaveRealm`).
    pub fn enter_realm_id(&mut self, realm: RealmId) -> Result<RealmId> {
        if realm.0 != self.current_realm && realm.0 >= self.realms.len() {
            return Err(ScriptError::Runtime("no such realm"));
        }
        Ok(RealmId(self.enter_realm(realm.0)))
    }
    /// The global object of `realm`.
    pub fn realm_global(&self, realm: RealmId) -> Result<ObjectId> {
        let intrinsics = if realm.0 == self.current_realm {
            self.intrinsics.as_ref()
        } else {
            self.realms.get(realm.0).and_then(|r| r.intrinsics.as_ref())
        };
        intrinsics
            .map(|i| ObjectId(i.global))
            .ok_or(ScriptError::Runtime("no such realm"))
    }
    /// The realm an object was allocated in ([[Realm]] of functions, §10.2).
    pub fn object_realm(&self, object: ObjectId) -> RealmId {
        RealmId(self.realm_of(object.0))
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
    /// %Error.prototype% of the current realm.
    pub fn error_prototype(&self) -> Option<ObjectId> {
        self.intrinsics
            .as_ref()
            .and_then(|i| i.errors.get("Error"))
            .map(|&id| ObjectId(id))
    }
    /// %Array.prototype% of the current realm.
    pub fn array_prototype(&self) -> Option<ObjectId> {
        self.intrinsics.as_ref().map(|i| ObjectId(i.array))
    }
    /// %IteratorPrototype% (§27.1.3) of the current realm: the [[Prototype]] of
    /// %ArrayIteratorPrototype%.
    pub fn iterator_prototype(&self) -> Option<ObjectId> {
        let i = self.intrinsics.as_ref()?;
        self.data(i.array_iterator).ok()?.prototype.map(ObjectId)
    }
    /// The symbol value id of the well-known symbol `@@name` (§6.1.5.1), e.g.
    /// `well_known_symbol("iterator")`.
    pub fn well_known_symbol(&self, name: &str) -> Option<u64> {
        super::symbols::WELL_KNOWN
            .iter()
            .position(|n| *n == name)
            .map(|i| i as u64)
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
    /// [[DefineOwnProperty]] (§10.1.6, or the exotic object's own) of a
    /// partial descriptor; false when the object refused it.
    pub fn define_own_property(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
        desc: PartialDescriptor,
    ) -> Result<bool> {
        let key: Key = key.into().into();
        let function = |f: Option<ObjectId>| f.map_or(Value::Undefined, |f| Value::Object(f.0));
        let value = desc.value.map(|v| v.0);
        self.frame(|e| {
            if let Some(v) = &value {
                e.protect(v)?;
            }
            e.define(
                object.0,
                key,
                Descriptor {
                    value,
                    writable: desc.writable,
                    get: desc.get.map(function),
                    set: desc.set.map(function),
                    enumerable: desc.enumerable,
                    configurable: desc.configurable,
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
    /// [[SetPrototypeOf]] (§10.1.2); false when the object refused it.
    pub fn set_object_prototype(
        &mut self,
        object: ObjectId,
        prototype: Option<ObjectId>,
    ) -> Result<bool> {
        self.frame(|e| e.set_prototype(object.0, prototype.map(|p| p.0)))
    }
    /// Make `object` an immutable prototype exotic object (§10.4.7): later
    /// [[SetPrototypeOf]] calls only accept its current [[Prototype]]. Used for
    /// WebIDL global objects. False for proxies.
    pub fn set_immutable_prototype(&mut self, object: ObjectId) -> Result<bool> {
        if self.is_proxy(object.0) {
            return Ok(false);
        }
        let mut data = self.data(object.0)?.clone();
        data.immutable_prototype = true;
        self.replace_data(object.0, data)?;
        Ok(true)
    }
    /// HasProperty (§7.3.12).
    pub fn has_property(&mut self, object: ObjectId, key: impl Into<PropertyKey>) -> Result<bool> {
        let key: Key = key.into().into();
        self.frame(|e| e.has(object.0, key))
    }
    pub fn prototype_of(&self, object: ObjectId) -> Result<Option<ObjectId>> {
        Ok(self.data(object.0)?.prototype.map(ObjectId))
    }
    /// [[GetOwnProperty]] (§10.1.5, or the exotic object's own).
    pub fn own_property(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
    ) -> Result<Option<PropertyDescriptor>> {
        let key: Key = key.into().into();
        let property = self.frame(|e| e.own(object.0, key))?;
        Ok(property.map(|p| {
            let (value, getter, setter) = match p.kind {
                Kind::Data { value, writable } => {
                    return PropertyDescriptor {
                        value: Some(JsValue(value)),
                        writable,
                        getter: None,
                        setter: None,
                        enumerable: p.enumerable,
                        configurable: p.configurable,
                    }
                }
                Kind::Accessor { get, set } => (None, get, set),
            };
            let function = |v: Value| match v {
                Value::Object(id) => Some(ObjectId(id)),
                _ => None,
            };
            PropertyDescriptor {
                value,
                writable: false,
                getter: function(getter),
                setter: function(setter),
                enumerable: p.enumerable,
                configurable: p.configurable,
            }
        }))
    }
    /// [[Get]] with an explicit receiver (§10.1.8; `Reflect.get` with a receiver).
    pub fn get_property_with_receiver(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
        receiver: &JsValue,
    ) -> Result<JsValue> {
        let key: Key = key.into().into();
        let receiver = receiver.0.clone();
        self.frame(|e| {
            e.protect(&receiver)?;
            e.ordinary_get(object.0, key, receiver).map(JsValue)
        })
    }
    /// [[Set]] with an explicit receiver (§10.1.9); false when it was refused.
    pub fn set_property_with_receiver(
        &mut self,
        object: ObjectId,
        key: impl Into<PropertyKey>,
        value: JsValue,
        receiver: &JsValue,
    ) -> Result<bool> {
        let key: Key = key.into().into();
        let receiver = receiver.0.clone();
        self.frame(|e| {
            e.protect(&value.0)?;
            e.protect(&receiver)?;
            e.ordinary_set(object.0, key, value.0, receiver)
        })
    }
    /// SetIntegrityLevel(object, frozen) (§7.3.15); false when [[PreventExtensions]]
    /// or a [[DefineOwnProperty]] refused.
    pub fn freeze_object(&mut self, object: ObjectId) -> Result<bool> {
        self.frame(|e| {
            if !e.prevent_extensions(object.0)? {
                return Ok(false);
            }
            for key in e.own_keys(object.0)? {
                let mut d = Descriptor {
                    configurable: Some(false),
                    ..Descriptor::default()
                };
                match e.own(object.0, &key)? {
                    None => continue,
                    Some(p) if matches!(p.kind, Kind::Data { .. }) => d.writable = Some(false),
                    Some(_) => {}
                }
                if !e.define(object.0, &key, d)? {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }
    /// SameValue (§7.2.10).
    pub fn same_value(&self, a: &JsValue, b: &JsValue) -> bool {
        a.0.same_value(&b.0)
    }
    /// IsArray (§7.2.2): throws on a revoked proxy.
    pub fn is_array_value(&mut self, value: &JsValue) -> Result<bool> {
        self.is_array(&value.0)
    }
    /// GetFunctionRealm (§7.3.24).
    pub fn function_realm_id(&mut self, function: &JsValue) -> Result<RealmId> {
        self.function_realm(&function.0).map(RealmId)
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
