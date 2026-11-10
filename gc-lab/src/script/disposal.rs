//! Explicit resource management: DisposeCapability records (ECMA-262 §9.11, from the
//! merged proposal), DisposableStack (§27.3), AsyncDisposableStack (§27.4) and
//! %AsyncIteratorPrototype% [ @@asyncDispose ]. The `using` declarations reuse the same
//! records through a hidden scope object. Design: RESOURCE_MANAGEMENT.md.
use super::{
    objects::{Descriptor, Key},
    promises::{Capability, Cell, Internal},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;

pub(super) const DISPOSE: u64 = 13;
pub(super) const ASYNC_DISPOSE: u64 = 14;

/// How a DisposableResource's [[DisposeMethod]] is called. The spec wraps `adopt`
/// callbacks and the async fallback to @@dispose in closures; they are not observable,
/// so the record keeps the original function and the call shape instead.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Call {
    /// Call(method, value).
    Method,
    /// DisposableStack.prototype.adopt's closure: Call(onDispose, undefined, «value»).
    Adopt,
    /// GetDisposeMethod's async fallback (§9.11.4 step 1.b): Call(@@dispose, value),
    /// then a promise resolved with undefined (or rejected with the throw).
    SyncFallback,
}

/// DisposableResource Record { [[ResourceValue]], [[Hint]], [[DisposeMethod]] }.
#[derive(Clone)]
pub(super) struct Resource {
    value: Value,
    /// Undefined only for `await using x = null/undefined` (needsAwait bookkeeping).
    method: Value,
    async_hint: bool,
    call: Call,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Stack,
    AsyncStack,
    /// The hidden capability of a block holding `using` declarations.
    Scope,
}

/// [[DisposableState]] and [[DisposeCapability]].
#[derive(Clone)]
pub(super) struct DisposeSlot {
    pub owner: Owner,
    pub disposed: bool,
    pub resources: Vec<Resource>,
}
impl DisposeSlot {
    pub fn edges(&self) -> Vec<Id> {
        resource_edges(&self.resources)
    }
}
fn resource_edges(resources: &[Resource]) -> Vec<Id> {
    resources
        .iter()
        .flat_map(|r| [r.value.id(), r.method.id()])
        .flatten()
        .collect()
}

/// The async DisposeResources closure (§9.11.6 with Await), resumed by
/// `Internal::DisposeStep` reactions.
#[derive(Clone)]
pub(super) struct AsyncDispose {
    capability: Capability,
    resources: Vec<Resource>,
    /// The completion so far: Some(value) is a throw completion.
    error: Option<Value>,
    needs_await: bool,
    has_awaited: bool,
}
impl AsyncDispose {
    pub fn edges(&self) -> Vec<Id> {
        let mut ids = resource_edges(&self.resources);
        ids.extend(self.capability.values().iter().filter_map(Value::id));
        ids.extend(self.error.as_ref().and_then(Value::id));
        ids
    }
}

pub(super) fn handles(name: &str) -> bool {
    name == "DisposableStack"
        || name == "AsyncDisposableStack"
        || name.starts_with("DisposableStack.")
        || name.starts_with("AsyncDisposableStack.")
        || name == "%AsyncIteratorPrototype%.asyncDispose"
}

pub(super) fn is_constructor(name: &str) -> bool {
    matches!(name, "DisposableStack" | "AsyncDisposableStack")
}

const STACK_METHODS: &[(&str, usize)] = &[
    ("adopt", 2),
    ("defer", 1),
    ("move", 0),
    ("use", 1),
    ("disposed", 0),
];

impl Engine<'_> {
    pub(super) fn bootstrap_disposal(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let object = self.intrinsics.as_ref().unwrap().object;
        let function = self.intrinsics.as_ref().unwrap().function;
        for (name, dispose, symbol) in [
            ("DisposableStack", "dispose", DISPOSE),
            ("AsyncDisposableStack", "disposeAsync", ASYNC_DISPOSE),
        ] {
            let ctor = self.disposal_function(env, name, name, 0)?;
            objects.insert(name.into(), ctor);
            let proto = self.new_object(Some(object))?;
            self.runtime.link(env, proto)?;
            objects.insert(format!("{name}.prototype"), proto);
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert(name.into(), proto);
            for &(method, length) in STACK_METHODS.iter().chain(std::iter::once(&(dispose, 0))) {
                let full = format!("{name}.prototype.{method}");
                if method == "disposed" {
                    // §27.3.3.4 get DisposableStack.prototype.disposed.
                    let f = self.disposal_function(env, &full, "get disposed", 0)?;
                    self.define(
                        proto,
                        method,
                        Descriptor {
                            get: Some(Value::Object(f)),
                            configurable: Some(true),
                            ..Descriptor::default()
                        },
                    )?;
                    continue;
                }
                let f = self.disposal_function(env, &full, method, length)?;
                self.hidden(proto, method, Value::Object(f))?;
                if method == dispose {
                    // §27.3.3.7 / §27.4.3.7: the initial value of @@dispose /
                    // @@asyncDispose is the same function object as dispose / disposeAsync.
                    self.hidden(proto, Key::Symbol(symbol), Value::Object(f))?;
                }
            }
            self.define(
                proto,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String(name.into())),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
        }
        // §27.1.4.1 %AsyncIteratorPrototype% [ @@asyncDispose ] ( ).
        let async_iterator = self.intrinsics.as_ref().unwrap().collection["AsyncIterator"];
        let f = self.disposal_function(
            env,
            "%AsyncIteratorPrototype%.asyncDispose",
            "[Symbol.asyncDispose]",
            0,
        )?;
        self.hidden(async_iterator, Key::Symbol(ASYNC_DISPOSE), Value::Object(f))?;
        let _ = function;
        Ok(())
    }

    fn disposal_function(&mut self, env: Id, full: &str, name: &str, length: usize) -> Result<Id> {
        let function = self.intrinsics.as_ref().unwrap().function;
        let f = self.intrinsic_function(env, full, Some(function))?;
        for (key, value) in [
            ("length", Value::number(length as f64)),
            ("name", Value::String(name.into())),
        ] {
            self.define(
                f,
                key,
                Descriptor {
                    value: Some(value),
                    writable: Some(false),
                    enumerable: Some(false),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
        }
        self.intrinsics
            .as_mut()
            .unwrap()
            .functions
            .insert(full.into(), f);
        Ok(f)
    }

    /// §27.3.1.1 DisposableStack ( ) / §27.4.1.1 AsyncDisposableStack ( ) as [[Construct]].
    pub(super) fn construct_disposal(&mut self, name: &str, new_target: Value) -> Result<Value> {
        // Step 2: OrdinaryCreateFromConstructor(NewTarget, "%DisposableStack.prototype%").
        let proto = self.prototype_from_constructor(new_target.clone(), |i| i.collection[name])?;
        let id = self.new_object(Some(proto))?;
        let result = Value::Object(id);
        self.protect(&result)?;
        let owner = if name == "DisposableStack" {
            Owner::Stack
        } else {
            Owner::AsyncStack
        };
        self.set_dispose_slot(id, Some(fresh(owner)))?;
        Ok(result)
    }

    fn set_dispose_slot(&mut self, id: Id, slot: Option<DisposeSlot>) -> Result<()> {
        let mut data = self.data(id)?.clone();
        data.dispose = slot;
        self.replace_data(id, data)
    }

    fn dispose_slot(&self, v: &Value, owner: Owner) -> Option<(Id, DisposeSlot)> {
        let Value::Object(id) = v else { return None };
        let slot = self.data(*id).ok()?.dispose.clone()?;
        (slot.owner == owner).then_some((*id, slot))
    }

    pub(super) fn disposal_builtin(
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
        if is_constructor(name) {
            return self.throw_error("TypeError", &format!("Constructor {name} requires 'new'"));
        }
        if name == "%AsyncIteratorPrototype%.asyncDispose" {
            return self.async_iterator_dispose(receiver);
        }
        let (class, method) = name.split_once(".prototype.").unwrap_or((name, ""));
        let asynchronous = class == "AsyncDisposableStack";
        let owner = if asynchronous {
            Owner::AsyncStack
        } else {
            Owner::Stack
        };
        if method == "disposeAsync" {
            return self.dispose_async_method(receiver);
        }
        // RequireInternalSlot(disposableStack, [[DisposableState]]).
        let Some((id, mut slot)) = self.dispose_slot(&receiver, owner) else {
            return self.throw_error(
                "TypeError",
                &format!("{class}.prototype.{method} called on incompatible receiver"),
            );
        };
        match method {
            "disposed" => return Ok(Value::Bool(slot.disposed)),
            // §27.3.3.3 DisposableStack.prototype.dispose: steps 3-5.
            "dispose" => {
                if slot.disposed {
                    return Ok(Value::Undefined);
                }
                let resources = std::mem::take(&mut slot.resources);
                slot.disposed = true;
                self.set_dispose_slot(id, Some(slot))?;
                return match self.dispose_sync(resources, None)? {
                    Some(error) => Err(self.thrown(error)),
                    None => Ok(Value::Undefined),
                };
            }
            _ => {}
        }
        if slot.disposed {
            return self.throw_error(
                "ReferenceError",
                &format!("{class}.prototype.{method} called on a disposed stack"),
            );
        }
        let hint = asynchronous;
        match method {
            // §27.3.3.6 use(value): AddDisposableResource(cap, value, hint).
            "use" => {
                let value = arg(0);
                if let Some(resource) = self.create_resource(value.clone(), hint)? {
                    self.push_resource(id, resource)?;
                }
                Ok(value)
            }
            // §27.3.3.1 adopt(value, onDispose).
            "adopt" => {
                let on_dispose = arg(1);
                if !self.callable(&on_dispose) {
                    return self.throw_error("TypeError", "adopt: onDispose is not a function");
                }
                self.push_resource(
                    id,
                    Resource {
                        value: arg(0),
                        method: on_dispose,
                        async_hint: hint,
                        call: Call::Adopt,
                    },
                )?;
                Ok(arg(0))
            }
            // §27.3.3.2 defer(onDispose).
            "defer" => {
                let on_dispose = arg(0);
                if !self.callable(&on_dispose) {
                    return self.throw_error("TypeError", "defer: onDispose is not a function");
                }
                self.push_resource(
                    id,
                    Resource {
                        value: Value::Undefined,
                        method: on_dispose,
                        async_hint: hint,
                        call: Call::Method,
                    },
                )?;
                Ok(Value::Undefined)
            }
            // §27.3.3.5 move().
            "move" => {
                let proto = self.intrinsics.as_ref().unwrap().collection[class];
                let fresh_id = self.new_object(Some(proto))?;
                let result = Value::Object(fresh_id);
                self.protect(&result)?;
                let moved = DisposeSlot {
                    owner,
                    disposed: false,
                    resources: std::mem::take(&mut slot.resources),
                };
                self.set_dispose_slot(fresh_id, Some(moved))?;
                slot.disposed = true;
                self.set_dispose_slot(id, Some(slot))?;
                Ok(result)
            }
            _ => self.throw_error(
                "TypeError",
                &format!("{name} called on incompatible receiver"),
            ),
        }
    }

    fn push_resource(&mut self, id: Id, resource: Resource) -> Result<()> {
        let mut data = self.data(id)?.clone();
        if let Some(slot) = data.dispose.as_mut() {
            slot.resources.push(resource);
        }
        self.replace_data(id, data)
    }

    /// AddDisposableResource (§9.11.2) without a method: steps 1.a-b, then
    /// CreateDisposableResource (§9.11.3) and GetDisposeMethod (§9.11.4).
    /// `None` means nothing is recorded (sync hint with null/undefined).
    fn create_resource(&mut self, value: Value, async_hint: bool) -> Result<Option<Resource>> {
        if matches!(value, Value::Null | Value::Undefined) {
            if !async_hint {
                return Ok(None);
            }
            return Ok(Some(Resource {
                value: Value::Undefined,
                method: Value::Undefined,
                async_hint,
                call: Call::Method,
            }));
        }
        if !matches!(value, Value::Object(_)) {
            return self.throw_error("TypeError", "using: resource is not an object");
        }
        let mut call = Call::Method;
        let mut method =
            self.dispose_get_method(&value, if async_hint { ASYNC_DISPOSE } else { DISPOSE })?;
        if async_hint && method == Value::Undefined {
            method = self.dispose_get_method(&value, DISPOSE)?;
            call = Call::SyncFallback;
        }
        if method == Value::Undefined {
            return self.throw_error(
                "TypeError",
                if async_hint {
                    "using: resource has no [Symbol.asyncDispose] or [Symbol.dispose] method"
                } else {
                    "using: resource has no [Symbol.dispose] method"
                },
            );
        }
        Ok(Some(Resource {
            value,
            method,
            async_hint,
            call,
        }))
    }

    /// GetMethod (§7.3.10).
    fn dispose_get_method(&mut self, value: &Value, symbol: u64) -> Result<Value> {
        let method = self.get_member(value.clone(), Key::Symbol(symbol))?;
        self.protect(&method)?;
        if matches!(method, Value::Undefined | Value::Null) {
            return Ok(Value::Undefined);
        }
        if !self.callable(&method) {
            return self.throw_error("TypeError", "dispose method is not a function");
        }
        Ok(method)
    }

    fn call_resource(&mut self, r: &Resource) -> Result<Value> {
        match r.call {
            Call::Method | Call::SyncFallback => {
                self.invoke_this(r.method.clone(), r.value.clone(), vec![])
            }
            Call::Adopt => {
                self.invoke_this(r.method.clone(), Value::Undefined, vec![r.value.clone()])
            }
        }
    }

    /// §9.11.6 DisposeResources step 2.c.iii: fold a disposer's throw into the completion.
    fn fold_dispose_error(&mut self, error: Option<Value>, thrown: Value) -> Result<Value> {
        let Some(previous) = error else {
            return Ok(thrown);
        };
        self.protect(&thrown)?;
        self.protect(&previous)?;
        self.error_constructor("SuppressedError", vec![thrown, previous], None)
    }

    /// DisposeResources (§9.11.6) for a capability with no `await` hint. `error` is the
    /// incoming throw completion; the result is the outgoing one.
    pub(super) fn dispose_sync(
        &mut self,
        mut resources: Vec<Resource>,
        mut error: Option<Value>,
    ) -> Result<Option<Value>> {
        for r in &resources {
            self.protect(&r.value)?;
            self.protect(&r.method)?;
        }
        if let Some(e) = &error {
            self.protect(e)?;
        }
        while let Some(r) = resources.pop() {
            if r.method == Value::Undefined {
                continue;
            }
            match self.call_resource(&r) {
                Ok(_) => {}
                Err(ScriptError::Thrown(ex)) => {
                    error = Some(self.fold_dispose_error(error, ex.value)?);
                }
                Err(other) => return Err(other),
            }
        }
        Ok(error)
    }

    /// The async DisposeResources as a promise: fulfilled with undefined, or rejected
    /// with the final throw completion.
    pub(super) fn dispose_async(
        &mut self,
        resources: Vec<Resource>,
        error: Option<Value>,
    ) -> Result<Value> {
        let promise_ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let capability = self.capability(promise_ctor)?;
        for v in capability.values() {
            self.protect(&v)?;
        }
        let result = capability.promise.clone();
        let job = self.new_object(None)?;
        self.protect(&Value::Object(job))?;
        let state = AsyncDispose {
            capability,
            resources,
            error,
            needs_await: false,
            has_awaited: false,
        };
        self.dispose_job_save(job, state.clone())?;
        self.dispose_run(job, state)?;
        Ok(result)
    }

    fn dispose_job_save(&mut self, job: Id, state: AsyncDispose) -> Result<()> {
        let mut data = self.data(job)?.clone();
        data.cell = Some(Cell::Dispose(state));
        self.replace_data(job, data)
    }

    /// The reaction entry point: an awaited disposal result settled.
    pub(super) fn dispose_step(&mut self, job: Id, value: Value, reject: bool) -> Result<Value> {
        self.protect(&value)?;
        let Some(Cell::Dispose(mut state)) = self.data(job)?.cell.clone() else {
            return Err(ScriptError::Runtime("missing DisposeResources state"));
        };
        if reject {
            state.error = Some(self.fold_dispose_error(state.error.take(), value)?);
        }
        self.dispose_run(job, state)?;
        Ok(Value::Undefined)
    }

    /// Await (§27.7.5.3) inside the closure: save the state and resume on settlement.
    fn dispose_await(&mut self, job: Id, state: AsyncDispose, value: Value) -> Result<()> {
        let promise = self.promise_resolve(
            Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]),
            value,
        )?;
        self.protect(&promise)?;
        self.dispose_job_save(job, state)?;
        let yes = self.internal_function(Internal::DisposeStep { job, reject: false }, 1)?;
        let no = self.internal_function(Internal::DisposeStep { job, reject: true }, 1)?;
        let Value::Object(p) = promise else {
            unreachable!()
        };
        self.attach_reaction(p, Some(yes), Some(no), None)
    }

    /// §9.11.6 DisposeResources steps 2-6 with the needsAwait / hasAwaited bookkeeping.
    fn dispose_run(&mut self, job: Id, mut state: AsyncDispose) -> Result<()> {
        for r in &state.resources {
            self.protect(&r.value)?;
            self.protect(&r.method)?;
        }
        while let Some(r) = state.resources.pop() {
            // Step 2.b: a sync resource after an un-awaited null `await using`.
            if !r.async_hint && state.needs_await && !state.has_awaited {
                state.resources.push(r);
                state.needs_await = false;
                return self.dispose_await(job, state, Value::Undefined);
            }
            if r.method == Value::Undefined {
                // Step 2.d: `await using x = null` / undefined.
                state.needs_await = true;
                continue;
            }
            match self.call_resource(&r) {
                Ok(result) if r.async_hint => {
                    state.has_awaited = true;
                    let awaited = if r.call == Call::SyncFallback {
                        Value::Undefined
                    } else {
                        result
                    };
                    return self.dispose_await(job, state, awaited);
                }
                Ok(_) => {}
                Err(ScriptError::Thrown(ex)) if r.async_hint => {
                    state.has_awaited = true;
                    if r.call == Call::SyncFallback {
                        // The closure returns a rejected promise; Await rethrows it.
                        let promise_ctor =
                            Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
                        let cap = self.capability(promise_ctor)?;
                        self.invoke(cap.reject.clone(), vec![ex.value])?;
                        return self.dispose_await(job, state, cap.promise);
                    }
                    state.error = Some(self.fold_dispose_error(state.error.take(), ex.value)?);
                }
                Err(ScriptError::Thrown(ex)) => {
                    state.error = Some(self.fold_dispose_error(state.error.take(), ex.value)?);
                }
                Err(other) => return Err(other),
            }
        }
        // Step 3.
        if state.needs_await && !state.has_awaited {
            state.needs_await = false;
            return self.dispose_await(job, state, Value::Undefined);
        }
        let cap = state.capability.clone();
        match state.error {
            Some(e) => self.invoke(cap.reject, vec![e])?,
            None => self.invoke(cap.resolve, vec![Value::Undefined])?,
        };
        Ok(())
    }

    /// §27.4.3.3 AsyncDisposableStack.prototype.disposeAsync ( ).
    fn dispose_async_method(&mut self, receiver: Value) -> Result<Value> {
        let promise_ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let Some((id, mut slot)) = self.dispose_slot(&receiver, Owner::AsyncStack) else {
            // Step 3: reject with a TypeError.
            let cap = self.capability(promise_ctor)?;
            let error = self.error_value(
                "TypeError",
                "AsyncDisposableStack.prototype.disposeAsync called on incompatible receiver",
            )?;
            self.invoke(cap.reject, vec![error])?;
            return Ok(cap.promise);
        };
        if slot.disposed {
            let cap = self.capability(promise_ctor)?;
            self.invoke(cap.resolve, vec![Value::Undefined])?;
            return Ok(cap.promise);
        }
        let resources = std::mem::take(&mut slot.resources);
        slot.disposed = true;
        self.set_dispose_slot(id, Some(slot))?;
        self.dispose_async(resources, None)
    }

    /// §27.1.4.1 %AsyncIteratorPrototype% [ @@asyncDispose ] ( ).
    fn async_iterator_dispose(&mut self, receiver: Value) -> Result<Value> {
        let promise_ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let cap = self.capability(promise_ctor.clone())?;
        for v in cap.values() {
            self.protect(&v)?;
        }
        let outcome = (|| -> Result<()> {
            let method = self.get_member(receiver.clone(), "return")?;
            self.protect(&method)?;
            if matches!(method, Value::Undefined | Value::Null) {
                self.invoke(cap.resolve.clone(), vec![Value::Undefined])?;
                return Ok(());
            }
            if !self.callable(&method) {
                return self.throw_error("TypeError", "return is not a function");
            }
            let result = self.invoke_this(method, receiver.clone(), vec![])?;
            self.protect(&result)?;
            let wrapper = self.promise_resolve(promise_ctor.clone(), result)?;
            self.protect(&wrapper)?;
            let unwrap = self.internal_function(
                Internal::Thunk {
                    value: Value::Undefined,
                    reject: false,
                },
                1,
            )?;
            let Value::Object(p) = wrapper else {
                unreachable!()
            };
            self.attach_reaction(p, Some(unwrap), None, Some(cap.clone()))
        })();
        match outcome {
            Ok(()) => {}
            Err(ScriptError::Thrown(ex)) => {
                self.invoke(cap.reject.clone(), vec![ex.value])?;
            }
            Err(other) => return Err(other),
        }
        Ok(cap.promise)
    }

    /// AddDisposableResource for a `using` / `await using` binding into the scope's
    /// hidden capability (§14.3.1.2 BindingEvaluation step 5 / §9.11.2).
    pub(super) fn add_using_resource(
        &mut self,
        scope: &Value,
        value: Value,
        async_hint: bool,
    ) -> Result<()> {
        let Value::Object(id) = scope else {
            return Err(ScriptError::Runtime("missing using scope"));
        };
        if let Some(resource) = self.create_resource(value, async_hint)? {
            self.push_resource(*id, resource)?;
        }
        Ok(())
    }

    /// Run `f` in `env` with a hidden dispose capability bound as `%dispose` when
    /// `body` declares `using` bindings directly (§14.2.2 Block, §14.7.4.2 for heads,
    /// FunctionBody via FunctionDeclarationInstantiation's DisposeCapability).
    pub(super) fn using_scope<T>(
        &mut self,
        body: &[super::parser::Stmt],
        env: Id,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if !super::parser::has_direct_using(body) {
            return f(self);
        }
        let scope = self.new_using_scope()?;
        self.initialize(env, "%dispose", scope.clone())?;
        // DisposeResources runs after the body's `return`: keep its calls ordinary.
        let outcome = self.non_tail_scope(f);
        self.dispose_scope_outcome(&scope, outcome)
    }
    /// A fresh hidden capability for a block with `using` declarations.
    pub(super) fn new_using_scope(&mut self) -> Result<Value> {
        let id = self.new_object(None)?;
        let scope = Value::Object(id);
        self.protect(&scope)?;
        self.set_dispose_slot(id, Some(fresh(Owner::Scope)))?;
        Ok(scope)
    }

    /// Take the scope's resources (DisposeResources clears the stack, step 4).
    pub(super) fn take_using_resources(&mut self, scope: &Value) -> Result<Vec<Resource>> {
        let Value::Object(id) = scope else {
            return Ok(vec![]);
        };
        let mut data = self.data(*id)?.clone();
        let resources = data
            .dispose
            .as_mut()
            .map(|s| std::mem::take(&mut s.resources))
            .unwrap_or_default();
        for r in &resources {
            self.protect(&r.value)?;
            self.protect(&r.method)?;
        }
        self.replace_data(*id, data)?;
        Ok(resources)
    }

    /// Run a sync scope's disposal on a body outcome (§14.2.2 Block evaluation step 5:
    /// DisposeResources(blockEnv.[[DisposeCapability]], blockValue)). Only throw
    /// completions and disposal errors change the outcome.
    pub(super) fn dispose_scope_outcome<T>(
        &mut self,
        scope: &Value,
        outcome: Result<T>,
    ) -> Result<T> {
        let resources = self.take_using_resources(scope)?;
        let (error, outcome) = match outcome {
            Err(ScriptError::Thrown(ex)) => (Some(ex.value.clone()), Err(ScriptError::Thrown(ex))),
            Err(e) if e.is_terminal() => return Err(e),
            other => (None, other),
        };
        let had_error = error.is_some();
        match self.dispose_sync(resources, error.clone())? {
            Some(final_error) => {
                if had_error && error.as_ref() == Some(&final_error) {
                    outcome
                } else {
                    Err(self.thrown(final_error))
                }
            }
            None => outcome,
        }
    }
}

fn fresh(owner: Owner) -> DisposeSlot {
    DisposeSlot {
        owner,
        disposed: false,
        resources: vec![],
    }
}
