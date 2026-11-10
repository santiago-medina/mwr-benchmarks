//! Promise state and jobs are ordinary traced records; queue entries are explicit roots.
use super::{
    objects::{Data, Descriptor, Key},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id};
use std::collections::VecDeque;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct Capability {
    pub promise: Value,
    pub resolve: Value,
    pub reject: Value,
}
impl Capability {
    pub(super) fn values(&self) -> Vec<Value> {
        vec![
            self.promise.clone(),
            self.resolve.clone(),
            self.reject.clone(),
        ]
    }
}
#[derive(Clone)]
pub(super) struct Reaction {
    pub fulfilled: Option<Value>,
    pub rejected: Option<Value>,
    pub capability: Option<Capability>,
}
#[derive(Clone)]
pub(super) enum State {
    Pending,
    Fulfilled(Value),
    Rejected(Value),
}
#[derive(Clone)]
pub(super) struct PromiseState {
    pub state: State,
    pub reactions: Vec<Reaction>,
    pub handled: bool,
}
impl PromiseState {
    pub fn edges(&self) -> Vec<Id> {
        let mut values = vec![];
        match &self.state {
            State::Fulfilled(v) | State::Rejected(v) => values.push(v.clone()),
            State::Pending => {}
        }
        for r in &self.reactions {
            values.extend(r.fulfilled.clone());
            values.extend(r.rejected.clone());
            if let Some(cap) = &r.capability {
                values.extend(cap.values());
            }
        }
        values.iter().filter_map(Value::id).collect()
    }
}
#[derive(Clone)]
pub(super) enum Internal {
    AggregateElement {
        group: Id,
        guard: Id,
        index: u64,
        reject: bool,
    },
    ModuleSettled {
        index: usize,
        reject: bool,
    },
    /// A step of ContinueDynamicImport (§13.3.10.2) for registry slot `slot`.
    DynamicImport {
        slot: usize,
        step: super::modules::DynamicStep,
    },
    AsyncResume {
        activation: Id,
        reject: bool,
    },
    Resolve {
        cell: Id,
        reject: bool,
    },
    Capability {
        cell: Id,
    },
    Finally {
        handler: Value,
        constructor: Value,
        reject: bool,
    },
    Thunk {
        value: Value,
        reject: bool,
    },
    IterResult {
        done: bool,
    },
    AsyncGeneratorReturn {
        generator: Id,
        reject: bool,
    },
    FromAsyncStep {
        job: Id,
        reject: bool,
    },
    CloseSyncIterator {
        iterator: Value,
    },
    DisposeStep {
        job: Id,
        reject: bool,
    },
}
impl Internal {
    pub fn edges(&self) -> Vec<Id> {
        match self {
            Self::AggregateElement { group, guard, .. } => vec![*group, *guard],
            Self::ModuleSettled { .. } | Self::DynamicImport { .. } => vec![],
            Self::AsyncResume { activation, .. } => vec![*activation],
            Self::Resolve { cell, .. } | Self::Capability { cell } => vec![*cell],
            Self::Finally {
                handler,
                constructor,
                ..
            } => [handler, constructor]
                .into_iter()
                .filter_map(Value::id)
                .collect(),
            Self::Thunk { value, .. } => value.id().into_iter().collect(),
            Self::IterResult { .. } => vec![],
            Self::AsyncGeneratorReturn { generator, .. } => vec![*generator],
            Self::FromAsyncStep { job, .. } | Self::DisposeStep { job, .. } => vec![*job],
            Self::CloseSyncIterator { iterator } => iterator.id().into_iter().collect(),
        }
    }
}
#[derive(Clone)]
pub(super) enum Cell {
    Aggregate(super::promise_combinators::Aggregate),
    Called(bool),
    Resolving { promise: Id, resolved: bool },
    Capability { resolve: Value, reject: Value },
    FromAsync(super::from_async::FromAsync),
    Dispose(super::disposal::AsyncDispose),
}
impl Cell {
    pub fn edges(&self) -> Vec<Id> {
        match self {
            Self::Aggregate(state) => state.edges(),
            Self::Called(_) => vec![],
            Self::FromAsync(state) => state.edges(),
            Self::Dispose(state) => state.edges(),
            Self::Resolving { promise, .. } => vec![*promise],
            Self::Capability { resolve, reject } => [resolve, reject]
                .into_iter()
                .filter_map(Value::id)
                .collect(),
        }
    }
}
#[derive(Clone)]
pub(super) enum Job {
    Reaction {
        handler: Option<Value>,
        argument: Value,
        reject: bool,
        capability: Option<Capability>,
    },
    Thenable {
        thenable: Value,
        then: Value,
        resolve: Value,
        reject: Value,
    },
    Callback(Value),
}
impl Job {
    pub fn edges(&self) -> Vec<Id> {
        let values = match self {
            Self::Reaction {
                handler,
                argument,
                capability,
                ..
            } => {
                let mut values = vec![argument.clone()];
                values.extend(handler.clone());
                if let Some(cap) = capability {
                    values.extend(cap.values());
                }
                values
            }
            Self::Thenable {
                thenable,
                then,
                resolve,
                reject,
            } => vec![
                thenable.clone(),
                then.clone(),
                resolve.clone(),
                reject.clone(),
            ],
            Self::Callback(f) => vec![f.clone()],
        };
        values.iter().filter_map(Value::id).collect()
    }
}
pub(super) struct QueuedJob {
    pub serial: u64,
    pub root: ExternalRoot,
}
impl Engine<'_> {
    pub fn set_microtask_queue_limit(&mut self, limit: usize) {
        self.microtask_queue_limit = limit;
    }
    pub fn pending_microtasks(&self) -> usize {
        self.jobs.len()
    }
    pub fn cancel_microtasks(&mut self) {
        self.cancel_fetches();
        let jobs = std::mem::take(&mut self.jobs);
        for job in jobs {
            self.runtime.unroot(job.root).expect("owned microtask root");
        }
    }
    pub fn microtask_checkpoint(&mut self, limit: usize) -> Result<usize> {
        if self.call_depth != 0 || self.native_depth != 0 || self.checkpoint_running {
            return Err(ScriptError::Runtime("reentrant microtask checkpoint"));
        }
        self.checkpoint_running = true;
        let result = (|| {
            let mut count = 0;
            while !self.jobs.is_empty() {
                if count >= limit {
                    return Err(ScriptError::Runtime("microtask checkpoint limit"));
                }
                self.run_one_microtask(None)?;
                count += 1;
            }
            Ok(count)
        })();
        self.checkpoint_running = false;
        if result.is_err() {
            self.cancel_microtasks();
        }
        // HTML "perform a microtask checkpoint": ClearKeptObjects() after the queue drains.
        self.clear_kept_objects();
        self.maybe_collect();
        result
    }
    /// Run exactly the host-scheduled job. Stale/cancelled tokens are harmless.
    pub fn run_one_microtask(&mut self, serial: Option<u64>) -> Result<bool> {
        if self.call_depth != 0 || self.native_depth != 0 {
            return Err(ScriptError::Runtime("microtask execution during script"));
        }
        let Some(front) = self.jobs.front() else {
            return Ok(false);
        };
        if serial.is_some_and(|s| s != front.serial) {
            return Ok(false);
        }
        let root = front.root;
        let id = self.runtime.resolve(root)?;
        self.steps_for_external_event();
        let result = self.frame(|e| {
            e.protect(&Value::Object(id))?;
            let queued = e.jobs.pop_front().unwrap();
            e.runtime.unroot(queued.root)?;
            let job = e
                .data(id)?
                .job
                .clone()
                .ok_or(ScriptError::Runtime("missing job record"))?;
            e.run_job(job)
        });
        let result = match result {
            Err(ScriptError::Thrown(e)) => {
                self.microtask_errors
                    .push(format!("{:?}: {}", e.name, e.message));
                Ok(())
            }
            other => other,
        };
        if result.is_err() {
            self.cancel_microtasks();
        }
        self.maybe_collect();
        result.map(|_| true)
    }
    pub(super) fn enqueue_job(&mut self, job: Job) -> Result<()> {
        if self.jobs.len() >= self.microtask_queue_limit {
            return Err(ScriptError::Runtime("pending microtask limit"));
        }
        let serial = self.next_job;
        let next_job = serial
            .checked_add(1)
            .ok_or(ScriptError::Runtime("job serial limit"))?;
        let id = self.new_object(None)?;
        let mut data = Data::new(None);
        data.job = Some(job);
        self.replace_data(id, data)?;
        let root = self.runtime.root(id)?;
        self.next_job = next_job;
        self.jobs.push_back(QueuedJob { serial, root });
        if let Some(adapter) = self.adapter.as_mut() {
            if let Err(error) = adapter.queue_microtask(serial) {
                let queued = self.jobs.pop_back().unwrap();
                self.runtime.unroot(queued.root)?;
                self.adapter_error = Some(error);
                return Err(ScriptError::Runtime("host microtask scheduling failed"));
            }
        }
        Ok(())
    }
    pub(super) fn run_job(&mut self, job: Job) -> Result<()> {
        match job {
            Job::Reaction {
                handler,
                argument,
                reject,
                capability,
            } => {
                let outcome = if let Some(handler) = handler {
                    self.invoke(handler, vec![argument])
                } else if reject {
                    Err(self.thrown(argument))
                } else {
                    Ok(argument)
                };
                if let Some(capability) = capability {
                    match outcome {
                        Ok(v) => {
                            self.protect(&v)?;
                            self.invoke(capability.resolve, vec![v])?;
                        }
                        Err(ScriptError::Thrown(e)) => {
                            self.protect(&e.value)?;
                            self.invoke(capability.reject, vec![e.value])?;
                        }
                        Err(e) => return Err(e),
                    }
                } else {
                    outcome?;
                }
                Ok(())
            }
            Job::Thenable {
                thenable,
                then,
                resolve,
                reject,
            } => match self.invoke_this(then, thenable, vec![resolve, reject.clone()]) {
                Ok(_) => Ok(()),
                Err(ScriptError::Thrown(e)) => {
                    self.protect(&e.value)?;
                    self.invoke(reject, vec![e.value])?;
                    Ok(())
                }
                Err(e) => Err(e),
            },
            Job::Callback(callback) => match self.invoke(callback, vec![]) {
                Ok(_) => Ok(()),
                Err(ScriptError::Thrown(e)) => {
                    self.microtask_errors
                        .push(format!("{:?}: {}", e.name, e.message));
                    Ok(())
                }
                Err(e) => Err(e),
            },
        }
    }
    pub fn take_microtask_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.microtask_errors)
    }
    pub fn async_test_state(&self) -> (usize, Option<&str>) {
        (self.done_count, self.done_error.as_deref())
    }
    pub fn unhandled_test_failure(&self) -> Option<String> {
        self.unhandled
            .values()
            .find(|(failure, _)| *failure)
            .map(|(_, message)| message.clone())
    }
    pub fn unhandled_rejection_count(&self) -> usize {
        self.unhandled.len()
    }
    pub fn take_unhandled_rejections(&mut self) -> Vec<String> {
        self.unhandled
            .drain()
            .map(|(_, (_, message))| message)
            .collect()
    }
    pub(super) fn internal_function(&mut self, action: Internal, length: usize) -> Result<Value> {
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().function))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.internal = Some(action);
        self.objects.insert(
            id,
            Object::Builtin {
                name: "%PromiseInternal%".into(),
                data: self.data(id)?.clone(),
            },
        );
        self.replace_data(id, data)?;
        self.define(
            id,
            "length",
            Descriptor {
                value: Some(Value::number(length as f64)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        self.define(
            id,
            "name",
            Descriptor {
                value: Some(Value::String(String::new().into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(Value::Object(id))
    }
    fn resolving_functions(&mut self, promise: Id) -> Result<(Value, Value)> {
        let cell = self.new_object(None)?;
        self.protect(&Value::Object(cell))?;
        let mut data = self.data(cell)?.clone();
        data.cell = Some(Cell::Resolving {
            promise,
            resolved: false,
        });
        self.replace_data(cell, data)?;
        let resolve = self.internal_function(
            Internal::Resolve {
                cell,
                reject: false,
            },
            1,
        )?;
        let reject = self.internal_function(Internal::Resolve { cell, reject: true }, 1)?;
        Ok((resolve, reject))
    }
    pub(super) fn promise_constructor_target(
        &mut self,
        new_target: Value,
        executor: Value,
    ) -> Result<Value> {
        if !self.callable(&executor) {
            return self.throw_error("TypeError", "promise executor is not callable");
        }
        let proto = self.get_member(new_target, "prototype")?;
        let proto = if let Value::Object(id) = proto {
            id
        } else {
            self.intrinsics.as_ref().unwrap().promise
        };
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.promise = Some(PromiseState {
            state: State::Pending,
            reactions: vec![],
            handled: false,
        });
        self.replace_data(id, data)?;
        let (resolve, reject) = self.resolving_functions(id)?;
        match self.invoke(executor, vec![resolve, reject.clone()]) {
            Ok(_) => {}
            Err(ScriptError::Thrown(ex)) => {
                self.protect(&ex.value)?;
                self.invoke(reject, vec![ex.value])?;
            }
            Err(e) => return Err(e),
        }
        Ok(Value::Object(id))
    }
    pub(super) fn capability(&mut self, constructor: Value) -> Result<Capability> {
        let cell = self.new_object(None)?;
        self.protect(&Value::Object(cell))?;
        let mut data = self.data(cell)?.clone();
        data.cell = Some(Cell::Capability {
            resolve: Value::Undefined,
            reject: Value::Undefined,
        });
        self.replace_data(cell, data)?;
        let executor = self.internal_function(Internal::Capability { cell }, 2)?;
        let promise = self.construct(constructor, vec![executor])?;
        self.protect(&promise)?;
        let Some(Cell::Capability { resolve, reject }) = self.data(cell)?.cell.clone() else {
            unreachable!()
        };
        if !self.callable(&resolve) || !self.callable(&reject) {
            return self.throw_error("TypeError", "invalid promise capability");
        }
        Ok(Capability {
            promise,
            resolve,
            reject,
        })
    }
    fn settle(&mut self, id: Id, value: Value, reject: bool) -> Result<()> {
        self.protect(&value)?;
        let mut data = self.data(id)?.clone();
        let p = data
            .promise
            .as_mut()
            .ok_or(ScriptError::Runtime("missing promise record"))?;
        if !matches!(p.state, State::Pending) {
            return Ok(());
        }
        let reactions = std::mem::take(&mut p.reactions);
        let jobs: Vec<_> = reactions
            .into_iter()
            .map(|r| Job::Reaction {
                handler: if reject { r.rejected } else { r.fulfilled },
                argument: value.clone(),
                reject,
                capability: r.capability,
            })
            .collect();
        for job in &jobs {
            for id in job.edges() {
                self.protect(&Value::Object(id))?;
            }
        }
        p.state = if reject {
            State::Rejected(value)
        } else {
            State::Fulfilled(value)
        };
        self.replace_data(id, data)?;
        if reject && !self.data(id)?.promise.as_ref().unwrap().handled {
            let State::Rejected(value) = &self.data(id)?.promise.as_ref().unwrap().state else {
                unreachable!()
            };
            let assertion = matches!(value,Value::Object(error)if self.data(*error).is_ok_and(|d|d.error_class.as_deref()==Some("Test262Error")));
            let diagnostic = format!("{:?}", self.thrown(value.clone()));
            if self.unhandled.len() >= self.microtask_queue_limit {
                return Err(ScriptError::Runtime("unhandled rejection diagnostic limit"));
            }
            self.unhandled.insert(id, (assertion, diagnostic));
        }
        for job in jobs {
            self.enqueue_job(job)?;
        }
        Ok(())
    }
    fn resolve_value(&mut self, id: Id, value: Value) -> Result<()> {
        if value == Value::Object(id) {
            let error = self.error_value("TypeError", "promise resolves to itself")?;
            return self.settle(id, error, true);
        }
        if !matches!(value, Value::Object(_)) {
            return self.settle(id, value, false);
        }
        let then = match self.get_member(value.clone(), "then") {
            Ok(v) => v,
            Err(ScriptError::Thrown(ex)) => return self.settle(id, ex.value, true),
            Err(e) => return Err(e),
        };
        self.protect(&then)?;
        if !self.callable(&then) {
            return self.settle(id, value, false);
        }
        let (resolve, reject) = self.resolving_functions(id)?;
        self.enqueue_job(Job::Thenable {
            thenable: value,
            then,
            resolve,
            reject,
        })
    }
    fn species(&mut self, object: Value) -> Result<Value> {
        let default = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let c = self.get_member(object, "constructor")?;
        self.protect(&c)?;
        if c == Value::Undefined {
            return Ok(default);
        }
        if !matches!(c, Value::Object(_)) {
            return self.throw_error("TypeError", "promise constructor is not object");
        }
        let species = self.get_member(c, Key::Symbol(2))?;
        self.protect(&species)?;
        if matches!(species, Value::Null | Value::Undefined) {
            Ok(default)
        } else {
            if !self.constructible(&species) {
                return self.throw_error("TypeError", "promise species is not constructor");
            }
            Ok(species)
        }
    }
    pub(super) fn promise_resolve(&mut self, constructor: Value, value: Value) -> Result<Value> {
        if !matches!(constructor, Value::Object(_)) {
            return self.throw_error("TypeError", "promise constructor is not object");
        }
        if let Value::Object(id) = value {
            // IsPromise (§27.2.1.6): only objects with a [[PromiseState]] slot;
            // host nodes and proxies have none.
            if !matches!(
                self.objects.get(&id),
                Some(Object::Node { .. } | Object::Proxy { .. })
            ) && self.data(id)?.promise.is_some()
            {
                let c = self.get_member(Value::Object(id), "constructor")?;
                if c.same_value(&constructor) {
                    return Ok(Value::Object(id));
                }
            }
        }
        let cap = self.capability(constructor)?;
        self.invoke(cap.resolve, vec![value])?;
        Ok(cap.promise)
    }
    pub(super) fn promise_internal(&mut self, action: Internal, args: Vec<Value>) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match action {
            Internal::AggregateElement {
                group,
                guard,
                index,
                reject,
            } => self.aggregate_element(group, guard, index, reject, arg(0)),
            Internal::ModuleSettled { index, reject } => self.module_settled(index, arg(0), reject),
            Internal::DynamicImport { slot, step } => self.dynamic_import_step(slot, step, arg(0)),
            Internal::AsyncResume { activation, reject } => {
                let generator = self
                    .data(activation)?
                    .activation
                    .as_ref()
                    .and_then(|a| a.async_generator());
                if let Some(generator) = generator {
                    // The finished activation no longer reaches the generator; keep it for the drain.
                    self.protect(&Value::Object(generator))?;
                }
                self.resume_async(activation, arg(0), reject)?;
                if let Some(generator) = generator {
                    self.async_generator_drain(generator)?;
                }
                Ok(Value::Undefined)
            }
            Internal::IterResult { done } => self.iterator_result(arg(0), done),
            Internal::FromAsyncStep { job, reject } => self.from_async_step(job, arg(0), reject),
            Internal::DisposeStep { job, reject } => self.dispose_step(job, arg(0), reject),
            Internal::CloseSyncIterator { iterator } => {
                self.close_sync_iterator_rejected(iterator, arg(0))
            }
            Internal::AsyncGeneratorReturn { generator, reject } => {
                self.async_generator_awaited_return(generator, arg(0), reject)?;
                Ok(Value::Undefined)
            }
            Internal::Resolve { cell, reject } => {
                let mut data = self.data(cell)?.clone();
                let Some(Cell::Resolving { promise, resolved }) = data.cell.as_mut() else {
                    unreachable!()
                };
                if *resolved {
                    return Ok(Value::Undefined);
                }
                *resolved = true;
                let id = *promise;
                self.replace_data(cell, data)?;
                if reject {
                    self.settle(id, arg(0), true)?;
                } else {
                    self.resolve_value(id, arg(0))?;
                }
                Ok(Value::Undefined)
            }
            Internal::Capability { cell } => {
                let mut data = self.data(cell)?.clone();
                let Some(Cell::Capability { resolve, reject }) = data.cell.as_mut() else {
                    unreachable!()
                };
                if *resolve != Value::Undefined || *reject != Value::Undefined {
                    return self.throw_error("TypeError", "capability executor called twice");
                }
                *resolve = arg(0);
                *reject = arg(1);
                self.replace_data(cell, data)?;
                Ok(Value::Undefined)
            }
            Internal::Finally {
                handler,
                constructor,
                reject,
            } => {
                let result = self.invoke(handler, vec![])?;
                self.protect(&result)?;
                let promise = self.promise_resolve(constructor, result)?;
                self.protect(&promise)?;
                let thunk = self.internal_function(
                    Internal::Thunk {
                        value: arg(0),
                        reject,
                    },
                    0,
                )?;
                let then = self.get_member(promise.clone(), "then")?;
                self.protect(&then)?;
                self.invoke_this(then, promise, vec![thunk])
            }
            Internal::Thunk { value, reject } => {
                if reject {
                    Err(self.thrown(value))
                } else {
                    Ok(value)
                }
            }
        }
    }
    pub(super) fn attach_reaction(
        &mut self,
        id: Id,
        fulfilled: Option<Value>,
        rejected: Option<Value>,
        capability: Option<Capability>,
    ) -> Result<()> {
        let reaction = Reaction {
            fulfilled,
            rejected,
            capability,
        };
        let mut data = self.data(id)?.clone();
        let p = data
            .promise
            .as_mut()
            .ok_or(ScriptError::Runtime("missing promise record"))?;
        p.handled = true;
        let job = match &p.state {
            State::Pending => {
                p.reactions.push(reaction);
                None
            }
            State::Fulfilled(v) => Some(Job::Reaction {
                handler: reaction.fulfilled,
                argument: v.clone(),
                reject: false,
                capability: reaction.capability,
            }),
            State::Rejected(v) => Some(Job::Reaction {
                handler: reaction.rejected,
                argument: v.clone(),
                reject: true,
                capability: reaction.capability,
            }),
        };
        self.replace_data(id, data)?;
        self.unhandled.remove(&id);
        if let Some(job) = job {
            self.enqueue_job(job)?;
        }
        Ok(())
    }
    pub(super) fn promise_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "Promise" => self.throw_error("TypeError", "Promise requires new"),
            "Promise.species" => Ok(receiver),
            "Promise.resolve" => self.promise_resolve(receiver, arg(0)),
            "Promise.all"
            | "Promise.race"
            | "Promise.allSettled"
            | "Promise.any"
            | "Promise.withResolvers"
            | "Promise.try"
            | "Promise.allKeyed"
            | "Promise.allSettledKeyed" => self.promise_combinator(name, receiver, args),
            "Promise.reject" => {
                let cap = self.capability(receiver)?;
                self.invoke(cap.reject, vec![arg(0)])?;
                Ok(cap.promise)
            }
            "queueMicrotask" => {
                if !self.callable(&arg(0)) {
                    return self.throw_error("TypeError", "microtask callback is not callable");
                }
                self.enqueue_job(Job::Callback(arg(0)))?;
                Ok(Value::Undefined)
            }
            "$DONE" => {
                self.done_count += 1;
                if arg(0) != Value::Undefined {
                    self.done_error = Some(format!("{:?}", self.thrown(arg(0))));
                }
                Ok(Value::Undefined)
            }
            "Promise.prototype.then" => {
                let Value::Object(id) = receiver.clone() else {
                    return self.throw_error("TypeError", "then receiver is not promise");
                };
                if self.data(id)?.promise.is_none() {
                    return self.throw_error("TypeError", "then receiver is not promise");
                }
                let c = self.species(receiver)?;
                let cap = self.capability(c)?;
                self.attach_reaction(
                    id,
                    if self.callable(&arg(0)) {
                        Some(arg(0))
                    } else {
                        None
                    },
                    if self.callable(&arg(1)) {
                        Some(arg(1))
                    } else {
                        None
                    },
                    Some(cap.clone()),
                )?;
                Ok(cap.promise)
            }
            "Promise.prototype.catch" => {
                let method = self.get_member(receiver.clone(), "then")?;
                self.protect(&method)?;
                self.invoke_this(method, receiver, vec![Value::Undefined, arg(0)])
            }
            "Promise.prototype.finally" => {
                if !matches!(receiver, Value::Object(_)) {
                    return self.throw_error("TypeError", "finally receiver is not object");
                }
                let c = self.species(receiver.clone())?;
                let (fulfilled, rejected) = if self.callable(&arg(0)) {
                    (
                        self.internal_function(
                            Internal::Finally {
                                handler: arg(0),
                                constructor: c.clone(),
                                reject: false,
                            },
                            1,
                        )?,
                        self.internal_function(
                            Internal::Finally {
                                handler: arg(0),
                                constructor: c,
                                reject: true,
                            },
                            1,
                        )?,
                    )
                } else {
                    (arg(0), arg(0))
                };
                let method = self.get_member(receiver.clone(), "then")?;
                self.protect(&method)?;
                self.invoke_this(method, receiver, vec![fulfilled, rejected])
            }
            _ => Err(ScriptError::Unsupported("promise operation")),
        }
    }
}
pub(super) fn empty_queue() -> VecDeque<QueuedJob> {
    VecDeque::new()
}
