//! WeakRef (§26.1), FinalizationRegistry (§26.2) and the weak-target processing model
//! (§9.10: liveness, AddToKeptObjects, ClearKeptObjects, CleanupFinalizationRegistry,
//! CanBeHeldWeakly). WeakMap/WeakSet tables live in collections.rs. Design: WEAK_REFS.md.
use super::{
    objects::{Data, Descriptor, Key},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id, IdSet};
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;

/// A FinalizationRegistry cell (§26.2: Record { [[WeakRefTarget]], [[HeldValue]],
/// [[UnregisterToken]] }). `Undefined` stands for ~empty~ in target and token.
#[derive(Clone)]
pub(super) struct RegistryCell {
    pub target: Value,
    pub holdings: Value,
    pub token: Value,
}

#[derive(Clone)]
pub(super) enum WeakSlot {
    /// [[WeakRefTarget]]; `Undefined` once the target was collected.
    Ref(Value),
    /// [[CleanupCallback]] and [[Cells]].
    Registry {
        cleanup: Value,
        cells: Vec<RegistryCell>,
    },
}
impl WeakSlot {
    /// Strong edges: the cleanup callback and every held value.
    pub fn edges(&self) -> Vec<Id> {
        match self {
            Self::Ref(_) => vec![],
            Self::Registry { cleanup, cells } => cleanup
                .id()
                .into_iter()
                .chain(cells.iter().filter_map(|c| c.holdings.id()))
                .collect(),
        }
    }
    pub fn weak_edges(&self, weak: &mut Vec<Id>) {
        match self {
            Self::Ref(target) => weak.extend(target.id()),
            Self::Registry { cells, .. } => {
                for c in cells {
                    weak.extend(c.target.id());
                    weak.extend(c.token.id());
                }
            }
        }
    }
}

/// Engine-wide weak bookkeeping.
#[derive(Default)]
pub(super) struct WeakState {
    /// Objects whose Data holds weak slots; scanned after each collection.
    pub holders: IdSet,
    /// [[KeptAlive]] (AddToKeptObjects), rooted until ClearKeptObjects.
    pub kept_alive: Vec<ExternalRoot>,
    /// Registries with emptied cells (HostEnqueueFinalizationRegistryCleanupJob).
    pub pending_cleanup: Vec<ExternalRoot>,
}

fn data_mut<'a>(objects: &'a mut crate::dual::IdMap<Object>, id: Id) -> Option<&'a mut Data> {
    match objects.get_mut(&id) {
        Some(
            Object::Ordinary(d)
            | Object::Builtin { data: d, .. }
            | Object::Bound { data: d, .. }
            | Object::Closure { data: d, .. },
        ) => Some(d),
        _ => None,
    }
}

impl Engine<'_> {
    /// CanBeHeldWeakly (§9.14): objects, and symbols not in the GlobalSymbolRegistry.
    pub(super) fn can_be_held_weakly(&self, v: &Value) -> bool {
        match v {
            Value::Object(_) => true,
            Value::Symbol(id) => !self.symbol_registry.values().any(|s| s == id),
            _ => false,
        }
    }
    /// AddToKeptObjects (§9.10): the target stays live until ClearKeptObjects.
    fn add_to_kept_objects(&mut self, v: &Value) -> Result<()> {
        if let Some(id) = v.id() {
            let root = self.runtime.root(id)?;
            self.weak.kept_alive.push(root);
        }
        Ok(())
    }
    /// ClearKeptObjects (§9.10); HTML runs it at the end of a microtask checkpoint.
    pub fn clear_kept_objects(&mut self) {
        for root in std::mem::take(&mut self.weak.kept_alive) {
            let _ = self.runtime.unroot(root);
        }
    }
    /// Keep the runtime's weak lists equal to `Data`'s weak column (called by replace_data).
    pub(super) fn sync_weak(&mut self, id: Id, data: &Data) -> Result<()> {
        let mut weak = vec![];
        let mut ephemerons = vec![];
        if let Some(t) = data.collection.as_ref() {
            t.weak_edges(&mut weak, &mut ephemerons);
        }
        if let Some(w) = data.weak.as_ref() {
            w.weak_edges(&mut weak);
        }
        let holds = data.collection.as_ref().is_some_and(|t| t.weak) || data.weak.is_some();
        if !holds && !self.weak.holders.contains(&id) {
            return Ok(());
        }
        self.runtime.set_weak(id, weak, ephemerons)?;
        if holds {
            self.weak.holders.insert(id);
        }
        Ok(())
    }
    /// Add a new object key to a weak table in place, keeping the runtime's weak column
    /// equal to `Table::weak_edges` without cloning the table.
    pub(super) fn weak_append(&mut self, id: Id, key: Value, value: Value) -> Result<()> {
        let k = key.id().expect("object key");
        let set = self.data(id)?.collection.as_ref().is_some_and(|t| t.set);
        let v = if set { None } else { value.id() };
        self.runtime.add_weak(id, k, v)?;
        self.weak.holders.insert(id);
        let data = data_mut(&mut self.objects, id).expect("weak table holder");
        data.collection.as_mut().unwrap().write(key, value);
        Ok(())
    }
    /// Post-collection step of §9.10 Execution: weak entries whose target died become
    /// ~empty~; registries with newly emptied cells get a cleanup job.
    pub(super) fn scrub_weak(&mut self) {
        let runtime = &self.runtime;
        let dead = |id: Id| runtime.value(id).is_err();
        self.weak.holders.retain(|id| !dead(*id));
        let mut enqueue = vec![];
        for &holder in &self.weak.holders {
            let Some(data) = data_mut(&mut self.objects, holder) else {
                continue;
            };
            if let Some(t) = data.collection.as_mut().filter(|t| t.weak) {
                t.scrub(dead);
            }
            match data.weak.as_mut() {
                Some(WeakSlot::Ref(target)) if target.id().is_some_and(dead) => {
                    *target = Value::Undefined;
                }
                Some(WeakSlot::Registry { cells, .. }) => {
                    let mut emptied = false;
                    for c in cells.iter_mut() {
                        if c.target.id().is_some_and(dead) {
                            c.target = Value::Undefined;
                            emptied = true;
                        }
                        if c.token.id().is_some_and(dead) {
                            c.token = Value::Undefined;
                        }
                    }
                    if emptied {
                        enqueue.push(holder);
                    }
                }
                _ => {}
            }
        }
        for registry in enqueue {
            let queued = self
                .weak
                .pending_cleanup
                .iter()
                .any(|r| self.runtime.resolve(*r) == Ok(registry));
            if queued {
                continue;
            }
            if let Ok(root) = self.runtime.root(registry) {
                self.weak.pending_cleanup.push(root);
            }
        }
    }
    pub fn pending_finalization_cleanups(&self) -> usize {
        self.weak.pending_cleanup.len()
    }
    pub(super) fn cancel_weak_jobs(&mut self) {
        for root in std::mem::take(&mut self.weak.pending_cleanup) {
            let _ = self.runtime.unroot(root);
        }
        self.clear_kept_objects();
    }
    /// The host task for queued registries: CleanupFinalizationRegistry (§9.13) each.
    /// Every registry runs even if an earlier callback throws; the first error is returned.
    pub fn run_finalization_cleanup(&mut self) -> Result<()> {
        let mut first = Ok(());
        for root in std::mem::take(&mut self.weak.pending_cleanup) {
            let Ok(registry) = self.runtime.resolve(root) else {
                continue;
            };
            self.steps_for_external_event();
            let result = self.frame(|e| {
                e.protect(&Value::Object(registry))?;
                e.runtime.unroot(root)?;
                e.cleanup_finalization_registry(registry)
            });
            if result.is_err() {
                // The cells left after an abrupt callback stay pending (WEAK_REFS.md):
                // queue the registry again so a later cleanup task reaches them. Each
                // call removes its cell first, so a callback that always throws still
                // ends.
                let empty_left = matches!(
                    self.data(registry).map(|d| &d.weak),
                    Ok(Some(WeakSlot::Registry { cells, .. }))
                        if cells.iter().any(|c| c.target == Value::Undefined)
                );
                if empty_left {
                    if let Ok(root) = self.runtime.root(registry) {
                        self.weak.pending_cleanup.push(root);
                    }
                }
            }
            if first.is_ok() {
                first = result;
            }
        }
        first
    }
    /// CleanupFinalizationRegistry (§9.13): while a cell has an ~empty~ target, remove it
    /// and call the cleanup callback with its held value.
    fn cleanup_finalization_registry(&mut self, registry: Id) -> Result<()> {
        loop {
            let mut data = self.data(registry)?.clone();
            let Some(WeakSlot::Registry { cleanup, cells }) = data.weak.as_mut() else {
                return Ok(());
            };
            let Some(at) = cells.iter().position(|c| c.target == Value::Undefined) else {
                return Ok(());
            };
            let cell = cells.remove(at);
            let callback = cleanup.clone();
            self.frame(|e| {
                e.protect(&cell.holdings)?;
                e.replace_data(registry, data)?;
                e.invoke_this(callback, Value::Undefined, vec![cell.holdings.clone()])
                    .map(|_| ())
            })?;
        }
    }

    pub(super) fn bootstrap_weak(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let object = self.intrinsics.as_ref().unwrap().object;
        let function = self.intrinsics.as_ref().unwrap().function;
        let table: &[(&str, &[(&str, usize)])] = &[
            (
                "WeakMap",
                &[
                    ("delete", 1),
                    ("get", 1),
                    ("has", 1),
                    ("set", 2),
                    ("getOrInsert", 2),
                    ("getOrInsertComputed", 2),
                ],
            ),
            ("WeakSet", &[("add", 1), ("delete", 1), ("has", 1)]),
            ("WeakRef", &[("deref", 0)]),
            (
                "FinalizationRegistry",
                &[("register", 2), ("unregister", 1)],
            ),
        ];
        for (name, methods) in table {
            let proto = self.new_object(Some(object))?;
            self.runtime.link(env, proto)?;
            objects.insert(format!("{name}.prototype"), proto);
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert((*name).into(), proto);
            let length = if matches!(*name, "WeakMap" | "WeakSet") {
                0
            } else {
                1
            };
            self.define(
                objects[*name],
                "length",
                Descriptor {
                    value: Some(Value::number(length as f64)),
                    ..Descriptor::default()
                },
            )?;
            // §24.3.3.x / §26.x.3.x [ %Symbol.toStringTag% ]: configurable only.
            self.define(
                proto,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String((*name).into())),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
            for (method, length) in *methods {
                let f = self.intrinsic_function(
                    env,
                    &format!("{name}.prototype.{method}"),
                    Some(function),
                )?;
                self.define(
                    f,
                    "length",
                    Descriptor {
                        value: Some(Value::number(*length as f64)),
                        ..Descriptor::default()
                    },
                )?;
                self.hidden(proto, *method, Value::Object(f))?;
            }
        }
        Ok(())
    }

    /// [[Construct]] of WeakRef and FinalizationRegistry (WeakMap/WeakSet go through
    /// construct_collection).
    pub(super) fn construct_weak(
        &mut self,
        name: &str,
        new_target: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = args.first().cloned().unwrap_or(Value::Undefined);
        let slot = if name == "WeakRef" {
            // §26.1.1.1 WeakRef(target), step 2.
            if !self.can_be_held_weakly(&arg) {
                return self.throw_error("TypeError", "WeakRef: invalid target");
            }
            WeakSlot::Ref(arg.clone())
        } else {
            // §26.2.1.1 FinalizationRegistry(cleanupCallback), step 2.
            if !self.callable(&arg) {
                return self.throw_error(
                    "TypeError",
                    "FinalizationRegistry: cleanup must be callable",
                );
            }
            WeakSlot::Registry {
                cleanup: arg.clone(),
                cells: vec![],
            }
        };
        self.protect(&arg)?;
        // OrdinaryCreateFromConstructor(NewTarget, "%WeakRef.prototype%" / ...).
        let proto = self.prototype_from_constructor(new_target, |i| i.collection[name])?;
        let id = self.new_object(Some(proto))?;
        let result = Value::Object(id);
        self.protect(&result)?;
        if name == "WeakRef" {
            self.add_to_kept_objects(&arg)?;
        }
        let mut data = self.data(id)?.clone();
        data.weak = Some(slot);
        self.replace_data(id, data)?;
        Ok(result)
    }

    pub(super) fn weak_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if matches!(name, "WeakRef" | "FinalizationRegistry") {
            return self.throw_error("TypeError", &format!("Constructor {name} requires 'new'"));
        }
        let arg = args.first().cloned().unwrap_or(Value::Undefined);
        let slot = match &receiver {
            Value::Object(id) => self.data(*id).ok().and_then(|d| d.weak.clone()),
            _ => None,
        };
        let Value::Object(id) = receiver else {
            return self.throw_error(
                "TypeError",
                &format!("{name} called on incompatible receiver"),
            );
        };
        match (name, slot) {
            // §26.1.3.2 WeakRef.prototype.deref: WeakRefDeref (§26.1.4.1).
            ("WeakRef.prototype.deref", Some(WeakSlot::Ref(target))) => {
                if target != Value::Undefined {
                    self.add_to_kept_objects(&target)?;
                }
                Ok(target)
            }
            // §26.2.3.2 FinalizationRegistry.prototype.register(target, heldValue, unregisterToken)
            ("FinalizationRegistry.prototype.register", Some(WeakSlot::Registry { .. })) => {
                let held = args.get(1).cloned().unwrap_or(Value::Undefined);
                let token = args.get(2).cloned().unwrap_or(Value::Undefined);
                if !self.can_be_held_weakly(&arg) {
                    return self.throw_error("TypeError", "register: invalid target");
                }
                if arg.same_value(&held) {
                    return self.throw_error(
                        "TypeError",
                        "register: target and holdings must not be same",
                    );
                }
                if !self.can_be_held_weakly(&token) && token != Value::Undefined {
                    return self.throw_error("TypeError", "register: invalid unregister token");
                }
                let mut data = self.data(id)?.clone();
                if let Some(WeakSlot::Registry { cells, .. }) = data.weak.as_mut() {
                    cells.push(RegistryCell {
                        target: arg,
                        holdings: held,
                        token,
                    });
                }
                self.replace_data(id, data)?;
                Ok(Value::Undefined)
            }
            // §26.2.3.3 FinalizationRegistry.prototype.unregister(unregisterToken)
            ("FinalizationRegistry.prototype.unregister", Some(WeakSlot::Registry { .. })) => {
                if !self.can_be_held_weakly(&arg) {
                    return self.throw_error("TypeError", "unregister: invalid unregister token");
                }
                let mut data = self.data(id)?.clone();
                let mut removed = false;
                if let Some(WeakSlot::Registry { cells, .. }) = data.weak.as_mut() {
                    let before = cells.len();
                    cells.retain(|c| c.token == Value::Undefined || !c.token.same_value(&arg));
                    removed = cells.len() != before;
                }
                if removed {
                    self.replace_data(id, data)?;
                }
                Ok(Value::Bool(removed))
            }
            _ => self.throw_error(
                "TypeError",
                &format!("{name} called on incompatible receiver"),
            ),
        }
    }
}
