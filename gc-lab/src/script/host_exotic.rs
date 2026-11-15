//! Host exotic objects (E3): essential internal methods (ECMA-262 §6.1.7.2)
//! answered by the embedder through `Host::exotic`. Design: HOST_EXOTIC.md.
//!
//! A host exotic keeps an ordinary `Data` (prototype, extensibility, host slots,
//! property map) and `is_proxy` answers true for it, so ordinary fast paths skip
//! it and every internal method reaches the `proxy_*` entry points, which call
//! the `exotic_*` methods below before touching any script-proxy slot. An
//! `ExoticReply::Default` runs the ordinary algorithm, as a trap-less
//! `js::BaseProxyHandler` does. There are no §10.5 invariant checks: the
//! embedder is trusted, as with SpiderMonkey's DOM proxies.
use super::{
    embed::{ExoticOp, ExoticReply, JsValue, ObjectId, PartialDescriptor, PropertyKey},
    objects::{Descriptor, Key, Kind, Property},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

fn embed_key(key: &Key) -> PropertyKey {
    match key {
        Key::String(s) => PropertyKey::String(s.clone()),
        Key::Symbol(s) => PropertyKey::Symbol(*s),
    }
}

/// The accessor field of a validated descriptor: undefined or a callable.
fn accessor(v: Option<Value>) -> Option<Option<ObjectId>> {
    v.map(|v| match v {
        Value::Object(id) => Some(ObjectId(id)),
        _ => None,
    })
}

fn op_name(op: &ExoticOp) -> &'static str {
    match op {
        ExoticOp::GetPrototypeOf => "[[GetPrototypeOf]]",
        ExoticOp::SetPrototypeOf(_) => "[[SetPrototypeOf]]",
        ExoticOp::IsExtensible => "[[IsExtensible]]",
        ExoticOp::PreventExtensions => "[[PreventExtensions]]",
        ExoticOp::GetOwnProperty(_) => "[[GetOwnProperty]]",
        ExoticOp::DefineOwnProperty(..) => "[[DefineOwnProperty]]",
        ExoticOp::HasProperty(_) => "[[HasProperty]]",
        ExoticOp::Get(..) => "[[Get]]",
        ExoticOp::Set(..) => "[[Set]]",
        ExoticOp::Delete(_) => "[[Delete]]",
        ExoticOp::OwnPropertyKeys => "[[OwnPropertyKeys]]",
    }
}

impl Engine<'_> {
    /// The handler word of a host exotic object.
    pub(super) fn exotic_handler(&self, id: Id) -> Option<u64> {
        match self.objects.get(&id) {
            Some(Object::HostExotic { handler, .. }) => Some(*handler),
            _ => None,
        }
    }

    /// Turn a fresh ordinary object into a host exotic (same Data, same edges).
    pub(super) fn make_host_exotic(&mut self, id: Id, handler: u64) -> Result<()> {
        let data = self.data(id)?.clone();
        self.objects
            .insert(id, Object::HostExotic { data, handler });
        Ok(())
    }

    /// Send `op` to the host, inside a frame that keeps the object alive. With
    /// no host installed every method is ordinary.
    fn exotic_send(&mut self, id: Id, handler: u64, op: ExoticOp) -> Result<ExoticReply> {
        let Some(host) = self.host.clone() else {
            return Ok(ExoticReply::Default);
        };
        self.protect(&Value::Object(id))?;
        let reply = host.exotic(self, ObjectId(id), handler, op)?;
        // Root what the reply carries for the rest of the internal method.
        match &reply {
            ExoticReply::Prototype(Some(p)) => self.protect(&Value::Object(p.0))?,
            ExoticReply::Value(v) => self.protect(&v.0)?,
            ExoticReply::Property(Some(d)) => {
                if let Some(v) = &d.value {
                    self.protect(&v.0)?;
                }
                for f in [d.getter, d.setter].into_iter().flatten() {
                    self.protect(&Value::Object(f.0))?;
                }
            }
            _ => {}
        }
        Ok(reply)
    }

    fn exotic_bad_reply<T>(&mut self, op: &ExoticOp) -> Result<T> {
        self.throw_error(
            "TypeError",
            &format!(
                "host exotic object returned an invalid result for {}",
                op_name(op)
            ),
        )
    }

    /// An op whose reply is a boolean; `default` runs on `Default`.
    fn exotic_bool(
        &mut self,
        id: Id,
        handler: u64,
        op: ExoticOp,
        default: impl FnOnce(&mut Self) -> Result<bool>,
    ) -> Result<bool> {
        self.frame(|e| match e.exotic_send(id, handler, op.clone())? {
            ExoticReply::Default => default(e),
            ExoticReply::Bool(b) => Ok(b),
            _ => e.exotic_bad_reply(&op),
        })
    }

    pub(super) fn exotic_get_prototype_of(&mut self, id: Id, handler: u64) -> Result<Option<Id>> {
        let op = ExoticOp::GetPrototypeOf;
        self.frame(|e| match e.exotic_send(id, handler, op.clone())? {
            ExoticReply::Default => Ok(e.data(id)?.prototype),
            ExoticReply::Prototype(p) => Ok(p.map(|p| p.0)),
            _ => e.exotic_bad_reply(&op),
        })
    }

    pub(super) fn exotic_set_prototype(
        &mut self,
        id: Id,
        handler: u64,
        proto: Option<Id>,
    ) -> Result<bool> {
        let op = ExoticOp::SetPrototypeOf(proto.map(ObjectId));
        self.exotic_bool(id, handler, op, |e| e.ordinary_set_prototype(id, proto))
    }

    pub(super) fn exotic_is_extensible(&mut self, id: Id, handler: u64) -> Result<bool> {
        self.exotic_bool(id, handler, ExoticOp::IsExtensible, |e| {
            Ok(e.data(id)?.extensible)
        })
    }

    pub(super) fn exotic_prevent_extensions(&mut self, id: Id, handler: u64) -> Result<bool> {
        self.exotic_bool(id, handler, ExoticOp::PreventExtensions, |e| {
            e.data_mut(id)?.extensible = false;
            Ok(true)
        })
    }

    pub(super) fn exotic_own(
        &mut self,
        id: Id,
        handler: u64,
        key: &Key,
    ) -> Result<Option<Property>> {
        let op = ExoticOp::GetOwnProperty(embed_key(key));
        self.frame(|e| match e.exotic_send(id, handler, op.clone())? {
            ExoticReply::Default => e.ordinary_own(id, key.clone()),
            ExoticReply::Property(None) => Ok(None),
            ExoticReply::Property(Some(d)) => {
                // HOST_EXOTIC.md "Debug invariant check": legacy platform objects
                // report configurable properties (WebIDL §3.9); warn, never throw.
                #[cfg(debug_assertions)]
                if !d.configurable {
                    eprintln!("host exotic: non-configurable own property {key:?}");
                }
                let function =
                    |f: Option<ObjectId>| f.map_or(Value::Undefined, |f| Value::Object(f.0));
                // A descriptor with a value is a data property; otherwise an accessor.
                let kind = match d.value {
                    Some(v) => Kind::Data {
                        value: v.0,
                        writable: d.writable,
                    },
                    None => Kind::Accessor {
                        get: function(d.getter),
                        set: function(d.setter),
                    },
                };
                Ok(Some(Property {
                    kind,
                    enumerable: d.enumerable,
                    configurable: d.configurable,
                }))
            }
            _ => e.exotic_bad_reply(&op),
        })
    }

    pub(super) fn exotic_define(
        &mut self,
        id: Id,
        handler: u64,
        key: &Key,
        d: Descriptor,
    ) -> Result<bool> {
        let partial = PartialDescriptor {
            value: d.value.clone().map(JsValue),
            writable: d.writable,
            get: accessor(d.get.clone()),
            set: accessor(d.set.clone()),
            enumerable: d.enumerable,
            configurable: d.configurable,
        };
        let op = ExoticOp::DefineOwnProperty(embed_key(key), partial);
        self.frame(|e| {
            if let Some(v) = &d.value {
                e.protect(v)?;
            }
            e.exotic_bool(id, handler, op, |e| e.ordinary_define(id, key.clone(), d))
        })
    }

    /// [[HasProperty]]; `Default` is OrdinaryHasProperty (§10.1.7.1).
    pub(super) fn exotic_has(&mut self, id: Id, handler: u64, key: &Key) -> Result<bool> {
        let op = ExoticOp::HasProperty(embed_key(key));
        self.exotic_bool(id, handler, op, |e| {
            if e.own(id, key.clone())?.is_some() {
                return Ok(true);
            }
            match e.get_prototype_of(id)? {
                Some(parent) => e.has(parent, key.clone()),
                None => Ok(false),
            }
        })
    }

    /// [[Get]]; `Default` is OrdinaryGet (§10.1.8.1).
    pub(super) fn exotic_get(
        &mut self,
        id: Id,
        handler: u64,
        key: &Key,
        receiver: Value,
    ) -> Result<Value> {
        let op = ExoticOp::Get(embed_key(key), JsValue(receiver.clone()));
        self.frame(|e| {
            e.protect(&receiver)?;
            match e.exotic_send(id, handler, op.clone())? {
                ExoticReply::Value(v) => Ok(v.0),
                ExoticReply::Default => match e.own(id, key.clone())? {
                    None => match e.get_prototype_of(id)? {
                        Some(parent) => e.ordinary_get(parent, key.clone(), receiver),
                        None => Ok(Value::Undefined),
                    },
                    Some(Property {
                        kind: Kind::Data { value, .. },
                        ..
                    }) => Ok(value),
                    Some(Property {
                        kind: Kind::Accessor { get, .. },
                        ..
                    }) => {
                        if get == Value::Undefined {
                            Ok(Value::Undefined)
                        } else {
                            e.invoke_this(get, receiver, vec![])
                        }
                    }
                },
                _ => e.exotic_bad_reply(&op),
            }
        })
    }

    /// [[Set]]; `Default` is OrdinarySet (§10.1.9.1).
    pub(super) fn exotic_set(
        &mut self,
        id: Id,
        handler: u64,
        key: &Key,
        value: Value,
        receiver: Value,
    ) -> Result<bool> {
        let op = ExoticOp::Set(
            embed_key(key),
            JsValue(value.clone()),
            JsValue(receiver.clone()),
        );
        self.frame(|e| {
            e.protect(&value)?;
            e.protect(&receiver)?;
            e.exotic_bool(id, handler, op, |e| {
                // §10.1.9.2 step 1-2: ownDesc, else the parent's [[Set]].
                let own = e.own(id, key.clone())?;
                if own.is_none() {
                    if let Some(parent) = e.get_prototype_of(id)? {
                        return e.ordinary_set(parent, key.clone(), value, receiver);
                    }
                }
                e.set_with_own_descriptor(own, key.clone(), value, receiver)
            })
        })
    }

    pub(super) fn exotic_delete(&mut self, id: Id, handler: u64, key: &Key) -> Result<bool> {
        let op = ExoticOp::Delete(embed_key(key));
        self.exotic_bool(id, handler, op, |e| e.ordinary_delete(id, key.clone()))
    }

    pub(super) fn exotic_own_keys(&mut self, id: Id, handler: u64) -> Result<Vec<Key>> {
        let op = ExoticOp::OwnPropertyKeys;
        self.frame(|e| match e.exotic_send(id, handler, op.clone())? {
            ExoticReply::Default => e.ordinary_own_keys(id),
            ExoticReply::Keys(keys) => Ok(keys.into_iter().map(Key::from).collect()),
            _ => e.exotic_bad_reply(&op),
        })
    }
}
