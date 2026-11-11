//! OBJECT_MODEL.md slice 3: global-name slot hints.
//!
//! Each compiled chunk keeps one hint per name (`Chunk::hints`): the index of the name's
//! entry in the global object's `PropertyMap`. The environment walk of §9.1.2.1
//! GetIdentifierReference still runs on every access, so a declarative binding, a `with`
//! object or an eval-introduced `var` that shadows the global is always found first. The
//! hint only replaces the last step, the global object record's HasBinding /
//! GetBindingValue / SetMutableBinding (§9.1.1.4.1, .4, .6 → §9.1.1.2), by a direct
//! entry access. That is equivalent only for an ordinary global object and an own data
//! property, which is all the fast path accepts: [[HasProperty]], [[Get]] and [[Set]]
//! (OrdinarySet with Receiver = O on an own writable data property, §10.1.9.2 steps 2-3)
//! have no other observable step there.
//!
//! The hint is validated by key on every use (`PropertyMap::named_at`), so it needs no
//! invalidation epoch: a delete, a redefinition as an accessor or a reindex makes the
//! check fail and the slow path runs.
//!
//! Part b (checkpoint 44): each hint also remembers the root environment (the global
//! Environment Record: no parent, no `with` object, no import aliases) at which the
//! environment walk ended on the name's VarNames entry (`object_bindings`, §9.1.1.4
//! [[VarNames]]). For that record the walk's answer cannot change: `object_bindings` only
//! grows, except through DeleteBinding (§9.1.1.4.7), which bumps `Engine::var_names_epoch`
//! and so drops every root; a global record never gains a `with` object or an alias; and
//! environment ids carry a serial, so they are never reused. While the name is in VarNames, a
//! later script cannot declare it lexically (§16.1.7 step 5.a), so nothing can shadow it.
//! A use that starts at that same record therefore skips the walk and goes straight to the
//! object record step, which is still validated by key.
use super::objects::{Key, Kind, Property};
use super::props::PropertyMap;
use super::vm::{Engine, Object, ScriptError, Value};
use crate::dual::Id;
use std::cell::Cell;

type Result<T> = std::result::Result<T, ScriptError>;

/// A hint that has not found an entry yet.
const UNTRIED: u32 = u32::MAX;
/// The name resolved to a global property the fast path does not handle (an accessor or
/// a read-only property): stay on the slow path.
const FAILED: u32 = u32::MAX - 1;

/// The per-name, per-chunk hint (`Chunk::hints`).
#[derive(Clone, Debug)]
pub(super) struct NameHint {
    /// Index of the name's entry in the global object's `PropertyMap`.
    entry: Cell<u32>,
    /// The root environment whose VarNames held the name, and the engine's
    /// `var_names_epoch` then (see the module comment).
    root: Cell<Option<(Id, u32)>>,
}

impl NameHint {
    pub(super) fn new() -> Self {
        NameHint {
            entry: Cell::new(UNTRIED),
            root: Cell::new(None),
        }
    }
    /// The walk from `env` is known to end at the global object record on its first step.
    pub(super) fn at_root(&self, env: Id, epoch: u32) -> bool {
        self.root.get() == Some((env, epoch))
    }
    pub(super) fn set_root(&self, env: Id, epoch: u32) {
        self.root.set(Some((env, epoch)));
    }
}

/// The own properties of `global`, when it is an ordinary object.
fn ordinary_properties(object: Option<&Object>) -> Option<&PropertyMap> {
    match object? {
        Object::Ordinary(d)
            if d.typed.is_none() && d.namespace.is_none() && d.parameter_map.is_none() =>
        {
            Some(&d.properties)
        }
        _ => None,
    }
}

/// The entry index of the own property `name` in `properties`, through `hint`.
fn hinted_entry(properties: &PropertyMap, name: &str, hint: &NameHint) -> Option<usize> {
    let at = hint.entry.get();
    if at == FAILED {
        return None;
    }
    if at != UNTRIED && properties.named_at(at as usize, name).is_some() {
        return Some(at as usize);
    }
    // An absent name leaves the hint untried: the binding may be created later.
    let found = properties.index_of(&Key::String(name.to_owned()))?;
    if found >= FAILED as usize {
        return None;
    }
    hint.entry.set(found as u32);
    Some(found)
}

impl Engine<'_> {
    /// The global object's properties, in one map lookup.
    fn global_properties(&self) -> Option<&PropertyMap> {
        let global = self.intrinsics.as_ref()?.global;
        ordinary_properties(self.objects.get(&global))
    }
    /// GetBindingValue on the global object record for an own data property.
    pub(super) fn global_hint_get(&self, name: &str, hint: Option<&NameHint>) -> Option<Value> {
        let hint = hint?;
        if hint.entry.get() == FAILED {
            return None;
        }
        let properties = self.global_properties()?;
        let at = hinted_entry(properties, name, hint)?;
        match &properties.named_at(at, name)?.kind {
            Kind::Data { value, .. } => Some(value.clone()),
            Kind::Accessor { .. } => {
                hint.entry.set(FAILED);
                None
            }
        }
    }
    /// HasBinding on the global object record: the global has an own property `name`.
    pub(super) fn global_hint_has(&self, name: &str, hint: Option<&NameHint>) -> bool {
        hint.is_some_and(|hint| {
            self.global_properties()
                .is_some_and(|properties| hinted_entry(properties, name, hint).is_some())
        })
    }
    /// SetMutableBinding on the global object record for an own writable data property.
    /// Returns false (nothing mutated) when the slow path must run.
    pub(super) fn global_hint_set(
        &mut self,
        name: &str,
        hint: Option<&NameHint>,
        value: &Value,
    ) -> Result<bool> {
        let Some(hint) = hint else {
            return Ok(false);
        };
        if hint.entry.get() == FAILED {
            return Ok(false);
        }
        let Some(global) = self.intrinsics.as_ref().map(|i| i.global) else {
            return Ok(false);
        };
        // One map lookup: `link`/`unlink` touch only the runtime records, never `objects`.
        let Some(Object::Ordinary(data)) = self.objects.get_mut(&global) else {
            return Ok(false);
        };
        if data.typed.is_some() || data.namespace.is_some() || data.parameter_map.is_some() {
            return Ok(false);
        }
        let Some(at) = hinted_entry(&data.properties, name, hint) else {
            return Ok(false);
        };
        let Some(Property {
            kind:
                Kind::Data {
                    value: slot,
                    writable: true,
                    ..
                },
            ..
        }) = data.properties.named_at_mut(at, name)
        else {
            hint.entry.set(FAILED);
            return Ok(false);
        };
        // As `plain_set`: link the new edge before storing, unlink the old one after.
        if let Some(child) = value.id() {
            self.runtime.link(global, child)?;
        }
        let old = std::mem::replace(slot, value.clone()).id();
        if let Some(child) = old {
            self.runtime.unlink(global, child)?;
        }
        Ok(true)
    }
}
