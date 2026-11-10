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
use super::objects::{Key, Kind, Property};
use super::props::PropertyMap;
use super::vm::{Engine, Object, ScriptError, Value};
use std::cell::Cell;

type Result<T> = std::result::Result<T, ScriptError>;

/// A hint that has not found an entry yet.
pub(super) const UNTRIED: u32 = u32::MAX;
/// The name resolved to a global property the fast path does not handle (an accessor or
/// a read-only property): stay on the slow path.
const FAILED: u32 = u32::MAX - 1;

impl Engine<'_> {
    /// The own properties of the global object, when it is an ordinary object.
    fn global_properties(&self) -> Option<&PropertyMap> {
        let global = self.intrinsics.as_ref()?.global;
        match self.objects.get(&global)? {
            Object::Ordinary(d)
                if d.typed.is_none() && d.namespace.is_none() && d.parameter_map.is_none() =>
            {
                Some(&d.properties)
            }
            _ => None,
        }
    }
    /// The entry index of the global object's own property `name`, through `hint`.
    fn hinted_entry(&self, name: &str, hint: &Cell<u32>) -> Option<usize> {
        let at = hint.get();
        if at == FAILED {
            return None;
        }
        let properties = self.global_properties()?;
        if at != UNTRIED && properties.named_at(at as usize, name).is_some() {
            return Some(at as usize);
        }
        // An absent name leaves the hint untried: the binding may be created later.
        let found = properties.index_of(&Key::String(name.to_owned()))?;
        if found >= FAILED as usize {
            return None;
        }
        hint.set(found as u32);
        Some(found)
    }
    /// GetBindingValue on the global object record for an own data property.
    pub(super) fn global_hint_get(&self, name: &str, hint: Option<&Cell<u32>>) -> Option<Value> {
        let hint = hint?;
        let at = self.hinted_entry(name, hint)?;
        match &self.global_properties()?.named_at(at, name)?.kind {
            Kind::Data { value, .. } => Some(value.clone()),
            Kind::Accessor { .. } => {
                hint.set(FAILED);
                None
            }
        }
    }
    /// HasBinding on the global object record: the global has an own property `name`.
    pub(super) fn global_hint_has(&self, name: &str, hint: Option<&Cell<u32>>) -> bool {
        hint.is_some_and(|hint| self.hinted_entry(name, hint).is_some())
    }
    /// SetMutableBinding on the global object record for an own writable data property.
    /// Returns false (nothing mutated) when the slow path must run.
    pub(super) fn global_hint_set(
        &mut self,
        name: &str,
        hint: Option<&Cell<u32>>,
        value: &Value,
    ) -> Result<bool> {
        let Some(hint) = hint else {
            return Ok(false);
        };
        let Some(at) = self.hinted_entry(name, hint) else {
            return Ok(false);
        };
        let Some(properties) = self.global_properties() else {
            return Ok(false);
        };
        if !matches!(
            properties.named_at(at, name),
            Some(Property {
                kind: Kind::Data { writable: true, .. },
                ..
            })
        ) {
            hint.set(FAILED);
            return Ok(false);
        }
        let global = self.intrinsics.as_ref().unwrap().global;
        // As `plain_set`: link the new edge before storing, unlink the old one after.
        if let Some(child) = value.id() {
            self.runtime.link(global, child)?;
        }
        let data = self.data_mut(global)?;
        let mut old = None;
        if let Some(Property {
            kind: Kind::Data { value: slot, .. },
            ..
        }) = data.properties.named_at_mut(at, name)
        {
            old = std::mem::replace(slot, value.clone()).id();
        }
        if let Some(child) = old {
            self.runtime.unlink(global, child)?;
        }
        Ok(true)
    }
}
