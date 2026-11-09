//! Array.fromAsync (ECMA-262 §23.1.2.2, ES2026) as a native async state machine.
//!
//! The spec runs an abstract closure under AsyncFunctionStart. Here the closure's
//! state lives in a traced `Cell::FromAsync` on a job object; each `Await` attaches
//! `Internal::FromAsyncStep` reactions that resume the machine at the saved phase.
//! The sync-iterator branch goes through the CreateAsyncFromSyncIterator steps
//! (§27.1.6.4 AsyncFromSyncIteratorContinuation), including closing the sync
//! iterator when a yielded value rejects.
use super::{
    objects::Key,
    promises::{Capability, Cell, Internal},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

const ASYNC_ITERATOR: u64 = 12;
const MAX_SAFE: u64 = 9007199254740991;

#[derive(Clone)]
pub(super) enum Source {
    Iterator {
        iterator: Value,
        next: Value,
        /// A sync iterator wrapped by CreateAsyncFromSyncIterator.
        sync: bool,
    },
    ArrayLike {
        object: Id,
        len: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// Awaiting the iterator's next result (step 3.h.iii.4).
    Next,
    /// Awaiting the mapped value (steps 3.h.iii.9 and 3.i.v.6).
    Mapped,
    /// Awaiting an array-like element (step 3.i.v.4).
    Value,
    /// AsyncIteratorClose awaiting the `return` result before rethrowing.
    Close,
}

#[derive(Clone)]
pub(super) struct FromAsync {
    capability: Capability,
    mapfn: Value,
    this_arg: Value,
    out: Value,
    k: u64,
    source: Source,
    phase: Phase,
    /// The pending throw completion while closing the iterator.
    error: Value,
}

impl FromAsync {
    pub fn edges(&self) -> Vec<Id> {
        let mut values = self.capability.values();
        values.extend([
            self.mapfn.clone(),
            self.this_arg.clone(),
            self.out.clone(),
            self.error.clone(),
        ]);
        match &self.source {
            Source::Iterator { iterator, next, .. } => {
                values.push(iterator.clone());
                values.push(next.clone());
            }
            Source::ArrayLike { object, .. } => values.push(Value::Object(*object)),
        }
        values.iter().filter_map(Value::id).collect()
    }
}

impl Engine<'_> {
    /// Array.fromAsync ( asyncItems [ , mapper [ , thisArg ] ] )
    pub(super) fn array_from_async(&mut self, receiver: Value, args: Vec<Value>) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
        let promise = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let cap = self.capability(promise)?;
        // 4. AsyncFunctionStart(promiseCapability, fromAsyncClosure).
        let started = self.from_async_start(receiver, arg(0), arg(1), arg(2), cap.clone());
        self.from_async_settle(&cap, started)?;
        Ok(cap.promise)
    }

    fn from_async_settle(&mut self, cap: &Capability, outcome: Result<()>) -> Result<()> {
        match outcome {
            Ok(()) => Ok(()),
            Err(ScriptError::Thrown(error)) => {
                self.protect(&error.value)?;
                self.invoke(cap.reject.clone(), vec![error.value])?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// GetMethod (§7.3.10) on a value that may be a primitive.
    fn from_async_method(&mut self, value: Value, key: Key) -> Result<Value> {
        let method = self.get_member(value, key)?;
        self.protect(&method)?;
        if matches!(method, Value::Undefined | Value::Null) {
            return Ok(Value::Undefined);
        }
        if !self.callable(&method) {
            return self.throw_error("TypeError", "iterator method is not callable");
        }
        Ok(method)
    }

    fn from_async_start(
        &mut self,
        constructor: Value,
        items: Value,
        mapfn: Value,
        this_arg: Value,
        cap: Capability,
    ) -> Result<()> {
        // 3.a-b. mapping
        if mapfn != Value::Undefined && !self.callable(&mapfn) {
            return self.throw_error("TypeError", "Array.fromAsync mapper is not callable");
        }
        if matches!(items, Value::Undefined | Value::Null) {
            return self.throw_error("TypeError", "Array.fromAsync input is null or undefined");
        }
        // 3.c-d. usingAsyncIterator, usingSyncIterator
        let mut sync = false;
        let mut method = self.from_async_method(items.clone(), Key::Symbol(ASYNC_ITERATOR))?;
        if method == Value::Undefined {
            method = self.from_async_method(items.clone(), Key::Symbol(0))?;
            sync = method != Value::Undefined;
        }
        let (source, out) = if method != Value::Undefined {
            // 3.f-g. GetIteratorFromMethod (§7.4.4), wrapped for the sync case.
            let iterator = self.invoke_this(method, items, vec![])?;
            self.protect(&iterator)?;
            if !matches!(iterator, Value::Object(_)) {
                return self.throw_error("TypeError", "iterator is not an object");
            }
            let next = self.get_member(iterator.clone(), "next")?;
            self.protect(&next)?;
            // 3.h.i. If IsConstructor(C), let A be ? Construct(C); else ! ArrayCreate(0).
            let out = if self.constructible(&constructor) {
                self.construct(constructor, vec![])?
            } else {
                Value::Object(self.new_array(0)?)
            };
            (
                Source::Iterator {
                    iterator,
                    next,
                    sync,
                },
                out,
            )
        } else {
            // 3.i. Array-like: ! ToObject, ? LengthOfArrayLike, Construct(C, « len »).
            let object = self.object_coercible(items)?;
            self.protect(&Value::Object(object))?;
            let len = self.length_of(object)?;
            let out = if self.constructible(&constructor) {
                self.construct(constructor, vec![Value::number(len as f64)])?
            } else {
                // ArrayCreate (§10.4.2.2) step 1: length > 2^32 - 1 is a RangeError.
                let Ok(n) = u32::try_from(len) else {
                    return self.throw_error("RangeError", "Invalid array length");
                };
                Value::Object(self.new_array(n)?)
            };
            (Source::ArrayLike { object, len }, out)
        };
        self.protect(&out)?;
        let job = self.new_object(None)?;
        self.protect(&Value::Object(job))?;
        let state = FromAsync {
            capability: cap,
            mapfn,
            this_arg,
            out,
            k: 0,
            source,
            phase: Phase::Next,
            error: Value::Undefined,
        };
        self.from_async_save(job, state.clone())?;
        self.from_async_loop(job, state)
    }

    fn from_async_state(&self, job: Id) -> Result<FromAsync> {
        let Some(Cell::FromAsync(state)) = self.data(job)?.cell.clone() else {
            return Err(ScriptError::Runtime("missing Array.fromAsync state"));
        };
        Ok(state)
    }

    fn from_async_save(&mut self, job: Id, state: FromAsync) -> Result<()> {
        let mut data = self.data(job)?.clone();
        data.cell = Some(Cell::FromAsync(state));
        self.replace_data(job, data)
    }

    /// Await (§27.7.5.3): PromiseResolve(%Promise%, value), then resume at `phase`.
    fn from_async_await(
        &mut self,
        job: Id,
        mut state: FromAsync,
        phase: Phase,
        value: Value,
    ) -> Result<()> {
        let promise = self.promise_resolve(
            Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]),
            value,
        )?;
        self.protect(&promise)?;
        state.phase = phase;
        self.from_async_save(job, state)?;
        let yes = self.internal_function(Internal::FromAsyncStep { job, reject: false }, 1)?;
        let no = self.internal_function(Internal::FromAsyncStep { job, reject: true }, 1)?;
        let Value::Object(p) = promise else {
            unreachable!()
        };
        self.attach_reaction(p, Some(yes), Some(no), None)
    }

    /// The reaction entry point: resume the closure with a fulfilled or rejected value.
    pub(super) fn from_async_step(&mut self, job: Id, value: Value, reject: bool) -> Result<Value> {
        self.protect(&value)?;
        let state = self.from_async_state(job)?;
        let cap = state.capability.clone();
        let outcome = self.from_async_resume(job, state, value, reject);
        self.from_async_settle(&cap, outcome)?;
        Ok(Value::Undefined)
    }

    fn from_async_resume(
        &mut self,
        job: Id,
        mut state: FromAsync,
        value: Value,
        reject: bool,
    ) -> Result<()> {
        let iterating = matches!(state.source, Source::Iterator { .. });
        match state.phase {
            Phase::Close => {
                // AsyncIteratorClose with a throw completion returns that completion,
                // whatever happened to the `return` call.
                Err(self.thrown(state.error))
            }
            Phase::Next if reject => Err(self.thrown(value)),
            Phase::Next => {
                // 3.h.iii.5-8
                if !matches!(value, Value::Object(_)) {
                    return self.throw_error("TypeError", "iterator result is not an object");
                }
                let done = self.get_member(value.clone(), "done")?.truthy();
                if done {
                    self.set_member(state.out.clone(), "length", Value::number(state.k as f64))?;
                    return self.from_async_finish(&state);
                }
                let next = self.get_member(value, "value")?;
                self.protect(&next)?;
                self.from_async_value(job, state, next)
            }
            Phase::Value if reject => Err(self.thrown(value)),
            Phase::Value => self.from_async_value(job, state, value),
            Phase::Mapped if reject && iterating => {
                // IfAbruptCloseAsyncIterator(mappedValue, iteratorRecord)
                self.from_async_close(job, state, value)
            }
            Phase::Mapped if reject => Err(self.thrown(value)),
            Phase::Mapped => {
                if self.from_async_define(job, &mut state, value)? {
                    self.from_async_loop(job, state)?;
                }
                Ok(())
            }
        }
    }

    /// A value is ready: map it (and await the mapping) or store it.
    fn from_async_value(&mut self, job: Id, mut state: FromAsync, value: Value) -> Result<()> {
        let iterating = matches!(state.source, Source::Iterator { .. });
        if state.mapfn != Value::Undefined {
            let mapped = self.invoke_this(
                state.mapfn.clone(),
                state.this_arg.clone(),
                vec![value, Value::number(state.k as f64)],
            );
            return match mapped {
                Ok(mapped) => {
                    self.protect(&mapped)?;
                    self.from_async_await(job, state, Phase::Mapped, mapped)
                }
                Err(ScriptError::Thrown(error)) if iterating => {
                    self.protect(&error.value)?;
                    self.from_async_close(job, state, error.value)
                }
                Err(error) => Err(error),
            };
        }
        if self.from_async_define(job, &mut state, value)? {
            self.from_async_loop(job, state)?;
        }
        Ok(())
    }

    /// CreateDataPropertyOrThrow(A, Pk, mappedValue); k += 1. For iterators an
    /// abrupt completion closes the iterator (3.h.iii.10); false = closing.
    fn from_async_define(&mut self, job: Id, state: &mut FromAsync, value: Value) -> Result<bool> {
        let Value::Object(out) = state.out else {
            return self.throw_error("TypeError", "Array.fromAsync target is not an object");
        };
        match self.put_data(out, state.k.to_string(), value) {
            Ok(()) => {
                state.k += 1;
                Ok(true)
            }
            Err(ScriptError::Thrown(error)) if matches!(state.source, Source::Iterator { .. }) => {
                self.protect(&error.value)?;
                self.from_async_close(job, state.clone(), error.value)?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Run the loop until the next Await (or completion).
    fn from_async_loop(&mut self, job: Id, state: FromAsync) -> Result<()> {
        match state.source.clone() {
            Source::Iterator {
                iterator,
                next,
                sync,
            } => {
                if state.k >= MAX_SAFE {
                    let error = self.error_constructor(
                        "TypeError",
                        vec![Value::String("Array.fromAsync exceeds safe length".into())],
                        None,
                    )?;
                    self.protect(&error)?;
                    return self.from_async_close(job, state, error);
                }
                let result = if sync {
                    self.async_from_sync_next(iterator, next)?
                } else {
                    let result = self.invoke_this(next, iterator, vec![])?;
                    self.protect(&result)?;
                    result
                };
                self.from_async_await(job, state, Phase::Next, result)
            }
            Source::ArrayLike { object, len } => {
                if state.k < len {
                    let value = self.get_member(Value::Object(object), state.k.to_string())?;
                    self.protect(&value)?;
                    return self.from_async_await(job, state, Phase::Value, value);
                }
                self.set_member(state.out.clone(), "length", Value::number(len as f64))?;
                self.from_async_finish(&state)
            }
        }
    }

    fn from_async_finish(&mut self, state: &FromAsync) -> Result<()> {
        self.invoke(state.capability.resolve.clone(), vec![state.out.clone()])?;
        Ok(())
    }

    /// AsyncIteratorClose(iteratorRecord, ThrowCompletion(error)) (§7.4.13).
    fn from_async_close(&mut self, job: Id, mut state: FromAsync, error: Value) -> Result<()> {
        let Source::Iterator { iterator, sync, .. } = state.source.clone() else {
            return Err(self.thrown(error));
        };
        state.error = error.clone();
        if sync {
            // %AsyncFromSyncIteratorPrototype%.return on the wrapped iterator:
            // errors are swallowed by the throw completion; one await follows.
            let closed = self.close_iterator(iterator, Ok(None));
            if let Err(e) = closed {
                if !matches!(e, ScriptError::Thrown(_)) {
                    return Err(e);
                }
            }
            return self.from_async_await(job, state, Phase::Close, Value::Undefined);
        }
        let method = match self.get_member(iterator.clone(), "return") {
            Ok(m) => m,
            Err(ScriptError::Thrown(_)) => return Err(self.thrown(error)),
            Err(e) => return Err(e),
        };
        self.protect(&method)?;
        if matches!(method, Value::Undefined | Value::Null) {
            return Err(self.thrown(error));
        }
        let result = match self.invoke_this(method, iterator, vec![]) {
            Ok(v) => v,
            Err(ScriptError::Thrown(_)) => return Err(self.thrown(error)),
            Err(e) => return Err(e),
        };
        self.protect(&result)?;
        match self.from_async_await(job, state, Phase::Close, result) {
            Err(ScriptError::Thrown(_)) => Err(self.thrown(error)),
            r => r,
        }
    }

    /// %AsyncFromSyncIteratorPrototype%.next (§27.1.6.2.1) followed by
    /// AsyncFromSyncIteratorContinuation (§27.1.6.4) with closeOnRejection = true.
    /// Returns the promise for the iterator result; abrupt steps reject it.
    fn async_from_sync_next(&mut self, iterator: Value, next: Value) -> Result<Value> {
        let ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let cap = self.capability(ctor.clone())?;
        let step: Result<()> = (|| {
            let result = self.invoke_this(next, iterator.clone(), vec![])?;
            self.protect(&result)?;
            if !matches!(result, Value::Object(_)) {
                return self.throw_error("TypeError", "iterator result is not an object");
            }
            let done = self.get_member(result.clone(), "done")?.truthy();
            let value = self.get_member(result, "value")?;
            self.protect(&value)?;
            let wrapper = match self.promise_resolve(ctor.clone(), value) {
                Ok(w) => w,
                Err(ScriptError::Thrown(error)) if !done => {
                    self.protect(&error.value)?;
                    return self
                        .close_iterator(iterator.clone(), Err(ScriptError::Thrown(error)))
                        .map(|_| ());
                }
                Err(e) => return Err(e),
            };
            self.protect(&wrapper)?;
            let fulfilled = self.internal_function(Internal::IterResult { done }, 1)?;
            let rejected = if done {
                None
            } else {
                Some(self.internal_function(
                    Internal::CloseSyncIterator {
                        iterator: iterator.clone(),
                    },
                    1,
                )?)
            };
            let Value::Object(p) = wrapper else {
                unreachable!()
            };
            self.attach_reaction(p, Some(fulfilled), rejected, Some(cap.clone()))
        })();
        match step {
            Ok(()) => {}
            Err(ScriptError::Thrown(error)) => {
                self.protect(&error.value)?;
                self.invoke(cap.reject.clone(), vec![error.value])?;
            }
            Err(e) => return Err(e),
        }
        self.protect(&cap.promise)?;
        Ok(cap.promise)
    }

    /// The closeIterator closure of AsyncFromSyncIteratorContinuation step 12:
    /// IteratorClose(syncIteratorRecord, ThrowCompletion(error)).
    pub(super) fn close_sync_iterator_rejected(
        &mut self,
        iterator: Value,
        error: Value,
    ) -> Result<Value> {
        self.protect(&error)?;
        let thrown = self.thrown(error);
        self.close_iterator(iterator, Err(thrown))?;
        Ok(Value::Undefined)
    }
}
